//! In-place image updates, without A/B partitions.
//!
//! An update is the same `.wic.bz2` and `.wic.bmap` that `mise run
//! image:flash` writes to a whole disk, applied to one partition instead:
//!
//! 1. **Prepare** (`prepare`, in tessaro-agent, while the kiosk runs): the
//!    compressed image is streamed once. Its partition table must describe
//!    this device's own disk (`layout`). Of the ranges the bmap lists, only
//!    the parts inside the boot and root partitions are kept, each checked
//!    against the bmap's SHA-256, into sparse staging files on `/data`, and
//!    re-cut into chunks with checksums of their own (`manifest`). The kernel
//!    is copied out of the staged boot partition.
//! 2. **Apply** (`apply`, in tessaro-flash, from the initramfs, before the
//!    root filesystem is mounted): every staged chunk is verified before a
//!    single byte reaches the disk, then written, then read back. The kernel
//!    file on the ESP is swapped by rename.
//!
//! A power cut in step 2 leaves the marker and the staging on `/data`, so the
//! next boot simply writes everything again - the half-written root is never
//! mounted. Staging that fails verification is refused with the disk
//! untouched and the old system boots.

pub mod apply;
pub mod bmap;
pub mod flash;
pub mod layout;
pub mod manifest;
pub mod prepare;
pub mod ptable;
pub mod wipe;

pub mod fsutil;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

/// Every Tessaro wks has the ESP (or the Pi's vfat boot partition) first
/// and the root filesystem second. The layout check is what makes relying on
/// that safe: the numbers are only used on an image whose PARTUUIDs already
/// matched this device's.
pub const BOOT_PARTITION: u32 = 1;
pub const ROOT_PARTITION: u32 = 2;

/// Largest piece of the root partition checked and written as one unit.
pub const CHUNK: u64 = 4 << 20;

/// File names inside the staging directory, `/data/tessaro/update`.
pub const UPLOAD: &str = "upload.part";
pub const UPLOAD_META: &str = "upload.json";
pub const ROOT_IMAGE: &str = "root.img";
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
pub const STAGING: &[&str] = &[
    UPLOAD,
    UPLOAD_META,
    ROOT_IMAGE,
    BOOT_IMAGE,
    KERNEL,
    MANIFEST,
    PENDING,
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
