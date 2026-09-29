//! File storage: `/data/files`, filled by `tessaro-ctl files ...` and served
//! read-only by the local nginx at `http://127.0.0.1/files/`.
//!
//! A file arrives the way an image upload does (`updates.rs`): `files-begin`
//! describes it, `files-chunk`s append it to
//! `/data/tessaro/files-upload/upload.part`, each synced before it is
//! acknowledged, and once the last byte is in the file gets the client's
//! mtime and is renamed into place. So nginx never serves half a file, and
//! the staging is on the same filesystem as the store, which is what makes
//! that rename atomic. There is one upload slot: beginning another file
//! drops an unfinished one, and beginning the same file again - same path,
//! size and mtime - resumes it, across an agent restart too, since all of
//! it is on disk.
//!
//! Nothing here follows a symlink. The agent never makes one, so one found
//! in the store was put there by hand, and following it could reach outside
//! `/data/files` - to `/data/tessaro`, say, with the tokens in it.
//!
//! A factory reset empties the store; unclaiming keeps it, since it is the
//! site's content, not access to the device.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use protocol::files::{self, FileBegun, FileData, FileEntry, FileKind, FilesListing};
use protocol::{Done, Received};
use serde::{Deserialize, Serialize};
use update::{fsutil, megabytes};

use crate::deadline::blocking;
use crate::log::Log;
use crate::paths::Paths;

/// Left free on `/data` whatever is uploaded: the Chromium profile, the
/// settings and an image update all live there too.
const RESERVE: u64 = 256 << 20;

const META: &str = "upload.json";
const PART: &str = "upload.part";
/// What the agent itself stores is written here, in `state_dir`, beside the
/// upload slot rather than in it, so it never drops an upload under way.
const STORE_PART: &str = "files-store.part";

/// The file being received.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Upload {
    path: String,
    size: u64,
    mtime: i64,
}

pub struct Files {
    paths: Paths,
    log: Arc<Log>,
    /// One change at a time: two connections must not append to the same
    /// upload, or delete what the other is putting in place.
    writes: tokio::sync::Mutex<()>,
    /// Bumped whenever a file lands, moves or goes, for whoever keeps a copy
    /// of one: the page bridge's injected script.
    changed: tokio::sync::watch::Sender<u64>,
}

impl Files {
    pub fn new(log: Arc<Log>, paths: Paths) -> Arc<Self> {
        Arc::new(Self {
            paths,
            log,
            writes: tokio::sync::Mutex::new(()),
            changed: tokio::sync::watch::channel(0).0,
        })
    }

