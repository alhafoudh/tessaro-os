//! Synthetic disks for tests, here and in tessaro-agent (feature `testing`).
//! The build makes the real images and bmaps; nothing here ships.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

use crate::image::Image;
use crate::layout::Probe;
use crate::manifest::{Manifest, Mode, Source};
use crate::prepare::{self, Observer};
use crate::ptable::SECTOR;

pub const BLOCK: u64 = 4096;
pub const MIB: u64 = 1 << 20;
/// The size of `device()`'s disk, in sectors: 16 MiB.
pub const DISK_SECTORS: u64 = 32768;
pub const BOOT_UUID: [u8; 16] = [1; 16];
pub const ROOT_UUID: [u8; 16] = [2; 16];
/// The same UUIDs, as `/dev/disk/by-partuuid` spells them.
pub const BOOT_PARTUUID: &str = "01010101-0101-0101-0101-010101010101";
pub const ROOT_PARTUUID: &str = "02020202-0202-0202-0202-020202020202";

/// A GPT for `parts` (first LBA, sector count, the GUID's 16 on-disk
/// bytes), written into the start of `image`.
pub fn write_gpt(image: &mut [u8], parts: &[(u64, u64, [u8; 16])]) {
    // Protective MBR.
    image[446 + 4] = 0xEE;
    image[510] = 0x55;
    image[511] = 0xAA;
    let header = SECTOR as usize;
    image[header..header + 8].copy_from_slice(b"EFI PART");
    image[header + 72..header + 80].copy_from_slice(&2u64.to_le_bytes());
    image[header + 80..header + 84].copy_from_slice(&128u32.to_le_bytes());
    image[header + 84..header + 88].copy_from_slice(&128u32.to_le_bytes());
    for (index, (first, sectors, guid)) in parts.iter().enumerate() {
        let entry = 2 * SECTOR as usize + index * 128;
        // Any non-zero type GUID.
        image[entry] = 0x42;
        image[entry + 16..entry + 32].copy_from_slice(guid);
        image[entry + 32..entry + 40].copy_from_slice(&first.to_le_bytes());
        image[entry + 40..entry + 48].copy_from_slice(&(first + sectors - 1).to_le_bytes());
    }
}

/// A bmap for `image` in bmaptool's format, mapping `ranges` (block
/// numbers, inclusive).
pub fn render_bmap(image: &[u8], block_size: u64, ranges: &[(u64, u64)]) -> String {
    let blocks = (image.len() as u64).div_ceil(block_size);
    let mut body = String::new();
    for &(first, last) in ranges {
        let start = (first * block_size) as usize;
        let end = (((last + 1) * block_size) as usize).min(image.len());
        let label = if first == last {
            first.to_string()
        } else {
            format!("{first}-{last}")
        };
        body.push_str(&format!(
            "        <Range chksum=\"{}\"> {label} </Range>\n",
            crate::sha256(&image[start..end])
        ));
    }
    let text = format!(
        "<?xml version=\"1.0\" ?>\n<bmap version=\"2.0\">\n    <ImageSize> {} </ImageSize>\n    \
         <BlockSize> {block_size} </BlockSize>\n    <BlocksCount> {blocks} </BlocksCount>\n    \
         <MappedBlocksCount> 0 </MappedBlocksCount>\n    <ChecksumType> sha256 </ChecksumType>\n    \
         <BmapFileChecksum> {} </BmapFileChecksum>\n    <BlockMap>\n{body}    </BlockMap>\n</bmap>\n",
        image.len(),
        "0".repeat(64)
    );
    let checksum = crate::sha256(text.as_bytes());
    text.replacen(&"0".repeat(64), &checksum, 1)
}

/// A 12 MiB disk and its bmap: boot at 1 MiB (1 MiB), root at 2 MiB
/// (6 MiB, so it spans more than one chunk), data at 8 MiB. Mapped: the
/// table, some of boot, most of root including a range that runs on into
/// data, and some of data.
pub fn disk() -> (Vec<u8>, String) {
    let mut image = vec![0u8; (12 * MIB) as usize];
    write_gpt(
        &mut image,
        &[
            (2048, 2048, BOOT_UUID),
            (4096, 12288, ROOT_UUID),
            (16384, 8192, [3; 16]),
        ],
    );
    for (index, byte) in image.iter_mut().enumerate().skip(MIB as usize) {
        *byte = (index % 251) as u8 + 1;
    }
    let block = |bytes: u64| bytes / BLOCK;
    let ranges = [
        (0, 4),
        (block(MIB), block(MIB) + 10),
        (block(2 * MIB), block(2 * MIB) + 1100),
        (block(7 * MIB), block(8 * MIB) + 3),
        (block(10 * MIB), block(10 * MIB) + 5),
    ];
    let text = render_bmap(&image, BLOCK, &ranges);
    (image, text)
}

