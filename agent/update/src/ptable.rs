//! The partition table at the start of a wic image: GPT on x86, MBR on the
//! Pi. Only what an update needs - where each partition is and its PARTUUID,
//! which is what the kernel command line and fstab name it by.

/// wic always writes 512-byte sectors.
pub const SECTOR: u64 = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partition {
    /// 1-based, as in `/dev/sda2`.
    pub number: u32,
    /// Bytes from the start of the disk.
    pub start: u64,
    pub size: u64,
    /// Lower case, the way `/dev/disk/by-partuuid` spells it.
    pub partuuid: String,
}

impl Partition {
    pub fn end(&self) -> u64 {
        self.start + self.size
    }
}

pub fn find(partitions: &[Partition], number: u32) -> Option<&Partition> {
    partitions
        .iter()
        .find(|partition| partition.number == number)
}

/// `head` is the start of the image; the first MiB is always enough.
pub fn parse(head: &[u8]) -> Result<Vec<Partition>, String> {
    if head.len() < 1024 || head[510..512] != [0x55, 0xAA] {
        return Err("the image has no partition table".to_string());
    }
    let protective = (0..4).any(|slot| head[446 + 16 * slot + 4] == 0xEE);
    if protective {
        gpt(head)
    } else {
        mbr(head)
    }
}

fn gpt(head: &[u8]) -> Result<Vec<Partition>, String> {
    let header = &head[SECTOR as usize..];
    if &header[0..8] != b"EFI PART" {
        return Err("the image has a protective MBR but no GPT header".to_string());
    }
    let entries_lba = u64le(header, 72);
    let count = u32le(header, 80) as usize;
    let entry_size = u32le(header, 84) as usize;
    if entry_size < 128 || count > 1024 {
        return Err("the image's GPT header is malformed".to_string());
    }
    let first = (entries_lba * SECTOR) as usize;
    let end = first + count * entry_size;
    if end > head.len() {
        return Err("the image's GPT entries are not where they should be".to_string());
    }

    let mut partitions = Vec::new();
    for index in 0..count {
        let entry = &head[first + index * entry_size..][..entry_size];
        if entry[0..16].iter().all(|byte| *byte == 0) {
            continue;
        }
        let first_lba = u64le(entry, 32);
        let last_lba = u64le(entry, 40);
        if last_lba < first_lba {
            return Err(format!("GPT partition {} is malformed", index + 1));
        }
        partitions.push(Partition {
            number: index as u32 + 1,
            start: first_lba * SECTOR,
            size: (last_lba - first_lba + 1) * SECTOR,
            partuuid: guid(&entry[16..32]),
        });
    }
    Ok(partitions)
}

fn mbr(head: &[u8]) -> Result<Vec<Partition>, String> {
    let signature = u32le(head, 440);
    let mut partitions = Vec::new();
    for slot in 0..4 {
        let entry = &head[446 + 16 * slot..][..16];
        let kind = entry[4];
        // Empty, or an extended container: no Tessaro layout uses logical
        // partitions, and the ones an update touches are always primary.
        if matches!(kind, 0x00 | 0x05 | 0x0f | 0x85) {
            continue;
        }
        partitions.push(Partition {
            number: slot as u32 + 1,
            start: u32le(entry, 8) as u64 * SECTOR,
            size: u32le(entry, 12) as u64 * SECTOR,
            // The kernel's PARTUUID for MBR: disk signature, then the number.
            partuuid: format!("{signature:08x}-{:02x}", slot + 1),
        });
    }
    Ok(partitions)
}

/// A GUID as text. The first three fields are stored little-endian.
fn guid(bytes: &[u8]) -> String {
    format!(
        "{:08x}-{:04x}-{:04x}-{}-{}",
        u32le(bytes, 0),
        u16le(bytes, 4),
        u16le(bytes, 6),
        crate::hex(&bytes[8..10]),
        crate::hex(&bytes[10..16])
    )
}

fn u16le(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
}

fn u32le(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn u64le(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::write_gpt;

    #[test]
    fn gpt_partitions_and_their_partuuids() {
        let mut image = vec![0u8; 1 << 20];
        let guid = [
            0x76, 0xd6, 0xb3, 0x09, 0x01, 0x02, 0x03, 0x04, 0xaa, 0xbb, 0x10, 0x11, 0x12, 0x13,
            0x14, 0x15,
        ];
        write_gpt(&mut image, &[(2048, 2048, [1; 16]), (4096, 8192, guid)]);

        let partitions = parse(&image).unwrap();
        assert_eq!(partitions.len(), 2);
        let root = find(&partitions, 2).unwrap();
        assert_eq!(root.start, 4096 * 512);
        assert_eq!(root.size, 8192 * 512);
        assert_eq!(root.partuuid, "09b3d676-0201-0403-aabb-101112131415");
    }

    #[test]
    fn mbr_partuuids_come_from_the_disk_signature() {
        let mut image = vec![0u8; 4096];
        image[440..444].copy_from_slice(&0x1234abcdu32.to_le_bytes());
        image[510] = 0x55;
        image[511] = 0xAA;
        let slot = |image: &mut Vec<u8>, n: usize, kind: u8, start: u32, sectors: u32| {
            let at = 446 + 16 * n;
            image[at + 4] = kind;
            image[at + 8..at + 12].copy_from_slice(&start.to_le_bytes());
            image[at + 12..at + 16].copy_from_slice(&sectors.to_le_bytes());
        };
        slot(&mut image, 0, 0x0c, 8192, 204800);
        slot(&mut image, 1, 0x83, 212992, 4_000_000);

        let partitions = parse(&image).unwrap();
        assert_eq!(partitions[1].number, 2);
        assert_eq!(partitions[1].partuuid, "1234abcd-02");
        assert_eq!(partitions[1].start, 212992 * 512);
    }

    #[test]
    fn no_table_is_an_error() {
        assert!(parse(&[0u8; 4096]).is_err());
    }
}
