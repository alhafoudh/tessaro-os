//! Root's `authorized_keys`, and the host keys a client should expect.
//!
//! The claim model owns this file the way it owns root's password: keys are
//! added by `tessaro-ctl ssh connect` (a token or the local socket), listed and
//! revoked the same way, and every one of them goes when the device is
//! unclaimed or reset. An unclaimed device has no credential at all.
//!
//! Every write replaces the file whole: a temporary next to it at 0600,
//! `fsync`, `rename`, `fsync` of the directory. Dropbear refuses a key file
//! or `.ssh` that is group or world writable without saying why, so both
//! modes are set explicitly rather than left to the umask. Lines this does
//! not understand - options, a type it does not know - are kept as they are
//! by `add` and `remove`; `clear` takes them too.
//!
//! Host keys are read with `dropbearkey -y`, the only thing on the image that
//! turns dropbear's own key format into the OpenSSH line a client pins.

use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use protocol::sshkey::{PublicKey, TYPES};

use crate::store;

/// The keys the file holds, in file order, each once.
pub fn list(path: &Path) -> io::Result<Vec<PublicKey>> {
    let mut keys: Vec<PublicKey> = Vec::new();
    for line in read_lines(path)? {
        if let Ok(key) = PublicKey::parse(&line) {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
    }
    Ok(keys)
}

/// Append `key` unless it is already there. Returns whether it was added.
pub fn add(path: &Path, key: &PublicKey) -> io::Result<bool> {
    let mut lines = read_lines(path)?;
    if lines
        .iter()
        .any(|line| PublicKey::parse(line).is_ok_and(|existing| existing == *key))
    {
        return Ok(false);
    }
    lines.push(key.line());
    write(path, &lines)?;
    Ok(true)
}

/// Remove the one key `query` names: its fingerprint (with or without the
/// `SHA256:` prefix), a unique prefix of that, or its exact comment.
pub fn remove(path: &Path, query: &str) -> Result<PublicKey, String> {
    let keys = list(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let key = select(&keys, query)?.clone();

    let lines = read_lines(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let kept: Vec<String> = lines
        .into_iter()
        .filter(|line| !PublicKey::parse(line).is_ok_and(|existing| existing == key))
        .collect();
    write(path, &kept).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(key)
}

/// Empty the file, if there is one. Never creates it.
pub fn clear(path: &Path) -> io::Result<bool> {
    match fs::metadata(path) {
        Ok(meta) if meta.len() == 0 => Ok(false),
        Ok(_) => {
            write(path, &[])?;
            Ok(true)
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

fn select<'a>(keys: &'a [PublicKey], query: &str) -> Result<&'a PublicKey, String> {
    let query = query.trim();
    if query.is_empty() {
        return Err("name a key: its fingerprint, a prefix of it, or its comment".to_string());
    }
    let bare = query.strip_prefix("SHA256:").unwrap_or(query);

    let exact: Vec<&PublicKey> = keys
        .iter()
        .filter(|key| key.fingerprint() == format!("SHA256:{bare}"))
        .collect();
    let by_comment: Vec<&PublicKey> = keys.iter().filter(|key| key.comment == query).collect();
    // A prefix shorter than this is more likely a typo than a choice.
    let by_prefix: Vec<&PublicKey> = if bare.len() >= 4 {
        keys.iter()
            .filter(|key| key.fingerprint()["SHA256:".len()..].starts_with(bare))
            .collect()
    } else {
        Vec::new()
    };

    for candidates in [exact, by_comment, by_prefix] {
        match candidates.as_slice() {
            [] => continue,
            [one] => return Ok(one),
            many => {
                let names: Vec<String> = many
                    .iter()
                    .map(|key| format!("{} {}", key.fingerprint(), key.comment))
                    .collect();
                return Err(format!(
                    "{query} matches {} keys; use the fingerprint:\n  {}",
                    many.len(),
                    names.join("\n  ")
                ));
            }
        }
    }
    Err(format!(
        "no key matches {query}; `tessaro-ctl ssh keys list` shows them"
    ))
}

fn read_lines(path: &Path) -> io::Result<Vec<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(str::to_string)
            .collect()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(err),
    }
}

fn write(path: &Path, lines: &[String]) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("authorized_keys has no directory"))?;
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    // An existing ~/.ssh keeps whatever mode it had; dropbear wants it tight.
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;

    let mut body = lines.join("\n");
    if !body.is_empty() {
        body.push('\n');
    }
    let temporary = dir.join(".authorized_keys.tessaro-tmp");
    store::write_synced(&temporary, body.as_bytes(), 0o600)?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    fs::rename(&temporary, path)?;
    store::sync_dir(dir)
}

