//! This device's own disk: which partition the kernel booted as root, and
//! its boot partition next to it. An image is only applied to a disk laid
//! out exactly as the image expects - same PARTUUIDs, same root offset and
//! size - because the new root filesystem names those PARTUUIDs itself (the
//! fstab wic writes into it), and the partition table is never rewritten.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ptable::{self, Partition, SECTOR};
use crate::{megabytes, BOOT_PARTITION, ROOT_PARTITION};

/// Where to look. `/proc/cmdline`, `/sys/class/block`, `/dev/disk/by-partuuid`
/// on a device; a temporary tree in the tests.
#[derive(Debug, Clone)]
pub struct Probe {
    pub cmdline: PathBuf,
    pub sys_block: PathBuf,
    pub by_partuuid: PathBuf,
}

impl Default for Probe {
    fn default() -> Self {
        Self {
            cmdline: "/proc/cmdline".into(),
            sys_block: "/sys/class/block".into(),
            by_partuuid: "/dev/disk/by-partuuid".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Part {
    /// The kernel's name, `sda2` or `mmcblk0p2`.
    pub name: String,
    pub partuuid: String,
    pub start: u64,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    pub root: Part,
    pub boot: Part,
}

pub fn probe(probe: &Probe) -> Result<Layout, String> {
    let cmdline = fs::read_to_string(&probe.cmdline)
        .map_err(|err| format!("{}: {err}", probe.cmdline.display()))?;
    let spec = cmdline
        .split_whitespace()
        .find_map(|word| word.strip_prefix("root="))
        .ok_or("the kernel command line names no root=")?;

    let name = if let Some(uuid) = spec.strip_prefix("PARTUUID=") {
        link_name(&probe.by_partuuid.join(uuid.to_ascii_lowercase()))
            .ok_or_else(|| format!("no partition has PARTUUID {uuid}"))?
    } else if let Some(device) = spec.strip_prefix("/dev/") {
        device.to_string()
    } else {
        return Err(format!("root={spec} is not a form the updater understands"));
    };
    let root = part(probe, &name)?;

    // The boot partition is the one numbered BOOT_PARTITION on the same disk.
    let disk = fs::canonicalize(probe.sys_block.join(&name))
        .map_err(|err| format!("{name}: {err}"))?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("{name} has no parent disk"))?;
    let boot_name = fs::read_dir(&disk)
        .map_err(|err| format!("{}: {err}", disk.display()))?
        .filter_map(Result::ok)
        .find(|entry| read_number(&entry.path().join("partition")) == Some(BOOT_PARTITION as u64))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .ok_or_else(|| format!("the disk holding {name} has no partition {BOOT_PARTITION}"))?;
    let boot = part(probe, &boot_name)?;

    if read_number(&probe.sys_block.join(&name).join("partition")) != Some(ROOT_PARTITION as u64) {
        return Err(format!(
            "the root filesystem is on {name}, not on partition {ROOT_PARTITION}"
        ));
    }
    Ok(Layout { root, boot })
}

/// Refuse an image whose boot or root partition is not this disk's.
pub fn check(layout: &Layout, image: &[Partition]) -> Result<(), String> {
    let reflash = "the disk layout changed, so this device needs one full reflash \
                   (mise run image:flash) before it can take updates";
    let root = ptable::find(image, ROOT_PARTITION).ok_or("the image has no root partition")?;
    let boot = ptable::find(image, BOOT_PARTITION).ok_or("the image has no boot partition")?;

    // An MBR PARTUUID is the disk signature plus the number, and this wic
    // cannot pin the signature - but nothing on an MBR image names one
    // either: the Pi boots root=/dev/mmcblk0p2 and its fstab uses device
    // nodes. There the geometry is the whole check. GPT images name their
    // PARTUUIDs in grub.cfg and fstab, and the wks pins them.
    let mbr = is_mbr(&root.partuuid) && is_mbr(&layout.root.partuuid);
    if !mbr && (root.partuuid != layout.root.partuuid || boot.partuuid != layout.boot.partuuid) {
        return Err(format!(
            "the image's partitions are {} and {}, this device's are {} and {}: \
             either the image is for another machine, or {reflash}",
            boot.partuuid, root.partuuid, layout.boot.partuuid, layout.root.partuuid
        ));
    }
    if root.start != layout.root.start || root.size != layout.root.size {
        return Err(format!(
            "the image's root partition is {} at offset {}, this device's is {} at {}: {reflash}",
            megabytes(root.size),
            root.start,
            megabytes(layout.root.size),
            layout.root.start
        ));
    }
    if mbr && (boot.start != layout.boot.start || boot.size != layout.boot.size) {
        return Err(format!(
            "the image's boot partition is not where this device's is: {reflash}"
        ));
    }
    Ok(())
}

/// `1234abcd-02`, as opposed to a GPT GUID.
fn is_mbr(partuuid: &str) -> bool {
    partuuid.len() == 11 && partuuid.as_bytes()[8] == b'-'
}

fn part(probe: &Probe, name: &str) -> Result<Part, String> {
    let dir = probe.sys_block.join(name);
    let sectors = |file: &str| {
        read_number(&dir.join(file))
            .map(|value| value * SECTOR)
            .ok_or_else(|| format!("{}/{file} is unreadable", dir.display()))
    };
    Ok(Part {
        name: name.to_string(),
        partuuid: partuuid_of(probe, name)?,
        start: sectors("start")?,
        size: sectors("size")?,
    })
}

/// The PARTUUID udev linked to `name`.
fn partuuid_of(probe: &Probe, name: &str) -> Result<String, String> {
    fs::read_dir(&probe.by_partuuid)
        .map_err(|err| format!("{}: {err}", probe.by_partuuid.display()))?
        .filter_map(Result::ok)
        .find(|entry| link_name(&entry.path()).as_deref() == Some(name))
        .map(|entry| entry.file_name().to_string_lossy().to_ascii_lowercase())
        .ok_or_else(|| format!("{name} has no PARTUUID"))
}

/// The last component of where a symlink points: `../../sda2` is `sda2`.
fn link_name(link: &Path) -> Option<String> {
    fs::read_link(link)
        .ok()?
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

fn read_number(path: &Path) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, BOOT_PARTUUID as BOOT, ROOT_PARTUUID as ROOT};

    fn image() -> Vec<Partition> {
        let mut image = vec![0u8; 1 << 20];
        testing::write_gpt(
            &mut image,
            &[
                (2048, 2048, testing::BOOT_UUID),
                (4096, 12288, testing::ROOT_UUID),
            ],
        );
        ptable::parse(&image).unwrap()
    }

    /// `testing::device`, booted with `root=<root_arg>`.
    fn fake(dir: &Path, root_arg: &str) -> Probe {
        let probe = testing::device(dir);
        fs::write(&probe.cmdline, format!("root={root_arg} rootwait ro\n")).unwrap();
        probe
    }

    #[test]
    fn root_by_partuuid() {
        let dir = tempfile::tempdir().unwrap();
        let probe = fake(dir.path(), &format!("PARTUUID={}", ROOT.to_uppercase()));
        let layout = super::probe(&probe).unwrap();
        assert_eq!(layout.root.name, "sda2");
        assert_eq!(layout.root.partuuid, ROOT);
        assert_eq!(layout.root.start, 4096 * 512);
        assert_eq!(layout.boot.name, "sda1");
        assert_eq!(layout.boot.partuuid, BOOT);
        check(&layout, &image()).unwrap();
    }

    #[test]
    fn root_by_device_node() {
        let dir = tempfile::tempdir().unwrap();
        let probe = fake(dir.path(), "/dev/sda2");
        let layout = super::probe(&probe).unwrap();
        assert_eq!(layout.root.partuuid, ROOT);
    }

    #[test]
    fn another_disk_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let probe = fake(dir.path(), "/dev/sda2");
        let mut layout = super::probe(&probe).unwrap();
        layout.root.partuuid = "09b3d676-0000-0000-0000-000000000000".to_string();
        let err = check(&layout, &image()).unwrap_err();
        assert!(err.contains("reflash"), "{err}");
    }

    #[test]
    fn a_resized_root_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let probe = fake(dir.path(), "/dev/sda2");
        let mut layout = super::probe(&probe).unwrap();
        layout.root.size /= 2;
        assert!(check(&layout, &image())
            .unwrap_err()
            .contains("root partition"));
    }

    #[test]
    fn an_mbr_image_is_matched_by_geometry() {
        let part = |number, start, size, partuuid: &str| Partition {
            number,
            start,
            size,
            partuuid: partuuid.to_string(),
        };
        let image = vec![
            part(1, 4 << 20, 100 << 20, "aaaa0001-01"),
            part(2, 104 << 20, 2048 << 20, "aaaa0001-02"),
        ];
        let device = |root_start| Layout {
            boot: Part {
                name: "mmcblk0p1".to_string(),
                partuuid: "bbbb0002-01".to_string(),
                start: 4 << 20,
                size: 100 << 20,
            },
            root: Part {
                name: "mmcblk0p2".to_string(),
                partuuid: "bbbb0002-02".to_string(),
                start: root_start,
                size: 2048 << 20,
            },
        };
        // Another build's disk signature, the same partitions.
        check(&device(104 << 20), &image).unwrap();
        assert!(check(&device(200 << 20), &image).is_err());
    }
}