    /// Changes whenever the store's content does.
    pub fn changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changed.subscribe()
    }

    fn touched(&self) {
        self.changed.send_modify(|count| *count += 1);
    }

    /// A whole stored file, up to `max` bytes; a bigger one is refused.
    pub async fn read_whole(&self, path: &str, max: u64) -> Result<Vec<u8>, String> {
        let path = files::normalize(path)?;
        let root = self.paths.files_dir.clone();
        blocking("reading a stored file", move || {
            read_whole(&root, &path, max)
        })
        .await
    }

    pub async fn list(&self, path: &str, recursive: bool) -> Result<FilesListing, String> {
        let path = files::normalize(path)?;
        let root = self.paths.files_dir.clone();
        blocking("listing the stored files", move || {
            list(&root, &path, recursive)
        })
        .await
    }

    pub async fn begin(
        &self,
        caller: &str,
        path: &str,
        size: u64,
        mtime: i64,
    ) -> Result<FileBegun, String> {
        let path = files::normalize(path)?;
        if path.is_empty() {
            return Err("a file needs a name".to_string());
        }
        let _writes = self.writes.lock().await;
        let root = self.paths.files_dir.clone();
        let staging = self.paths.files_upload_dir();
        let upload = Upload {
            path: path.clone(),
            size,
            mtime,
        };
        let begun = blocking("starting a file upload", move || {
            begin(&root, &staging, &upload)
        })
        .await?;
        match begun {
            Begun::Stored => {
                self.touched();
                self.log.info(format!(
                    "files: stored {path} ({}) from {caller}",
                    megabytes(size)
                ))
            }
            Begun::Unchanged => {
                self.log
                    .debug(format!("files: {path} is already there, unchanged"));
            }
            Begun::Resumed(offset) => self.log.info(format!(
                "files: {caller} resumes {path} at {} of {}",
                megabytes(offset),
                megabytes(size)
            )),
            Begun::Started => self.log.debug(format!(
                "files: receiving {path} ({}) from {caller}",
                megabytes(size)
            )),
        }
        Ok(FileBegun {
            offset: begun.offset(size),
            size,
        })
    }

    pub async fn chunk(
        &self,
        caller: &str,
        path: &str,
        offset: u64,
        data: String,
    ) -> Result<Received, String> {
        let path = files::normalize(path)?;
        let bytes = protocol::decode_chunk(&data)?;
        let _writes = self.writes.lock().await;
        let root = self.paths.files_dir.clone();
        let staging = self.paths.files_upload_dir();
        let name = path.clone();
        let (received, size) = blocking("writing a file upload", move || {
            chunk(&root, &staging, &name, offset, &bytes)
        })
        .await?;
        if received == size {
            self.touched();
            self.log.info(format!(
                "files: stored {path} ({}) from {caller}",
                megabytes(size)
            ));
        }
        Ok(Received { received, size })
    }

    /// A file the agent made itself - the `audio test --input` recording -
    /// put in place whole, over whatever had its name, with the time now.
    pub async fn store(&self, caller: &str, path: &str, bytes: Vec<u8>) -> Result<(), String> {
        let path = files::normalize(path)?;
        if path.is_empty() {
            return Err("a file needs a name".to_string());
        }
        let _writes = self.writes.lock().await;
        let root = self.paths.files_dir.clone();
        let part = self.paths.state_dir.join(STORE_PART);
        let name = path.clone();
        let size = bytes.len() as u64;
        blocking("storing a file", move || store(&root, &part, &name, &bytes)).await?;
        self.touched();
        self.log.info(format!(
            "files: stored {path} ({}) for {caller}",
            megabytes(size)
        ));
        Ok(())
    }

    pub async fn read(&self, path: &str, offset: u64, len: u64) -> Result<FileData, String> {
        let path = files::normalize(path)?;
        let root = self.paths.files_dir.clone();
        let len = len.min(protocol::UPDATE_CHUNK as u64);
        blocking("reading a stored file", move || {
            read(&root, &path, offset, len)
        })
        .await
    }

    pub async fn mkdir(&self, caller: &str, path: &str) -> Result<Done, String> {
        let path = files::normalize(path)?;
        let _writes = self.writes.lock().await;
        let root = self.paths.files_dir.clone();
        let dir = path.clone();
        let made = blocking("making a directory", move || mkdir(&root, &dir)).await?;
        if made {
            self.log.info(format!("files: made {path}/ for {caller}"));
        }
        Ok(Done::new(format!("{}/", display(&path))))
    }

    /// `move` is a keyword, hence the name.
    pub async fn rename(&self, caller: &str, from: &str, to: &str) -> Result<Done, String> {
        let from = files::normalize(from)?;
        let to = files::normalize(to)?;
        if from.is_empty() {
            return Err("the store itself cannot be moved; name what is in it".to_string());
        }
        let _writes = self.writes.lock().await;
        let root = self.paths.files_dir.clone();
        let (source, target) = (from.clone(), to.clone());
        let landed = blocking("moving stored files", move || {
            rename(&root, &source, &target)
        })
        .await?;
        self.touched();
        self.log
            .info(format!("files: moved {from} to {landed} for {caller}"));
        Ok(Done::new(format!("moved {from} to {landed}")))
    }

    pub async fn delete(
        &self,
        caller: &str,
        paths: Vec<String>,
        recursive: bool,
    ) -> Result<Done, String> {
        let paths = paths
            .iter()
            .map(|path| files::normalize(path))
            .collect::<Result<Vec<_>, _>>()?;
        if paths.is_empty() {
            return Err("nothing to remove".to_string());
        }
        if paths.iter().any(String::is_empty) {
            return Err("the store itself cannot be removed; name what is in it".to_string());
        }
        let _writes = self.writes.lock().await;
        let root = self.paths.files_dir.clone();
        let names = paths.clone();
        blocking("removing stored files", move || {
            delete(&root, &names, recursive)
        })
        .await?;
        self.touched();
        self.log
            .info(format!("files: removed {} for {caller}", paths.join(", ")));
        Ok(Done::new(format!("removed {}", paths.join(", "))))
    }
}

/// Empty the store: the factory reset. The store is renamed away and made
/// again first, so what nginx serves is gone at once whatever the deletion
/// then takes, and a deletion cut short - by a power cut, or the deadline on
/// the caller's `blocking` - is finished by `clean` at the next boot.
pub fn wipe(paths: &Paths) -> io::Result<()> {
    let trash = paths.files_trash_dir();
    remove_tree(&trash)?;
    match fs::rename(&paths.files_dir, &trash) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    make_dir(&paths.files_dir)?;
    remove_tree(&paths.files_upload_dir())?;
    remove_tree(&trash)
}

/// What a wipe or an upload left behind: at every boot.
pub fn clean(paths: &Paths) -> io::Result<()> {
    remove_tree(&paths.files_trash_dir())
}

enum Begun {
    /// Empty, so complete as soon as it was described.
    Stored,
    /// The store already has it, same size and mtime.
    Unchanged,
    Resumed(u64),
    Started,
}