/// Make dropbear's RSA host key if no directory has one yet, exactly as
/// `dropbearkey.service` would. That unit only runs on the first connection
/// (dropbear is socket-activated), so on a device nobody has logged in to yet
/// there is no key for `host_keys` to report - and the whole point is that
/// the first `tessaro-ctl ssh connect` already gets one.
pub fn ensure_host_key(dirs: &[PathBuf]) -> Result<(), String> {
    let Some(dir) = dirs.first() else {
        return Ok(());
    };
    if dirs.iter().any(|dir| !host_key_files(dir).is_empty()) {
        return Ok(());
    }
    DirBuilder::new()
        .recursive(true)
        .mode(0o755)
        .create(dir)
        .map_err(|err| format!("{}: {err}", dir.display()))?;
    let file = dir.join("dropbear_rsa_host_key");
    let status = Command::new("dropbearkey")
        .args(["-t", "rsa", "-f"])
        .arg(&file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|err| format!("dropbearkey: {err}"))?;
    // It refuses to overwrite: if dropbearkey.service got there first, the
    // key it made is just as good.
    if status.success() || file.exists() {
        Ok(())
    } else {
        Err(format!("dropbearkey could not make {}", file.display()))
    }
}

fn host_key_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = match fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with("dropbear_") && name.ends_with("_host_key")
                    })
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    files.sort();
    files
}

/// The device's host keys as `TYPE BASE64` lines, from the first directory
/// that has any. Empty when none can be read: the client then asks the user
/// the way ssh always does, which is worse but not wrong.
pub fn host_keys(dirs: &[PathBuf]) -> Vec<String> {
    for dir in dirs {
        let files = host_key_files(dir);
        let keys: Vec<String> = files
            .iter()
            .filter_map(|file| {
                let output = Command::new("dropbearkey")
                    .arg("-y")
                    .arg("-f")
                    .arg(file)
                    .stdin(Stdio::null())
                    .stderr(Stdio::null())
                    .output()
                    .ok()?;
                output
                    .status
                    .success()
                    .then(|| public_line(&String::from_utf8_lossy(&output.stdout)))
                    .flatten()
            })
            .collect();
        if !keys.is_empty() {
            return keys;
        }
    }
    Vec::new()
}

