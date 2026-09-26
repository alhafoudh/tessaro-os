//! File storage: `/data/files` on the device, served by the local nginx at
//! `http://127.0.0.1/files/`.
//!
//! A path on the wire is relative to that root, `/`-separated, whatever the
//! client's own separator is. The rules are here so the client refuses what
//! the device would, before anything is sent.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Longest path, in bytes, the whole of it.
pub const MAX_PATH: usize = 4096;

/// Longest single name, in bytes: ext4's own limit.
pub const MAX_NAME: usize = 255;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum FileKind {
    File,
    Dir,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileEntry {
    /// From the root of the store, `/`-separated.
    pub path: String,
    pub kind: FileKind,
    /// Bytes; 0 for a directory.
    pub size: u64,
    /// Modification time, whole seconds since the epoch. What `sync`
    /// compares, with the size.
    pub mtime: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FilesListing {
    /// Names sorted; recursively, depth first, a directory before what is
    /// in it. Paths are always from the store's root.
    pub entries: Vec<FileEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileBegun {
    /// Bytes the device already has of this file. Send from here.
    pub offset: u64,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileData {
    /// Base64, at most `UPDATE_CHUNK` bytes before encoding. Empty at the
    /// end of the file.
    pub data: String,
    /// The whole file's size and mtime, so the client knows when it is done
    /// and what to stamp its copy with.
    pub size: u64,
    pub mtime: i64,
}

/// The names in `path`, checked. Empty for the root, which is `""` (a lone
/// `/` or `.` is accepted as the root too, and a leading or trailing `/` is
/// ignored).
pub fn check_path(path: &str) -> Result<Vec<&str>, String> {
    if path.len() > MAX_PATH {
        let head: String = path.chars().take(32).collect();
        return Err(format!("{head}...: the path is too long"));
    }
    if path.chars().any(char::is_control) {
        return Err(format!("{path:?}: control characters are not allowed"));
    }
    if path.contains('\\') {
        return Err(format!("{path}: use / to separate names, not \\"));
    }
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() || trimmed == "." {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for name in trimmed.split('/') {
        match name {
            "" => return Err(format!("{path}: empty name between slashes")),
            "." | ".." => return Err(format!("{path}: . and .. are not allowed")),
            name if name.len() > MAX_NAME => {
                return Err(format!("{path}: a name is longer than {MAX_NAME} bytes"))
            }
            name => names.push(name),
        }
    }
    Ok(names)
}

/// `path` in the one spelling the device uses: no leading or trailing `/`,
/// `""` for the root.
pub fn normalize(path: &str) -> Result<String, String> {
    check_path(path).map(|names| names.join("/"))
}

/// `name` under `dir`, either of which may be the root.
pub fn join(dir: &str, name: &str) -> String {
    match (dir.is_empty(), name.is_empty()) {
        (true, _) => name.to_string(),
        (_, true) => dir.to_string(),
        _ => format!("{dir}/{name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_relative_and_plain() {
        assert_eq!(check_path("").unwrap(), Vec::<&str>::new());
        assert_eq!(check_path("/").unwrap(), Vec::<&str>::new());
        assert_eq!(check_path(".").unwrap(), Vec::<&str>::new());
        assert_eq!(check_path("a/b.mp4").unwrap(), vec!["a", "b.mp4"]);
        assert_eq!(check_path("/a/b/").unwrap(), vec!["a", "b"]);
        assert_eq!(check_path(".hidden").unwrap(), vec![".hidden"]);
        assert_eq!(normalize("/menu/today.json").unwrap(), "menu/today.json");

        assert!(check_path("a/../b").is_err());
        assert!(check_path("..").is_err());
        assert!(check_path("a/./b").is_err());
        assert!(check_path("a//b").is_err());
        assert!(check_path("a\\b").is_err());
        assert!(check_path("a\nb").is_err());
        assert!(check_path(&"x".repeat(MAX_NAME + 1)).is_err());
        assert!(check_path(&"x/".repeat(MAX_PATH)).is_err());
    }

    #[test]
    fn join_handles_the_root() {
        assert_eq!(join("", "a"), "a");
        assert_eq!(join("a", ""), "a");
        assert_eq!(join("a", "b"), "a/b");
    }
}
