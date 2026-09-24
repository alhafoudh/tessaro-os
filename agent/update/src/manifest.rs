//! What sits in the staging directory, as JSON. The agent writes all of it
//! except `result.json`, which is tessaro-flash's answer back.

use serde::{Deserialize, Serialize};

/// 2: the upload itself is what gets applied, no longer a staged root image.
pub const FORMAT: u32 = 2;

/// A prepared update: everything the initramfs needs, and nothing it has to
/// work out. Its presence means the dry run passed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Bumped on any change an older tessaro-flash would misread. The one
    /// applying an update is the *old* image's, inside the running kernel.
    pub format: u32,
    pub source: Source,
    /// The upload as the agent read it, which the initramfs checks again
    /// before it writes anything. The same as `source.sha256` unless the
    /// upload was sent with `--no-verify`.
    pub upload: Digest,
    pub mode: Mode,
    pub target: Target,
    /// The kernel to install on the ESP. A root update only: a disk update
    /// writes the whole ESP.
    pub kernel: Option<File>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// The root partition, then the kernel file. `/data` is kept, unless
    /// the commit asks for it to be wiped.
    Root,
    /// The whole disk: partition table, every partition, `/data` included.
    /// For a device on another disk layout; a power cut while it writes
    /// needs a physical reflash.
    Disk,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// The file name the technician uploaded.
    pub name: String,
    /// SHA-256 of the compressed file, as the client sent it.
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Digest {
    pub size: u64,
    pub sha256: String,
}

/// What is written where: the root partition, or the whole disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// Bytes from the start of the image: the root partition's offset, or 0.
    pub start: u64,
    pub size: u64,
    /// The image's root PARTUUID, for the log.
    pub partuuid: String,
    /// The mapped parts of the target, at target-relative offsets, in order:
    /// where they go on the device written to, and what they must hash to.
    pub chunks: Vec<Chunk>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    pub offset: u64,
    pub len: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct File {
    /// Its name on the ESP, and in the staging directory as `kernel`.
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

impl Target {
    pub fn mapped(&self) -> u64 {
        self.chunks.iter().map(|chunk| chunk.len).sum()
    }
}

/// The marker: apply at the next boot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    /// Re-create `/data` afterwards: settings, the claim, the browser
    /// profile and the `/etc` overlay all go.
    #[serde(default)]
    pub wipe_data: bool,
    /// Boots that tried to apply it. Counted before any work is done.
    #[serde(default)]
    pub attempts: u32,
    /// Set just before the first byte goes to the root partition. From then
    /// on the old root is gone, and staging that no longer verifies is not a
    /// reason to boot it.
    #[serde(default)]
    pub started: bool,
}

/// What the last apply did. No timestamps: device clocks drift, and the
/// journal already stamps the line that reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub applied: bool,
    pub message: String,
    /// The uploaded file's name.
    pub source: String,
    #[serde(default)]
    pub wiped_data: bool,
    #[serde(default)]
    pub attempts: u32,
    /// Set by the boot oneshot once it has put this in the journal, so it
    /// is logged by the boot that follows the apply and not by every one.
    #[serde(default)]
    pub reported: bool,
}

/// Written by the agent while an upload is under way, so a resumed upload
/// can be recognised as the same file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Upload {
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub bmap: String,
    /// Check the whole file against `sha256` once it is in, before
    /// preparing. Off, only the bmap's per-range checksums guard it - which
    /// still cover every block that will be written.
    #[serde(default = "yes")]
    pub verify: bool,
    /// Write the whole disk, partition table included (`Mode::Disk`).
    #[serde(default)]
    pub repartition: bool,
}

impl Upload {
    pub fn mode(&self) -> Mode {
        if self.repartition {
            Mode::Disk
        } else {
            Mode::Root
        }
    }
}

fn yes() -> bool {
    true
}
