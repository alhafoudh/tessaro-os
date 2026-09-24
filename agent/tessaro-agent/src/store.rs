//! A JSON file that survives the power being cut at any moment.
//!
//! `state.json` and `auth.json` both live here. A kiosk loses power as a
//! matter of routine, so a write must leave either the old file or the new
//! one on disk, never half of each, and the writers - the agent, the boot
//! oneshot, a factory reset - must never interleave.
//!
//! * **Locking** is `flock` on a separate `<name>.lock`. Not on the data file
//!   itself: the rename below swaps its inode, so a lock held on the old one
//!   would protect nothing. Readers take it shared, writers exclusive, and a
//!   read-modify-write (`update`) holds it exclusively throughout.
//! * **Writing** is: the new content into `<name>.tmp`, `fsync`; the current
//!   file hard-linked to `<name>.prev`; `rename` over the current file;
//!   `fsync` the directory, which is what makes the link and the rename
//!   themselves durable.
//! * **Reading** tries `<name>`, then `<name>.prev`, then gives up and returns
//!   the default. Each fallback is logged, and none of them is an error: a
//!   torn file must never stop the kiosk from coming up.
//!
//! Everything here is blocking file I/O. From the runtime, call it through
//! `spawn_blocking` - see `control::blocking`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::log::Log;

#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
    name: &'static str,
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>, name: &'static str) -> Self {
        Self {
            dir: dir.into(),
            name,
        }
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join(self.name)
    }

    fn sibling(&self, suffix: &str) -> PathBuf {
        self.dir.join(format!("{}{suffix}", self.name))
    }

    fn lock(&self, exclusive: bool) -> io::Result<File> {
        fs::create_dir_all(&self.dir)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(self.sibling(".lock"))?;
        if exclusive {
            lock.lock()?;
        } else {
            lock.lock_shared()?;
        }
        Ok(lock)
    }

    pub fn read<T: DeserializeOwned + Default>(&self, log: &Log) -> T {
        match self.lock(false) {
            Ok(_lock) => self.read_locked(log),
            Err(err) => {
                log.info(format!(
                    "{}: cannot lock ({err}); reading unlocked",
                    self.name
                ));
                self.read_locked(log)
            }
        }
    }

    fn read_locked<T: DeserializeOwned + Default>(&self, log: &Log) -> T {
        let current = self.path();
        let previous = self.sibling(".prev");

        match parse::<T>(&current) {
            Parsed::Ok(value) => return value,
            Parsed::Missing => {}
            Parsed::Broken(why) => log.info(format!(
                "{} is unreadable ({why}); trying {}",
                current.display(),
                previous.display()
            )),
        }

        match parse::<T>(&previous) {
            Parsed::Ok(value) => {
                if current.exists() {
                    log.info(format!("using {}", previous.display()));
                }
                value
            }
            Parsed::Missing => T::default(),
            Parsed::Broken(why) => {
                log.info(format!(
                    "{} is unreadable too ({why}); starting from defaults",
                    previous.display()
                ));
                T::default()
            }
        }
    }

    /// Read, change, write, under one exclusive lock. `change` returning an
    /// error leaves the file untouched.
    pub fn update<T, R>(
        &self,
        log: &Log,
        change: impl FnOnce(&mut T) -> Result<R, String>,
    ) -> Result<R, String>
    where
        T: DeserializeOwned + Serialize + Default,
    {
        let _lock = self
            .lock(true)
            .map_err(|err| format!("cannot lock {}: {err}", self.name))?;
        let mut value: T = self.read_locked(log);
        let outcome = change(&mut value)?;
        self.write_locked(&value)
            .map_err(|err| format!("cannot write {}: {err}", self.path().display()))?;
        Ok(outcome)
    }

    /// Delete the file and its previous generation - a factory reset.
    pub fn remove(&self) -> io::Result<()> {
        let _lock = self.lock(true)?;
        for path in [self.path(), self.sibling(".prev"), self.sibling(".tmp")] {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
        }
        sync_dir(&self.dir)
    }

    fn write_locked<T: Serialize>(&self, value: &T) -> io::Result<()> {
        let mut body = serde_json::to_vec_pretty(value)?;
        body.push(b'\n');

        let current = self.path();
        let previous = self.sibling(".prev");
        let temporary = self.sibling(".tmp");

        write_synced(&temporary, &body, 0o600)?;

        if current.exists() {
            match fs::remove_file(&previous) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
            fs::hard_link(&current, &previous)?;
        }

        fs::rename(&temporary, &current)?;
        sync_dir(&self.dir)
    }
}

enum Parsed<T> {
    Ok(T),
    Missing,
    Broken(String),
}