impl Begun {
    fn offset(&self, size: u64) -> u64 {
        match self {
            Begun::Stored | Begun::Unchanged => size,
            Begun::Resumed(offset) => *offset,
            Begun::Started => 0,
        }
    }
}

fn begin(root: &Path, staging: &Path, upload: &Upload) -> Result<Begun, String> {
    make_dir(root).map_err(|err| format!("{}: {err}", root.display()))?;
    let target = resolve(root, &upload.path)?;
    match fs::symlink_metadata(&target) {
        Ok(meta) if meta.is_dir() => {
            return Err(format!("{} is a directory", upload.path));
        }
        Ok(meta) if meta.len() == upload.size && meta.mtime() == upload.mtime => {
            return Ok(Begun::Unchanged);
        }
        _ => {}
    }

    let meta_path = staging.join(META);
    let part = staging.join(PART);
    if fsutil::read_json::<Upload>(&meta_path).is_ok_and(|known| known == *upload) {
        if let Ok(meta) = fs::metadata(&part) {
            if meta.len() <= upload.size {
                return Ok(Begun::Resumed(meta.len()));
            }
        }
    }

    remove_tree(staging).map_err(|err| format!("{}: {err}", staging.display()))?;
    make_dir(staging).map_err(|err| format!("{}: {err}", staging.display()))?;
    let free = fsutil::available(root).map_err(|err| format!("{}: {err}", root.display()))?;
    if free < upload.size.saturating_add(RESERVE) {
        return Err(format!(
            "/data has {} free; {} ({}) would leave less than the {} the kiosk keeps",
            megabytes(free),
            upload.path,
            megabytes(upload.size),
            megabytes(RESERVE)
        ));
    }
    fsutil::write_json(&meta_path, upload)
        .map_err(|err| format!("writing the upload's description: {err}"))?;
    File::create(&part).map_err(|err| format!("creating the upload: {err}"))?;
    if upload.size == 0 {
        finish(root, staging, upload)?;
        return Ok(Begun::Stored);
    }
    Ok(Begun::Started)
}

/// Append `bytes` at `offset`; the upload's received bytes and size.
fn chunk(
    root: &Path,
    staging: &Path,
    path: &str,
    offset: u64,
    bytes: &[u8],
) -> Result<(u64, u64), String> {
    let upload = fsutil::read_json::<Upload>(&staging.join(META))
        .map_err(|_| "no file upload is under way; `files-begin` starts one".to_string())?;
    if upload.path != path {
        return Err(format!(
            "the upload under way is {}, not {path}; begin again",
            upload.path
        ));
    }
    let part = staging.join(PART);
    let mut file = OpenOptions::new()
        .append(true)
        .open(&part)
        .map_err(|err| format!("{}: {err}", part.display()))?;
    let length = file.metadata().map_err(|err| err.to_string())?.len();
    if length != offset {
        return Err(format!(
            "the device has {length} bytes of {path}, not {offset}; begin again to resume"
        ));
    }
    let now = length + bytes.len() as u64;
    if now > upload.size {
        return Err("the chunk runs past the end of the file".to_string());
    }
    // Synced per chunk, so a power cut costs at most the one in flight.
    file.write_all(bytes)
        .and_then(|()| file.sync_data())
        .map_err(|err| format!("writing {path}: {err}"))?;
    drop(file);
    if now == upload.size {
        finish(root, staging, &upload)?;
    }
    Ok((now, upload.size))
}

/// The complete upload into place: its mtime, 0644 so nginx can read it,
/// renamed over whatever had its name.
fn finish(root: &Path, staging: &Path, upload: &Upload) -> Result<(), String> {
    let part = staging.join(PART);
    let target = resolve(root, &upload.path)?;
    let parent = target.parent().unwrap_or(root).to_path_buf();
    let fail = |err: io::Error| format!("storing {}: {err}", upload.path);

    let file = OpenOptions::new().write(true).open(&part).map_err(fail)?;
    file.set_permissions(fs::Permissions::from_mode(0o644))
        .map_err(fail)?;
    file.set_modified(time_of(upload.mtime)).map_err(fail)?;
    file.sync_all().map_err(fail)?;
    drop(file);

    make_dir(&parent).map_err(fail)?;
    if fs::symlink_metadata(&target).is_ok_and(|meta| meta.is_dir()) {
        return Err(format!("{} is a directory", upload.path));
    }
    fs::rename(&part, &target).map_err(fail)?;
    fsutil::sync_dir(&parent).map_err(fail)?;
    fsutil::remove_if_exists(&staging.join(META)).map_err(fail)?;
    Ok(())
}

