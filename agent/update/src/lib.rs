//! In-place image updates, without A/B partitions.
//!
//! An update is the same `.wic.bz2` and `.wic.bmap` that `mise run
//! image:flash` writes to a whole disk. It is kept on `/data` as it was
//! uploaded, compressed, and decompressed twice: once by the agent to check
//! it, once by the initramfs to write it. Either to the root partition
//! alone, or - `Mode::Disk`, `--repartition` - to the whole disk:
//!
//! 1. **Prepare** (`prepare`, in tessaro-agent, while the kiosk runs): a dry
//!    run. The image is streamed once, its partition table checked against
//!    this device's own disk (`layout`), and every range the bmap lists
//!    inside what will be written checked against the bmap's SHA-256 and
//!    re-cut into chunks with checksums of their own (`manifest`). Nothing of
//!    the image is kept but the kernel, which a root update copies out of the
//!    image's boot partition. The upload's own SHA-256 goes in the manifest.
//! 2. **Apply** (`apply`, in tessaro-flash, from the initramfs, before the
//!    root filesystem is mounted): the upload is checked against that
//!    SHA-256 before a single byte reaches the disk, then decompressed again,
//!    each chunk checked and written, and everything read back. A root update
//!    then swaps the kernel file on the ESP by rename; a disk update copies
//!    the upload into RAM first, since it overwrites the `/data` it sits on.
//!
//! A power cut during a root update leaves the marker and the upload on
//! `/data`, so the next boot simply writes everything again - the
//! half-written root is never mounted. A power cut during a disk update
//! leaves a device that needs a physical reflash: that is what
//! `--repartition` accepts. An upload that fails its check is refused with
//! the disk untouched and the old system boots.

pub mod apply;
pub mod bmap;
pub mod flash;
pub mod layout;
pub mod manifest;
pub mod prepare;
pub mod ptable;
pub mod wipe;

pub mod fsutil;
pub mod image;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

/// Every Tessaro wks has the ESP (or the Pi's vfat boot partition) first,
/// the root filesystem second and `/data` third. The layout check is what
/// makes relying on that safe: the numbers are only used on an image whose
/// partitions already matched this device's.
pub const BOOT_PARTITION: u32 = 1;
pub const ROOT_PARTITION: u32 = 2;
pub const DATA_PARTITION: u32 = 3;

/// Largest piece of the image checked and written as one unit.
pub const CHUNK: u64 = 4 << 20;

/// File names inside the staging directory, `/data/tessaro/update`.
pub const UPLOAD: &str = "upload.part";
pub const UPLOAD_META: &str = "upload.json";
/// The image's boot partition, sparse, only while the kernel is copied out.
pub const BOOT_IMAGE: &str = "boot.img";
pub const KERNEL: &str = "kernel";
pub const MANIFEST: &str = "manifest.json";
/// Present means: apply at the next boot.
pub const PENDING: &str = "pending";
/// What the last apply did. Survives until the next update starts.
pub const RESULT: &str = "result.json";

/// On the ESP, while `/data` is being re-created. It is the one place that
/// persists while `/data` does not exist, and it holds the result to write
/// into the new filesystem.
pub const WIPE_MARKER: &str = "tessaro-wipe";

/// Every file an update leaves in the staging directory, except the result.
/// `root.img` is what an older agent staged the root partition into; it goes
/// with the rest if a device ever still has one.
pub const STAGING: &[&str] = &[
    UPLOAD,
    UPLOAD_META,
    BOOT_IMAGE,
    KERNEL,
    MANIFEST,
    PENDING,
    "root.img",
];

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// Size and SHA-256 of a whole file.
pub fn sha256_file(path: &Path) -> io::Result<(u64, String)> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    let mut size = 0u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((size, hex(&hasher.finalize())))
}

/// Human-readable size, for progress lines and errors.
pub fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}
