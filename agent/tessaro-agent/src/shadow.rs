//! The root password, edited in `/etc/shadow` directly.
//!
//! The image has busybox `passwd` and no `chpasswd`, and busybox `passwd`
//! only talks to a terminal. So this rewrites root's second field itself:
//! an exclusive lock on `/etc/.pwd.lock`, the whole file copied to a
//! temporary with root's hash replaced and every other line untouched, the
//! original's mode and owner carried over, `fsync`, `rename`, `fsync` of the
//! directory. `lastchg` and the ageing fields are left exactly as they are -
//! they are dates, and nothing here trusts the clock.
//!
//! `/etc` is an overlay whose upper layer is on `/data`, so the change
//! persists - and, like every file there, the first write shadows the image's
//! copy for good. That is acceptable here: the image's value is "empty", and
//! the claim model owns this field from the first boot on.
//!
//! Hashing is the system's own `crypt(3)` (libxcrypt) with a SHA-512 salt,
//! the `$6$` format busybox login and dropbear both verify.

use std::ffi::{c_char, CStr, CString};
use std::fs::{self, OpenOptions};
use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::sync::Mutex;

use crate::store;

#[link(name = "crypt")]
extern "C" {
    fn crypt(phrase: *const c_char, setting: *const c_char) -> *mut c_char;
}

/// crypt(3) returns a pointer into static storage.
static CRYPT: Mutex<()> = Mutex::new(());

pub fn hash(password: &str) -> io::Result<String> {
    const SALT_CHARS: &[u8] = b"./0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let bytes = crate::auth::random(16).map_err(io::Error::other)?;
    let salt: String = bytes
        .iter()
        .map(|byte| SALT_CHARS[*byte as usize % SALT_CHARS.len()] as char)
        .collect();
    crypt_with(password, &format!("$6${salt}$"))
}

fn crypt_with(password: &str, setting: &str) -> io::Result<String> {
    let phrase = CString::new(password).map_err(io::Error::other)?;
    let setting = CString::new(setting).map_err(io::Error::other)?;

    let _guard = crate::sync::lock(&CRYPT);
    // SAFETY: both arguments are valid NUL-terminated strings for the whole
    // call; the result is copied out before the lock is released.
    let result = unsafe { crypt(phrase.as_ptr(), setting.as_ptr()) };
    if result.is_null() {
        return Err(io::Error::other("crypt(3) failed"));
    }
    // SAFETY: crypt returned a non-null pointer to a NUL-terminated string.
    let hashed = unsafe { CStr::from_ptr(result) }
        .to_string_lossy()
        .into_owned();

    // libxcrypt signals failure with a string starting with '*'.
    if hashed.starts_with('*') || !hashed.starts_with("$6$") {
        return Err(io::Error::other("crypt(3) refused the SHA-512 setting"));
    }
    Ok(hashed)
}

/// Does `password` match `hashed`? Used by the tests and by nothing else.
#[cfg(test)]
pub fn verify(password: &str, hashed: &str) -> bool {
    crypt_with(password, hashed).is_ok_and(|again| again == hashed)
}

/// Root's password field is not empty.
pub fn root_has_password(path: &Path) -> io::Result<bool> {
    let text = fs::read_to_string(path)?;
    let field = root_field(&text)
        .ok_or_else(|| io::Error::other(format!("{} has no root entry", path.display())))?;
    Ok(!field.is_empty())
}

/// Set root's hash; `None` empties it - the unclaimed state.
pub fn set_root(path: &Path, hashed: Option<&str>) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("the shadow file has no directory"))?;

    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(dir.join(".pwd.lock"))?;
    lock.lock()?;

    let text = fs::read_to_string(path)?;
    let rewritten = replace_root_field(&text, hashed.unwrap_or(""))
        .ok_or_else(|| io::Error::other(format!("{} has no root entry", path.display())))?;

    let metadata = fs::metadata(path)?;
    store::replace(
        path,
        rewritten.as_bytes(),
        metadata.mode() & 0o7777,
        Some((metadata.uid(), metadata.gid())),
    )
}

fn root_field(text: &str) -> Option<&str> {
    text.lines()
        .find(|line| line.starts_with("root:"))
        .and_then(|line| line.split(':').nth(1))
}

fn replace_root_field(text: &str, hashed: &str) -> Option<String> {
    let mut found = false;
    let mut out = String::with_capacity(text.len() + hashed.len());

    for line in text.split_inclusive('\n') {
        let (body, newline) = match line.strip_suffix('\n') {
            Some(body) => (body, "\n"),
            None => (line, ""),
        };
        if !found && body.starts_with("root:") {
            let mut fields: Vec<&str> = body.split(':').collect();
            if fields.len() < 2 {
                return None;
            }
            fields[1] = hashed;
            out.push_str(&fields.join(":"));
            out.push_str(newline);
            found = true;
        } else {
            out.push_str(line);
        }
    }

    found.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const SHADOW: &str =
        "root::19000:0:99999:7:::\ndaemon:*:19000:0:99999:7:::\nweston:!:19000::::::\n";

    #[test]
    fn a_hash_verifies_and_is_salted() {
        let first = hash("correct horse").unwrap();
        let second = hash("correct horse").unwrap();

        assert!(first.starts_with("$6$"));
        assert_ne!(first, second);
        assert!(verify("correct horse", &first));
        assert!(!verify("wrong horse", &first));
    }

    #[test]
    fn only_roots_field_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shadow");
        fs::write(&path, SHADOW).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();

        assert!(!root_has_password(&path).unwrap());

        let hashed = hash("pw").unwrap();
        set_root(&path, Some(&hashed)).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(
            text,
            SHADOW.replacen("root::", &format!("root:{hashed}:"), 1)
        );
        assert!(root_has_password(&path).unwrap());
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o400
        );

        set_root(&path, None).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), SHADOW);
        assert!(!root_has_password(&path).unwrap());
    }

    #[test]
    fn a_file_without_root_is_refused_and_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shadow");
        fs::write(&path, "daemon:*:1:::::\n").unwrap();

        assert!(set_root(&path, Some("$6$x$y")).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "daemon:*:1:::::\n");
    }

    #[test]
    fn a_last_line_without_a_newline_survives() {
        assert_eq!(replace_root_field("root:x:1", "h").unwrap(), "root:h:1");
    }
}