/// Written and synced beside the store, then renamed in, as an upload is.
fn store(root: &Path, part: &Path, path: &str, bytes: &[u8]) -> Result<(), String> {
    make_dir(root).map_err(|err| format!("{}: {err}", root.display()))?;
    let target = resolve(root, path)?;
    if fs::symlink_metadata(&target).is_ok_and(|meta| meta.is_dir()) {
        return Err(format!("{path} is a directory"));
    }
    let free = fsutil::available(root).map_err(|err| format!("{}: {err}", root.display()))?;
    if free < (bytes.len() as u64).saturating_add(RESERVE) {
        return Err(format!(
            "/data has {} free, less than the {} the kiosk keeps",
            megabytes(free),
            megabytes(RESERVE)
        ));
    }
    let parent = target.parent().unwrap_or(root).to_path_buf();
    let fail = |err: io::Error| format!("storing {path}: {err}");

    if let Some(dir) = part.parent() {
        fs::create_dir_all(dir).map_err(fail)?;
    }
    let mut file = File::create(part).map_err(fail)?;
    file.set_permissions(fs::Permissions::from_mode(0o644))
        .map_err(fail)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(fail)?;
    drop(file);

    make_dir(&parent).map_err(fail)?;
    fs::rename(part, &target).map_err(fail)?;
    fsutil::sync_dir(&parent).map_err(fail)
}

fn list(root: &Path, path: &str, recursive: bool) -> Result<FilesListing, String> {
    make_dir(root).map_err(|err| format!("{}: {err}", root.display()))?;
    let target = resolve(root, path)?;
    let meta = fs::symlink_metadata(&target).map_err(|err| missing(path, err))?;
    let mut entries = Vec::new();
    if meta.is_dir() {
        walk(&target, path, recursive, &mut entries)
            .map_err(|err| format!("{}: {err}", display(path)))?;
    } else if meta.is_file() {
        entries.push(entry(path, &meta));
    }
    Ok(FilesListing { entries })
}

/// What is in `dir`, names sorted; with `recursive`, everything under it,
/// depth first, a directory before what is in it. Symlinks and anything else
/// that is neither a file nor a directory are left out.
fn walk(dir: &Path, prefix: &str, recursive: bool, entries: &mut Vec<FileEntry>) -> io::Result<()> {
    let mut names = fs::read_dir(dir)?
        .map(|item| item.map(|item| item.file_name()))
        .collect::<io::Result<Vec<_>>>()?;
    names.sort();
    for name in names {
        let Some(name) = name.to_str() else { continue };
        let full = dir.join(name);
        let meta = fs::symlink_metadata(&full)?;
        let path = files::join(prefix, name);
        if meta.is_dir() {
            entries.push(entry(&path, &meta));
            if recursive {
                walk(&full, &path, recursive, entries)?;
            }
        } else if meta.is_file() {
            entries.push(entry(&path, &meta));
        }
    }
    Ok(())
}

fn entry(path: &str, meta: &fs::Metadata) -> FileEntry {
    let dir = meta.is_dir();
    FileEntry {
        path: path.to_string(),
        kind: if dir { FileKind::Dir } else { FileKind::File },
        size: if dir { 0 } else { meta.len() },
        mtime: meta.mtime(),
    }
}

/// A whole file, refused when it is bigger than `max`.
fn read_whole(root: &Path, path: &str, max: u64) -> Result<Vec<u8>, String> {
    let target = resolve(root, path)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&target)
        .map_err(|err| missing(path, err))?;
    let meta = file.metadata().map_err(|err| err.to_string())?;
    if !meta.is_file() {
        return Err(format!("{} is not a file", display(path)));
    }
    if meta.len() > max {
        return Err(format!(
            "{} is {}, more than the {} allowed",
            display(path),
            megabytes(meta.len()),
            megabytes(max)
        ));
    }
    let mut data = Vec::new();
    file.take(max + 1)
        .read_to_end(&mut data)
        .map_err(|err| format!("reading {path}: {err}"))?;
    if data.len() as u64 > max {
        return Err(format!("{} grew while it was read", display(path)));
    }
    Ok(data)
}

fn read(root: &Path, path: &str, offset: u64, len: u64) -> Result<FileData, String> {
    let target = resolve(root, path)?;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&target)
        .map_err(|err| missing(path, err))?;
    let meta = file.metadata().map_err(|err| err.to_string())?;
    if !meta.is_file() {
        return Err(format!("{} is not a file", display(path)));
    }
    let mut data = Vec::new();
    file.seek(SeekFrom::Start(offset))
        .and_then(|_| file.take(len).read_to_end(&mut data))
        .map_err(|err| format!("reading {path}: {err}"))?;
    Ok(FileData {
        data: if data.is_empty() {
            String::new()
        } else {
            openssl::base64::encode_block(&data)
        },
        size: meta.len(),
        mtime: meta.mtime(),
    })
}

