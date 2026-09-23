//! What sits in the staging directory, as JSON. The agent writes all of it
//! except `result.json`, which is tessaro-flash's answer back.

use serde::{Deserialize, Serialize};

pub const FORMAT: u32 = 1;

/// A prepared update: everything the initramfs needs, and nothing it has to
/// work out. Its presence means the staging is complete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Bumped on any change an older tessaro-flash would misread. The one
    /// applying an update is the *old* image's, inside the running kernel.
    pub format: u32,
    pub source: Source,
    pub root: Root,
    pub boot: Boot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// The file name the technician uploaded.
    pub name: String,
    /// SHA-256 of the compressed file.
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Root {
    pub partuuid: String,
    /// Bytes from the start of the disk.
    pub start: u64,
    pub size: u64,
    /// The mapped parts of the partition, at partition-relative offsets, in
    /// order. `root.img` holds them at the same offsets.
    pub chunks: Vec<Chunk>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    pub offset: u64,
    pub len: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Boot {
    pub partuuid: String,
    pub kernel: File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct File {
    /// Its name on the ESP, and in the staging directory as `kernel`.
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

impl Root {
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
}