fn parse<T: DeserializeOwned>(path: &Path) -> Parsed<T> {
    match fs::read(path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(value) => Parsed::Ok(value),
            Err(err) => Parsed::Broken(err.to_string()),
        },
        Err(err) if err.kind() == io::ErrorKind::NotFound => Parsed::Missing,
        Err(err) => Parsed::Broken(err.to_string()),
    }
}

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

    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no parent directory"))?;
    fs::create_dir_all(dir)?;

    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let temporary = dir.join(format!(".{name}.tessaro-tmp"));
    write_synced(&temporary, body, mode)?;
    fs::rename(&temporary, path)?;
    sync_dir(dir)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
    struct Doc {
        n: u32,
    }

    fn fixture() -> (tempfile::TempDir, Store, Log) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path(), "doc.json");
        (dir, store, Log::buffered(true))
    }

    #[test]
    fn a_missing_file_reads_as_the_default() {
        let (_dir, store, log) = fixture();
        assert_eq!(store.read::<Doc>(&log), Doc::default());
        assert!(log.lines().is_empty());
    }

    #[test]
    fn update_writes_and_keeps_the_previous_generation() {
        let (dir, store, log) = fixture();

        store
            .update(&log, |doc: &mut Doc| {
                doc.n = 1;
                Ok(())
            })
            .unwrap();
        store
            .update(&log, |doc: &mut Doc| {
                doc.n = 2;
                Ok(())
            })
            .unwrap();

        assert_eq!(store.read::<Doc>(&log), Doc { n: 2 });
        let previous: Doc =
            serde_json::from_slice(&fs::read(dir.path().join("doc.json.prev")).unwrap()).unwrap();
        assert_eq!(previous, Doc { n: 1 });
        assert!(!dir.path().join("doc.json.tmp").exists());
    }

    #[test]
    fn a_failed_change_writes_nothing() {
        let (_dir, store, log) = fixture();
        store
            .update(&log, |doc: &mut Doc| {
                doc.n = 1;
                Ok(())
            })
            .unwrap();

        let outcome = store.update(&log, |doc: &mut Doc| -> Result<(), String> {
            doc.n = 9;
            Err("no".to_string())
        });

        assert_eq!(outcome, Err("no".to_string()));
        assert_eq!(store.read::<Doc>(&log), Doc { n: 1 });
    }

    #[test]
    fn a_torn_file_falls_back_to_the_previous_generation() {
        let (dir, store, log) = fixture();
        store
            .update(&log, |doc: &mut Doc| {
                doc.n = 1;
                Ok(())
            })
            .unwrap();
        store
            .update(&log, |doc: &mut Doc| {
                doc.n = 2;
                Ok(())
            })
            .unwrap();

        // What a power cut in the middle of a non-atomic write would leave.
        fs::write(dir.path().join("doc.json"), b"{\"n\": ").unwrap();

        assert_eq!(store.read::<Doc>(&log), Doc { n: 1 });
        assert!(log.lines().iter().any(|line| line.contains("unreadable")));
    }

    #[test]
    fn both_generations_torn_is_the_default_and_loud() {
        let (dir, store, log) = fixture();
        fs::write(dir.path().join("doc.json"), b"garbage").unwrap();
        fs::write(dir.path().join("doc.json.prev"), b"garbage").unwrap();

        assert_eq!(store.read::<Doc>(&log), Doc::default());
        assert!(log
            .lines()
            .iter()
            .any(|line| line.contains("starting from defaults")));
    }

    #[test]
    fn an_update_on_a_torn_file_repairs_it_from_the_previous_generation() {
        let (dir, store, log) = fixture();
        store
            .update(&log, |doc: &mut Doc| {
                doc.n = 1;
                Ok(())
            })
            .unwrap();
        store
            .update(&log, |doc: &mut Doc| {
                doc.n = 2;
                Ok(())
            })
            .unwrap();
        fs::write(dir.path().join("doc.json"), b"").unwrap();

        store
            .update(&log, |doc: &mut Doc| {
                doc.n += 10;
                Ok(())
            })
            .unwrap();

        assert_eq!(store.read::<Doc>(&log), Doc { n: 11 });
    }

    #[test]
    fn remove_deletes_every_generation() {
        let (dir, store, log) = fixture();
        store
            .update(&log, |doc: &mut Doc| {
                doc.n = 1;
                Ok(())
            })
            .unwrap();
        store
            .update(&log, |doc: &mut Doc| {
                doc.n = 2;
                Ok(())
            })
            .unwrap();

        store.remove().unwrap();

        assert!(!dir.path().join("doc.json").exists());
        assert!(!dir.path().join("doc.json.prev").exists());
        assert_eq!(store.read::<Doc>(&log), Doc::default());
    }

    #[test]
    fn concurrent_updates_never_lose_a_write() {
        // Separate Store values, as separate processes would have: the lock
        // is on the file, not on anything in memory.
        let (dir, _store, _log) = fixture();
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let path = dir.path().to_path_buf();
                std::thread::spawn(move || {
                    let store = Store::new(path, "doc.json");
                    let log = Log::buffered(true);
                    for _ in 0..25 {
                        store
                            .update(&log, |doc: &mut Doc| {
                                doc.n += 1;
                                Ok(())
                            })
                            .unwrap();
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }

        let store = Store::new(dir.path(), "doc.json");
        assert_eq!(store.read::<Doc>(&Log::buffered(true)), Doc { n: 200 });
    }

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