/// Whether it had to be made.
fn mkdir(root: &Path, path: &str) -> Result<bool, String> {
    make_dir(root).map_err(|err| format!("{}: {err}", root.display()))?;
    let target = resolve(root, path)?;
    match fs::symlink_metadata(&target) {
        Ok(meta) if meta.is_dir() => Ok(false),
        Ok(_) => Err(format!("{path} is a file")),
        Err(_) => {
            make_dir(&target).map_err(|err| format!("{path}: {err}"))?;
            Ok(true)
        }
    }
}

/// `from` to `to`, or into `to` when that is a directory; where it landed.
/// A rename within the store, so atomic: a page never sees it half moved.
fn rename(root: &Path, from: &str, to: &str) -> Result<String, String> {
    let source = resolve(root, from)?;
    let meta = fs::symlink_metadata(&source).map_err(|err| missing(from, err))?;
    let mut to = to.to_string();
    if fs::symlink_metadata(resolve(root, &to)?).is_ok_and(|meta| meta.is_dir()) {
        let name = from.rsplit('/').next().unwrap_or(from);
        to = files::join(&to, name);
    }
    if to == from {
        return Ok(to);
    }
    if meta.is_dir() && to.starts_with(&format!("{from}/")) {
        return Err(format!("{from} cannot be moved into itself"));
    }
    let target = resolve(root, &to)?;
    match fs::symlink_metadata(&target) {
        Ok(existing) if existing.is_dir() => {
            return Err(format!("{to} already exists and is a directory"));
        }
        Ok(_) if meta.is_dir() => {
            return Err(format!("{to} already exists and is a file"));
        }
        _ => {}
    }
    let parent = target.parent().unwrap_or(root).to_path_buf();
    let fail = |err: io::Error| format!("moving {from} to {to}: {err}");
    make_dir(&parent).map_err(fail)?;
    fs::rename(&source, &target).map_err(fail)?;
    fsutil::sync_dir(&parent).map_err(fail)?;
    if let Some(old) = source.parent() {
        fsutil::sync_dir(old).map_err(fail)?;
    }
    Ok(to)
}

/// Every path is checked before anything is removed, so a typo in the last
/// one does not leave the others half done.
fn delete(root: &Path, paths: &[String], recursive: bool) -> Result<(), String> {
    let mut targets = Vec::new();
    for path in paths {
        let target = resolve(root, path)?;
        let meta = fs::symlink_metadata(&target).map_err(|err| missing(path, err))?;
        if meta.is_dir() && !recursive {
            return Err(format!(
                "{path} is a directory; -r removes it with everything in it"
            ));
        }
        targets.push((path, target, meta.is_dir()));
    }
    for (path, target, dir) in targets {
        let removed = if dir {
            fs::remove_dir_all(&target)
        } else {
            fs::remove_file(&target)
        };
        match removed {
            Ok(()) => {}
            // Named twice, or inside a directory removed before it.
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(format!("removing {path}: {err}")),
        }
        if let Some(parent) = target.parent() {
            fsutil::sync_dir(parent).map_err(|err| format!("removing {path}: {err}"))?;
        }
    }
    Ok(())
}

/// `path` under `root`, refusing a symlink or a file anywhere above it. What
/// does not exist yet is fine: that is where an upload or a mkdir goes.
fn resolve(root: &Path, path: &str) -> Result<PathBuf, String> {
    let names = files::check_path(path)?;
    let mut full = root.to_path_buf();
    let last = names.len();
    for (index, name) in names.into_iter().enumerate() {
        full.push(name);
        match fs::symlink_metadata(&full) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(format!(
                    "{} is a symlink, which the store does not follow",
                    shown(root, &full)
                ))
            }
            Ok(meta) if index + 1 < last && !meta.is_dir() => {
                return Err(format!("{} is a file", shown(root, &full)))
            }
            Ok(_) => {}
            // Nothing below a missing name exists either.
            Err(err) if err.kind() == io::ErrorKind::NotFound => break,
            Err(err) => return Err(format!("{}: {err}", shown(root, &full))),
        }
    }
    Ok(root.join(path))
}

fn shown(root: &Path, full: &Path) -> String {
    full.strip_prefix(root)
        .map(|rest| rest.display().to_string())
        .unwrap_or_else(|_| full.display().to_string())
}

fn missing(path: &str, err: io::Error) -> String {
    if err.kind() == io::ErrorKind::NotFound {
        format!("{} does not exist", display(path))
    } else {
        format!("{}: {err}", display(path))
    }
}

/// The root reads as `/` in a message.
fn display(path: &str) -> &str {
    if path.is_empty() {
        "/"
    } else {
        path
    }
}

/// 0755, whatever the umask, so nginx can walk to every file.
fn make_dir(dir: &Path) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o755).create(dir)
}

