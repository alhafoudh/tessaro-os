//! Files that survive the power being cut at any moment: everything the
//! agent renders (`generated.env`, the policy, keyfiles, units) and the few
//! files it keeps that are not in `tessaro.db` (the TLS identity, ssh keys,
//! the extra certificate authorities).
//!
//! A kiosk loses power as a matter of routine, so a write must leave either
//! the old file or the new one on disk, never half of each: a temporary
//! beside it, `fsync`, `rename` over it, `fsync` the directory, which is
//! what makes the rename itself durable.
//!
//! Everything here is blocking file I/O. From the runtime, call it through
//! `spawn_blocking` - see `deadline::blocking`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

/// Create or truncate `path` with `mode`, write `body` and `fsync` it.
pub fn write_synced(path: &Path, body: &[u8], mode: u32) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(mode)
        .open(path)?;
    file.write_all(body)?;
    file.sync_all()
}

pub fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

/// Replace `path` with `body` atomically, unless it already holds exactly
/// that. Returns whether anything was written: an unchanged render must not
/// restart anything.
pub fn replace_if_changed(path: &Path, body: &[u8], mode: u32) -> io::Result<bool> {
    if fs::read(path).is_ok_and(|existing| existing == body) {
        return Ok(false);
    }
    fs::create_dir_all(parent(path)?)?;
    replace(path, body, mode, None)?;
    Ok(true)
}

/// `replace_if_changed` for a filesystem with no Unix permissions: the Pi's
/// vfat boot partition, where a chmod fails with EPERM. No mode is set, so
/// the file gets whatever the mount gives every file.
pub fn replace_if_changed_on_vfat(path: &Path, body: &[u8]) -> io::Result<bool> {
    if fs::read(path).is_ok_and(|existing| existing == body) {
        return Ok(false);
    }
    replace_as(path, body, None, None)?;
    Ok(true)
}

/// Replace `path` with `body` atomically, so nobody ever reads half of it: a
/// temporary beside it, written and `fsync`ed with exactly `mode` (and
/// `owner`, uid and gid, when given), renamed over it, the directory synced.
/// A temporary left by a failure is removed.
pub fn replace(path: &Path, body: &[u8], mode: u32, owner: Option<(u32, u32)>) -> io::Result<()> {
    replace_as(path, body, Some(mode), owner)
}

fn replace_as(
    path: &Path,
    body: &[u8],
    mode: Option<u32>,
    owner: Option<(u32, u32)>,
) -> io::Result<()> {
    let dir = parent(path)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let temporary = dir.join(format!(".{name}.tessaro-tmp"));
    let written = (|| {
        write_synced(&temporary, body, mode.unwrap_or(0o644))?;
        if let Some(mode) = mode {
            // The mode given to open() is filtered through the umask; say it again.
            fs::set_permissions(&temporary, fs::Permissions::from_mode(mode))?;
        }
        if let Some((uid, gid)) = owner {
            std::os::unix::fs::chown(&temporary, Some(uid), Some(gid)).or_else(|err| {
                // Not root (a development host): the owner is already ours.
                if err.kind() == io::ErrorKind::PermissionDenied {
                    Ok(())
                } else {
                    Err(err)
                }
            })?;
        }
        fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written?;
    sync_dir(dir)
}

fn parent(path: &Path) -> io::Result<&Path> {
    path.parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no parent directory"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_if_changed_only_writes_a_difference() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/out.env");

        assert!(replace_if_changed(&path, b"A=1\n", 0o644).unwrap());
        assert!(!replace_if_changed(&path, b"A=1\n", 0o644).unwrap());
        assert!(replace_if_changed(&path, b"A=2\n", 0o644).unwrap());
        assert_eq!(fs::read(&path).unwrap(), b"A=2\n");
    }
}