/// A fake sysfs and udev tree under `dir` in which `disk()` is this
/// device's own disk: sda1 boot, sda2 root (booted as `root=`), sda3 data.
pub fn device(dir: &Path) -> Probe {
    let devices = dir.join("sys/devices/pci0/sda");
    let class = dir.join("sys/class/block");
    let by_partuuid = dir.join("dev/disk/by-partuuid");
    fs::create_dir_all(&class).unwrap();
    fs::create_dir_all(&by_partuuid).unwrap();
    for (name, number, start, size, uuid) in [
        ("sda1", 1, 2048, 2048, BOOT_PARTUUID),
        ("sda2", 2, 4096, 12288, ROOT_PARTUUID),
        (
            "sda3",
            3,
            16384,
            8192,
            "cccccccc-0000-0000-0000-000000000000",
        ),
    ] {
        let part = devices.join(name);
        fs::create_dir_all(&part).unwrap();
        fs::write(part.join("partition"), format!("{number}\n")).unwrap();
        fs::write(part.join("start"), format!("{start}\n")).unwrap();
        fs::write(part.join("size"), format!("{size}\n")).unwrap();
        symlink(&part, class.join(name)).unwrap();
        symlink(format!("../../{name}"), by_partuuid.join(uuid)).unwrap();
    }
    // The disk itself is in the class too, and has no partition file. It
    // is 16 MiB, room for `disk()` and some to spare.
    fs::write(devices.join("size"), format!("{DISK_SECTORS}\n")).unwrap();
    symlink(&devices, class.join("sda")).unwrap();
    let cmdline = dir.join("cmdline");
    fs::write(
        &cmdline,
        format!("BOOT_IMAGE=/bzImage root=PARTUUID={ROOT_PARTUUID} rootwait ro\n"),
    )
    .unwrap();
    Probe {
        cmdline,
        sys_block: class,
        by_partuuid,
    }
}

pub struct Quiet;

impl Observer for Quiet {
    fn progress(&mut self, _done: u64, _total: u64) {}
}

pub fn fake_kernel(_boot: &Path, dest: &Path) -> Result<String, String> {
    fs::write(dest, b"a kernel").map_err(|err| err.to_string())?;
    Ok("bzImage".to_string())
}

pub fn source() -> Source {
    Source {
        name: "tessaro.wic.bz2".to_string(),
        sha256: "0".repeat(64),
    }
}

/// `bytes` as the image stream prepare and apply read.
pub fn image(bytes: Vec<u8>) -> Image {
    Image::new(std::io::Cursor::new(bytes)).unwrap()
}

/// `bytes`, bz2-compressed in two streams, the way pbzip2 writes them.
pub fn compress(bytes: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut out = Vec::new();
    for half in bytes.chunks(bytes.len().div_ceil(2)) {
        let mut encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
        encoder.write_all(half).unwrap();
        out.extend(encoder.finish().unwrap());
    }
    out
}

/// `disk()` uploaded into `dir` and prepared there as a root update.
pub fn staged(dir: &Path) -> (Vec<u8>, Manifest) {
    staged_as(dir, Mode::Root)
}

/// `disk()` uploaded into `dir`, compressed, and prepared there for `mode`.
pub fn staged_as(dir: &Path, mode: Mode) -> (Vec<u8>, Manifest) {
    let (image, text) = disk();
    let upload = dir.join(crate::UPLOAD);
    fs::write(&upload, compress(&image)).unwrap();
    let bmap = crate::bmap::parse(&text).unwrap();
    let manifest = prepare::prepare(
        Image::open(&upload).unwrap(),
        &bmap,
        mode,
        source(),
        dir,
        |_| Ok(()),
        fake_kernel,
        &mut Quiet,
    )
    .unwrap();
    (image, manifest)
}