/// The `TYPE BASE64` part of `dropbearkey -y`'s output, which surrounds it
/// with "Public key portion is:" and a fingerprint line, and appends a
/// `user@host` comment a known_hosts entry has no use for.
fn public_line(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let mut fields = line.split_ascii_whitespace();
        let kind = fields.next()?;
        let data = fields.next()?;
        TYPES
            .contains(&kind)
            .then(|| PublicKey::parse(&format!("{kind} {data}")).ok())
            .flatten()
            .map(|key| key.line())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    const A: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIO7gEEtj0g4zaawVIwrP4wxLZQ2TqgASR86NTHDJ66jj a@laptop";
    const A_FINGERPRINT: &str = "SHA256:YY7C2uXwz+G0YAPdSZsL/SANSo5RStBfYENHBRMPE7A";

    fn other(comment: &str, byte: u8) -> PublicKey {
        let mut blob = 11u32.to_be_bytes().to_vec();
        blob.extend_from_slice(b"ssh-ed25519");
        blob.extend_from_slice(&32u32.to_be_bytes());
        blob.extend_from_slice(&[byte; 32]);
        PublicKey {
            kind: "ssh-ed25519".to_string(),
            blob,
            comment: comment.to_string(),
        }
    }

    fn file() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("root/.ssh/authorized_keys");
        (dir, path)
    }

    #[test]
    fn a_key_is_added_once_with_tight_modes() {
        let (_dir, path) = file();
        let key = PublicKey::parse(A).unwrap();

        assert!(add(&path, &key).unwrap());
        assert!(!add(&path, &key).unwrap());
        // The same key under another comment is still the same key.
        assert!(!add(
            &path,
            &PublicKey::parse(&A.replace("a@laptop", "b")).unwrap()
        )
        .unwrap());

        assert_eq!(fs::read_to_string(&path).unwrap(), format!("{A}\n"));
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        assert_eq!(
            fs::metadata(path.parent().unwrap()).unwrap().mode() & 0o777,
            0o700
        );
        assert_eq!(list(&path).unwrap(), vec![key]);
    }

    #[test]
    fn a_loose_ssh_dir_is_tightened() {
        let (_dir, path) = file();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o777)).unwrap();

        add(&path, &PublicKey::parse(A).unwrap()).unwrap();
        assert_eq!(
            fs::metadata(path.parent().unwrap()).unwrap().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn revoke_by_fingerprint_prefix_or_comment() {
        let (_dir, path) = file();
        add(&path, &PublicKey::parse(A).unwrap()).unwrap();
        add(&path, &other("phone", 7)).unwrap();
        add(&path, &other("tablet", 9)).unwrap();

        assert_eq!(remove(&path, A_FINGERPRINT).unwrap().comment, "a@laptop");
        assert_eq!(remove(&path, "phone").unwrap().comment, "phone");
        let tablet = other("tablet", 9).fingerprint();
        assert_eq!(
            remove(&path, &tablet["SHA256:".len()..][..8])
                .unwrap()
                .comment,
            "tablet"
        );
        assert!(list(&path).unwrap().is_empty());
        assert!(remove(&path, "phone")
            .unwrap_err()
            .contains("no key matches"));
    }

    #[test]
    fn an_ambiguous_name_removes_nothing() {
        let (_dir, path) = file();
        add(&path, &other("same", 1)).unwrap();
        add(&path, &other("same", 2)).unwrap();

        let err = remove(&path, "same").unwrap_err();
        assert!(err.contains("matches 2 keys"), "{err}");
        assert_eq!(list(&path).unwrap().len(), 2);
        assert!(remove(&path, "SHA").is_err(), "too short to be a prefix");
    }

    #[test]
    fn lines_it_does_not_understand_survive_add_and_remove() {
        let (_dir, path) = file();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let manual = format!("no-pty {A}");
        fs::write(&path, format!("{manual}\n")).unwrap();

        add(&path, &other("phone", 7)).unwrap();
        remove(&path, "phone").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), format!("{manual}\n"));
        // Not a key this manages, so not listed.
        assert!(list(&path).unwrap().is_empty());
    }

    #[test]
    fn clear_empties_and_never_creates() {
        let (_dir, path) = file();
        assert!(!clear(&path).unwrap());
        assert!(!path.exists());

        add(&path, &PublicKey::parse(A).unwrap()).unwrap();
        assert!(clear(&path).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "");
        assert!(!clear(&path).unwrap());
    }

    #[test]
    fn dropbearkeys_output_is_reduced_to_the_key() {
        let output = format!("Public key portion is:\n{A}\nFingerprint: sha1!! 00:11:22\n");
        assert_eq!(
            public_line(&output).unwrap(),
            A.trim_end_matches(" a@laptop")
        );
        assert_eq!(public_line("Public key portion is:\n"), None);
    }

    #[test]
    fn no_host_key_directory_means_no_host_keys() {
        let dir = tempfile::tempdir().unwrap();
        assert!(host_keys(&[dir.path().join("missing"), dir.path().to_path_buf()]).is_empty());
    }
}