fn remove_tree(dir: &Path) -> io::Result<()> {
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

fn time_of(mtime: i64) -> SystemTime {
    if mtime >= 0 {
        UNIX_EPOCH + Duration::from_secs(mtime as u64)
    } else {
        UNIX_EPOCH - Duration::from_secs(mtime.unsigned_abs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const MTIME: i64 = 1_700_000_000;

    struct Device {
        _dir: tempfile::TempDir,
        paths: Paths,
        log: Arc<Log>,
    }

    impl Device {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let at = |name: &str| dir.path().join(name).display().to_string();
            let env: HashMap<String, String> = [
                ("KIOSK_STATE_DIR", at("data/tessaro")),
                ("KIOSK_FILES_DIR", at("data/files")),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
            Device {
                paths: Paths::load(&env),
                log: Arc::new(Log::buffered(true)),
                _dir: dir,
            }
        }

        fn files(&self) -> Arc<Files> {
            Files::new(Arc::clone(&self.log), self.paths.clone())
        }

        fn stored(&self, path: &str) -> PathBuf {
            self.paths.files_dir.join(path)
        }
    }

    fn b64(bytes: &[u8]) -> String {
        openssl::base64::encode_block(bytes)
    }

    async fn upload(files: &Files, path: &str, body: &[u8], mtime: i64) {
        let begun = files
            .begin("test", path, body.len() as u64, mtime)
            .await
            .unwrap();
        let mut offset = begun.offset;
        while offset < body.len() as u64 {
            let end = (offset as usize + 3).min(body.len());
            let received = files
                .chunk("test", path, offset, b64(&body[offset as usize..end]))
                .await
                .unwrap();
            offset = received.received;
        }
    }

    #[tokio::test]
    async fn a_file_is_stored_with_its_mtime_only_once_complete() {
        let device = Device::new();
        let files = device.files();

        let begun = files
            .begin("test", "menu/today.json", 5, MTIME)
            .await
            .unwrap();
        assert_eq!(begun.offset, 0);
        files
            .chunk("test", "menu/today.json", 0, b64(b"{\"a\""))
            .await
            .unwrap();
        // Half a file is never where nginx serves from.
        assert!(!device.stored("menu/today.json").exists());
        let received = files
            .chunk("test", "menu/today.json", 4, b64(b"}"))
            .await
            .unwrap();
        assert_eq!((received.received, received.size), (5, 5));

        let stored = device.stored("menu/today.json");
        assert_eq!(fs::read(&stored).unwrap(), b"{\"a\"}");
        let meta = fs::metadata(&stored).unwrap();
        assert_eq!(meta.mtime(), MTIME);
        assert_eq!(meta.permissions().mode() & 0o777, 0o644);
        let dir = fs::metadata(device.stored("menu")).unwrap();
        assert_eq!(dir.permissions().mode() & 0o777, 0o755);
        assert!(!device.paths.files_upload_dir().join(META).exists());
    }

    #[tokio::test]
    async fn the_same_file_again_is_not_sent_again() {
        let device = Device::new();
        let files = device.files();
        upload(&files, "a.txt", b"hello", MTIME).await;

        let begun = files.begin("test", "a.txt", 5, MTIME).await.unwrap();
        assert_eq!(begun.offset, 5);
        // A different mtime is a different file.
        let begun = files.begin("test", "a.txt", 5, MTIME + 1).await.unwrap();
        assert_eq!(begun.offset, 0);
    }

    #[tokio::test]
    async fn an_interrupted_upload_resumes_after_a_restart() {
        let device = Device::new();
        let files = device.files();
        files.begin("test", "v.mp4", 6, MTIME).await.unwrap();
        files.chunk("test", "v.mp4", 0, b64(b"abc")).await.unwrap();
        drop(files);

        let files = device.files();
        let begun = files.begin("test", "v.mp4", 6, MTIME).await.unwrap();
        assert_eq!(begun.offset, 3);
        assert!(files.chunk("test", "v.mp4", 0, b64(b"abc")).await.is_err());
        files.chunk("test", "v.mp4", 3, b64(b"def")).await.unwrap();
        assert_eq!(fs::read(device.stored("v.mp4")).unwrap(), b"abcdef");
    }

    #[tokio::test]
    async fn another_file_replaces_an_unfinished_upload() {
        let device = Device::new();
        let files = device.files();
        files.begin("test", "a", 6, MTIME).await.unwrap();
        files.chunk("test", "a", 0, b64(b"abc")).await.unwrap();

        files.begin("test", "b", 2, MTIME).await.unwrap();
        let err = files.chunk("test", "a", 3, b64(b"def")).await.unwrap_err();
        assert!(err.contains("the upload under way is b"), "{err}");
        assert_eq!(files.begin("test", "a", 6, MTIME).await.unwrap().offset, 0);
    }

    #[tokio::test]
    async fn an_empty_file_is_stored_at_once() {
        let device = Device::new();
        let files = device.files();
        let begun = files.begin("test", "empty", 0, MTIME).await.unwrap();
        assert_eq!(begun.offset, 0);
        assert_eq!(fs::read(device.stored("empty")).unwrap(), b"");
        assert_eq!(fs::metadata(device.stored("empty")).unwrap().mtime(), MTIME);
    }

    #[tokio::test]
    async fn a_listing_is_one_level_like_ls_or_the_whole_tree() {
        let device = Device::new();
        let files = device.files();
        upload(&files, "b.txt", b"bb", MTIME).await;
        upload(&files, "a/x.txt", b"xxx", MTIME + 5).await;
        files.mkdir("test", "a/empty").await.unwrap();
        let paths = |listing: &FilesListing| {
            listing
                .entries
                .iter()
                .map(|e| e.path.clone())
                .collect::<Vec<_>>()
        };

        let top = files.list("", false).await.unwrap();
        assert_eq!(paths(&top), vec!["a", "b.txt"]);
        assert_eq!(top.entries[0].kind, FileKind::Dir);
        assert_eq!(
            paths(&files.list("/a", false).await.unwrap()),
            vec!["a/empty", "a/x.txt"]
        );

        let tree = files.list("", true).await.unwrap();
        assert_eq!(paths(&tree), vec!["a", "a/empty", "a/x.txt", "b.txt"]);
        let x = &tree.entries[2];
        assert_eq!((x.kind, x.size, x.mtime), (FileKind::File, 3, MTIME + 5));

        let one = files.list("/a/x.txt", false).await.unwrap();
        assert_eq!(paths(&one), vec!["a/x.txt"]);
        assert!(files
            .list("nope", false)
            .await
            .unwrap_err()
            .contains("does not exist"));
        // An empty store lists nothing rather than failing.
        let fresh = Device::new();
        assert!(fresh
            .files()
            .list("", false)
            .await
            .unwrap()
            .entries
            .is_empty());
    }

    #[tokio::test]
    async fn a_file_reads_back_in_pieces() {
        let device = Device::new();
        let files = device.files();
        upload(&files, "a.bin", b"0123456789", MTIME).await;

        let data = files.read("a.bin", 4, 3).await.unwrap();
        assert_eq!(openssl::base64::decode_block(&data.data).unwrap(), b"456");
        assert_eq!((data.size, data.mtime), (10, MTIME));
        assert_eq!(files.read("a.bin", 10, 3).await.unwrap().data, "");
        assert!(files.read("", 0, 3).await.is_err());
    }

    #[tokio::test]
    async fn symlinks_are_never_followed() {
        let device = Device::new();
        let files = device.files();
        upload(&files, "a.txt", b"a", MTIME).await;
        fs::create_dir_all(device.paths.state_dir.join("secret")).unwrap();
        fs::write(device.paths.state_dir.join("tessaro.db"), "tokens").unwrap();
        std::os::unix::fs::symlink(&device.paths.state_dir, device.stored("out")).unwrap();
        std::os::unix::fs::symlink(
            device.paths.state_dir.join("tessaro.db"),
            device.stored("auth"),
        )
        .unwrap();

        assert!(files.read("auth", 0, 10).await.is_err());
        assert!(files.read("out/tessaro.db", 0, 10).await.is_err());
        assert!(files.begin("test", "out/x", 1, MTIME).await.is_err());
        assert!(files.mkdir("test", "out/x").await.is_err());
        assert!(files.list("out", false).await.is_err());
        // Listed as if they were not there.
        let listing = files.list("", true).await.unwrap();
        assert_eq!(listing.entries.len(), 1);
    }

    #[tokio::test]
    async fn paths_that_climb_out_are_refused() {
        let device = Device::new();
        let files = device.files();
        assert!(files.begin("test", "../tessaro/x", 1, MTIME).await.is_err());
        assert!(files.read("a/../../x", 0, 1).await.is_err());
        assert!(files.begin("test", "", 1, MTIME).await.is_err());
    }

    #[tokio::test]
    async fn delete_needs_recursive_for_a_directory_and_checks_everything_first() {
        let device = Device::new();
        let files = device.files();
        upload(&files, "a/x.txt", b"x", MTIME).await;
        upload(&files, "b.txt", b"b", MTIME).await;

        let err = files
            .delete("test", vec!["b.txt".into(), "a".into()], false)
            .await
            .unwrap_err();
        assert!(err.contains("-r"), "{err}");
        assert!(device.stored("b.txt").exists());
        assert!(files
            .delete("test", vec!["b.txt".into(), "missing".into()], false)
            .await
            .is_err());
        assert!(device.stored("b.txt").exists());
        assert!(files.delete("test", vec!["".into()], true).await.is_err());

        files
            .delete("test", vec!["a".into(), "b.txt".into()], true)
            .await
            .unwrap();
        assert!(files.list("", true).await.unwrap().entries.is_empty());
    }

    #[tokio::test]
    async fn move_renames_or_moves_into_a_directory_like_mv() {
        let device = Device::new();
        let files = device.files();
        upload(&files, "a.txt", b"a", MTIME).await;
        upload(&files, "b.txt", b"b", MTIME).await;
        upload(&files, "media/x.mp4", b"x", MTIME).await;

        // To a new name, making the directory above it.
        let done = files.rename("test", "a.txt", "docs/one.txt").await.unwrap();
        assert_eq!(done.message, "moved a.txt to docs/one.txt");
        assert_eq!(fs::read(device.stored("docs/one.txt")).unwrap(), b"a");
        assert_eq!(
            fs::metadata(device.stored("docs/one.txt")).unwrap().mtime(),
            MTIME
        );
        // Into a directory that exists, keeping the name.
        files.rename("test", "b.txt", "/media").await.unwrap();
        assert!(device.stored("media/b.txt").exists());
        // A whole directory, and back to the top.
        files.rename("test", "media", "site/media").await.unwrap();
        assert!(device.stored("site/media/x.mp4").exists());
        files.rename("test", "site/media/x.mp4", "/").await.unwrap();
        assert!(device.stored("x.mp4").exists());

        // A file replaces a file, as mv does.
        files.rename("test", "x.mp4", "docs/one.txt").await.unwrap();
        assert_eq!(fs::read(device.stored("docs/one.txt")).unwrap(), b"x");

        assert!(files.rename("test", "site", "site/deeper").await.is_err());
        assert!(files.rename("test", "missing", "x").await.is_err());
        assert!(files.rename("test", "", "x").await.is_err());
        assert!(files
            .rename("test", "docs/one.txt", "../out")
            .await
            .is_err());
        upload(&files, "c.txt", b"c", MTIME).await;
        let err = files.rename("test", "site", "c.txt").await.unwrap_err();
        assert!(err.contains("is a file"), "{err}");
    }

    #[tokio::test]
    async fn a_file_cannot_take_a_directorys_place_or_live_under_a_file() {
        let device = Device::new();
        let files = device.files();
        upload(&files, "a/x.txt", b"x", MTIME).await;
        assert!(files.begin("test", "a", 1, MTIME).await.is_err());
        assert!(files.begin("test", "a/x.txt/y", 1, MTIME).await.is_err());
        assert!(files.mkdir("test", "a/x.txt").await.is_err());
    }

    #[tokio::test]
    async fn a_file_the_agent_stores_replaces_the_last_and_leaves_an_upload_alone() {
        let device = Device::new();
        let files = device.files();
        let begun = files.begin("test", "big.bin", 4, MTIME).await.unwrap();
        files
            .chunk("test", "big.bin", begun.offset, b64(b"ab"))
            .await
            .unwrap();

        files
            .store("test", "rec.wav", b"first".to_vec())
            .await
            .unwrap();
        files
            .store("test", "rec.wav", b"second".to_vec())
            .await
            .unwrap();
        assert_eq!(fs::read(device.stored("rec.wav")).unwrap(), b"second");
        let mode = fs::metadata(device.stored("rec.wav")).unwrap().mode();
        assert_eq!(mode & 0o777, 0o644);

        let received = files.chunk("test", "big.bin", 2, b64(b"cd")).await.unwrap();
        assert_eq!(received.received, 4);
        assert_eq!(fs::read(device.stored("big.bin")).unwrap(), b"abcd");
    }

    #[tokio::test]
    async fn the_space_reserve_is_kept() {
        let device = Device::new();
        let files = device.files();
        let err = files
            .begin("test", "huge", u64::MAX / 2, MTIME)
            .await
            .unwrap_err();
        assert!(err.contains("free"), "{err}");
    }

    #[test]
    fn a_wipe_empties_the_store_and_leaves_it_there() {
        let device = Device::new();
        fs::create_dir_all(device.stored("a/b")).unwrap();
        fs::write(device.stored("a/b/c"), "c").unwrap();
        fs::create_dir_all(device.paths.files_upload_dir()).unwrap();
        fs::write(device.paths.files_upload_dir().join(PART), "half").unwrap();

        wipe(&device.paths).unwrap();

        assert!(device.paths.files_dir.is_dir());
        assert_eq!(fs::read_dir(&device.paths.files_dir).unwrap().count(), 0);
        assert!(!device.paths.files_upload_dir().exists());
        assert!(!device.paths.files_trash_dir().exists());

        // A wipe cut short leaves the trash; the next boot finishes it.
        fs::create_dir_all(device.paths.files_trash_dir().join("x")).unwrap();
        clean(&device.paths).unwrap();
        assert!(!device.paths.files_trash_dir().exists());
    }
}
