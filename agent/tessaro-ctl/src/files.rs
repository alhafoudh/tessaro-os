//! `tessaro-ctl files ...`: the device's file store, `/data/files`, which
//! its local web server serves at `http://127.0.0.1/files/`.
//!
//! Files go up the way an image does: in `UPDATE_CHUNK` pieces, each
//! acknowledged before the next, and a file the device already has with the
//! same size and mtime is not sent again. `sync` is rsync `-r --delete`
//! compared on size and mtime alone: after it the store, or a directory in
//! it, holds exactly what the local directory does.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anstream::{eprintln, println};
use clap::Subcommand;
use protocol::files::{
    self as store, FileBegun, FileData, FileEntry, FileKind, FileReceived, FilesListing,
};
use protocol::{Command, Done};
use serde::Serialize;

use crate::connect::{self, Session};
use crate::style::{self, pad, paint};
use crate::update::{mb, percent, step_line, Progress, Rate};
use crate::{call, print};

#[derive(Subcommand)]
pub enum FilesCmd {
    /// What is in REMOTE (by default /), like `ls -l`: mtime (UTC), size,
    /// name, a directory with a trailing /.
    ///
    ///   tessaro-ctl files list /media
    #[command(visible_aliases = ["ls", "dir"])]
    List {
        remote: Option<String>,
        /// Everything under REMOTE, not just what is directly in it.
        #[arg(long, short = 'R')]
        recursive: bool,
    },
    /// Store LOCAL, a file or a directory with everything in it, as REMOTE.
    /// A file goes to REMOTE (a trailing / keeps its name), by default its
    /// own name at the top; a directory's contents go into REMOTE, by
    /// default a directory of its own name. Nothing on the device is
    /// removed. Files keep their mtime; one the device already has, same
    /// size and mtime, is skipped. Run it again after a dropped connection
    /// and it resumes.
    ///
    ///   tessaro-ctl files upload promo.mp4 media/
    Upload {
        local: PathBuf,
        remote: Option<String>,
    },
    /// Fetch REMOTE, a file or a directory with everything in it, to LOCAL:
    /// by default its own name here, and into LOCAL if that is a directory.
    /// Files keep their mtime.
    Download {
        remote: String,
        local: Option<PathBuf>,
    },
    /// Make REMOTE_DIR (by default the whole store) hold exactly what
    /// LOCAL_DIR does: new and changed files are sent, anything the local
    /// directory does not have is removed. A file counts as changed when its
    /// size or its mtime, to the second, differs; contents are not compared.
    /// Symlinks and special files here are skipped.
    ///
    ///   tessaro-ctl files sync ./site-assets
    Sync {
        local: PathBuf,
        remote: Option<String>,
        /// Print what would be sent and removed, and change nothing.
        #[arg(long)]
        dry_run: bool,
        /// Remove without asking.
        #[arg(long, short)]
        yes: bool,
    },
    /// Move or rename, like `mv`: SOURCE to DEST, or every SOURCE into DEST
    /// when DEST is a directory (a trailing / makes it one). A file at DEST
    /// is replaced. Missing directories above DEST are made.
    ///
    ///   tessaro-ctl files move /promo.mp4 /media/
    ///   tessaro-ctl files move /site /site-old
    #[command(visible_alias = "mv")]
    Move {
        /// SOURCE... DEST
        #[arg(required = true, num_args = 2.., value_name = "PATH")]
        paths: Vec<String>,
    },
    /// Remove files, or directories and everything in them with -r.
    Rm {
        #[arg(required = true)]
        remote: Vec<String>,
        #[arg(long, short)]
        recursive: bool,
        #[arg(long, short)]
        yes: bool,
    },
}

pub fn run(session: &mut Session, command: FilesCmd, json: bool) -> Result<(), String> {
    match command {
        FilesCmd::List { remote, recursive } => {
            let path = store::normalize(remote.as_deref().unwrap_or(""))?;
            let listing: FilesListing = call(
                session,
                Command::FilesList {
                    path: path.clone(),
                    recursive,
                },
            )?;
            print(json, &listing, || show_listing(&path, &listing))
        }
        FilesCmd::Upload { local, remote } => upload(session, &local, remote.as_deref(), json),
        FilesCmd::Download { remote, local } => download(session, &remote, local, json),
        FilesCmd::Sync {
            local,
            remote,
            dry_run,
            yes,
        } => sync(session, &local, remote.as_deref(), dry_run, yes, json),
        FilesCmd::Move { mut paths } => {
            let dest = paths.pop().expect("clap requires two paths");
            let into = dest.ends_with('/');
            let dest = store::normalize(&dest)?;
            if paths.len() > 1 && !into && !is_dir(session, &dest)? {
                return Err(format!(
                    "moving several paths needs DEST to be a directory; /{dest} is not one \
                     (a trailing / makes it one)"
                ));
            }
            let mut moved = Vec::new();
            for from in &paths {
                let from = store::normalize(from)?;
                // A trailing / means "into": name the file inside it, so a
                // directory that does not exist yet is made for it.
                let to = if into {
                    store::join(&dest, from.rsplit('/').next().unwrap_or(&from))
                } else {
                    dest.clone()
                };
                let done: Done = call(session, Command::FilesMove { from, to })?;
                if !json {
                    println!("{}", done.message);
                }
                moved.push(done);
            }
            if json {
                print(json, &moved, || {})?;
            }
            Ok(())
        }
        FilesCmd::Rm {
            remote,
            recursive,
            yes,
        } => {
            let paths = remote
                .iter()
                .map(|path| store::normalize(path))
                .collect::<Result<Vec<_>, _>>()?;
            if !yes
                && !connect::ask(&format!(
                    "Remove {} from {}?",
                    paths.join(", "),
                    session.node.name
                ))?
            {
                return Err("not confirmed".to_string());
            }
            let done: Done = call(session, Command::FilesDelete { paths, recursive })?;
            print(json, &done, || println!("{}", done.message))
        }
    }
}

/// Whether `path` is a directory in the store: its listing is not the file
/// itself. A missing path is an error, as it would be for `mv`.
fn is_dir(session: &mut Session, path: &str) -> Result<bool, String> {
    let listing: FilesListing = call(
        session,
        Command::FilesList {
            path: path.to_string(),
            recursive: false,
        },
    )?;
    Ok(!(listing.entries.len() == 1
        && listing.entries[0].path == path
        && listing.entries[0].kind == FileKind::File))
}

/// What a transfer did, for `--json` and the closing line.
#[derive(Debug, Default, Serialize)]
struct Report {
    sent: Vec<String>,
    received: Vec<String>,
    removed: Vec<String>,
    made: Vec<String>,
    unchanged: Vec<String>,
    skipped: Vec<String>,
    bytes: u64,
}

impl Report {
    fn show(&self, verb: &str) {
        let moved = self.sent.len() + self.received.len();
        let mut parts = vec![format!("{moved} {verb} ({})", mb(self.bytes))];
        if !self.removed.is_empty() {
            parts.push(format!("{} removed", self.removed.len()));
        }
        parts.push(format!("{} unchanged", self.unchanged.len()));
        if !self.skipped.is_empty() {
            parts.push(paint(
                style::WARN,
                format!("{} skipped", self.skipped.len()),
            ));
        }
        println!("{} {}", paint(style::OK, "done:"), parts.join(", "));
    }
}

/// A file or directory on this machine, by its path from the directory
/// being sent, in the device's spelling.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Local {
    path: String,
    kind: FileKind,
    size: u64,
    mtime: i64,
    full: PathBuf,
}

/// Everything under `dir`, depth first, names sorted, a directory before
/// what is in it. What cannot be sent - symlinks, sockets, names the device
/// would refuse - is left out, with a line saying so in `skipped`.
fn scan(
    dir: &Path,
    prefix: &str,
    out: &mut Vec<Local>,
    skipped: &mut Vec<String>,
) -> Result<(), String> {
    let mut items = fs::read_dir(dir)
        .map_err(|err| format!("{}: {err}", dir.display()))?
        .map(|item| item.map(|item| item.file_name()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("{}: {err}", dir.display()))?;
    items.sort();
    for name in items {
        let full = dir.join(&name);
        let Some(name) = name.to_str() else {
            skipped.push(format!("{}: not UTF-8", full.display()));
            continue;
        };
        let path = store::join(prefix, name);
        if let Err(err) = store::check_path(&path) {
            skipped.push(err);
            continue;
        }
        let meta =
            fs::symlink_metadata(&full).map_err(|err| format!("{}: {err}", full.display()))?;
        if meta.is_dir() {
            out.push(Local {
                path: path.clone(),
                kind: FileKind::Dir,
                size: 0,
                mtime: mtime_of(&meta),
                full: full.clone(),
            });
            scan(&full, &path, out, skipped)?;
        } else if meta.is_file() {
            out.push(Local {
                path,
                kind: FileKind::File,
                size: meta.len(),
                mtime: mtime_of(&meta),
                full,
            });
        } else {
            skipped.push(format!(
                "{}: {}",
                full.display(),
                if meta.file_type().is_symlink() {
                    "a symlink"
                } else {
                    "not a file or directory"
                }
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    /// Everything that goes, in one request: the top-most of it only.
    Remove(Vec<String>),
    Mkdir(String),
    Send(Local),
    Unchanged(String),
}

/// What makes `remote` (paths from the directory being synced) hold what
/// `local` does. Removals come first, so a file can take the place of a
/// directory and the other way round, and the space is free before new
/// files arrive.
fn plan(local: &[Local], remote: &[FileEntry]) -> Vec<Action> {
    let theirs: BTreeMap<&str, &FileEntry> = remote
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();
    let ours: BTreeMap<&str, &Local> = local
        .iter()
        .map(|item| (item.path.as_str(), item))
        .collect();

    let mut doomed: BTreeSet<&str> = BTreeSet::new();
    for entry in remote {
        match ours.get(entry.path.as_str()) {
            None => {
                doomed.insert(&entry.path);
            }
            Some(item) if item.kind != entry.kind => {
                doomed.insert(&entry.path);
            }
            Some(_) => {}
        }
    }
    let top: Vec<String> = doomed
        .iter()
        .filter(|path| !ancestors(path).any(|above| doomed.contains(above)))
        .map(|path| path.to_string())
        .collect();

    let mut actions = Vec::new();
    if !top.is_empty() {
        actions.push(Action::Remove(top));
    }
    for item in local {
        let kept = theirs
            .get(item.path.as_str())
            .filter(|_| !doomed.contains(item.path.as_str()))
            .filter(|_| !ancestors(&item.path).any(|above| doomed.contains(above)));
        match (item.kind, kept) {
            (FileKind::Dir, Some(_)) => {}
            (FileKind::Dir, None) => actions.push(Action::Mkdir(item.path.clone())),
            (FileKind::File, Some(entry))
                if entry.size == item.size && entry.mtime == item.mtime =>
            {
                actions.push(Action::Unchanged(item.path.clone()))
            }
            (FileKind::File, _) => actions.push(Action::Send(item.clone())),
        }
    }
    actions
}

/// `a/b/c` -> `a/b`, `a`.
fn ancestors(path: &str) -> impl Iterator<Item = &str> {
    path.match_indices('/').map(move |(at, _)| &path[..at])
}

fn sync(
    session: &mut Session,
    local: &Path,
    remote: Option<&str>,
    dry_run: bool,
    yes: bool,
    json: bool,
) -> Result<(), String> {
    let root = store::normalize(remote.unwrap_or(""))?;
    if !local.is_dir() {
        return Err(format!("{} is not a directory", local.display()));
    }
    let mut report = Report::default();
    let mut ours = Vec::new();
    scan(local, "", &mut ours, &mut report.skipped)?;
    for line in &report.skipped {
        eprintln!("{} {line}", paint(style::WARN, "skipped:"));
    }

    // A directory that is not there yet lists as empty; making it is the
    // first change, and a dry run makes nothing.
    let tree = Command::FilesList {
        path: root.clone(),
        recursive: true,
    };
    let listed: Result<FilesListing, String> = if dry_run || root.is_empty() {
        call(session, tree)
    } else {
        call::<Done>(session, Command::FilesMkdir { path: root.clone() })
            .and_then(|_| call(session, tree))
    };
    let theirs = match listed {
        Ok(listing) => relative(&root, listing.entries),
        Err(err) if dry_run && err.ends_with("does not exist") => Vec::new(),
        Err(err) => return Err(err),
    };

    let actions = plan(&ours, &theirs);
    let removals: Vec<String> = actions
        .iter()
        .find_map(|action| match action {
            Action::Remove(paths) => {
                Some(paths.iter().map(|path| store::join(&root, path)).collect())
            }
            _ => None,
        })
        .unwrap_or_default();

    if dry_run {
        for action in &actions {
            match action {
                Action::Remove(_) => {
                    for path in &removals {
                        println!("{}", step_line(style::WARN, "remove", path));
                    }
                }
                Action::Mkdir(path) => {
                    println!(
                        "{}",
                        step_line(style::LABEL, "mkdir", store::join(&root, path))
                    )
                }
                Action::Send(item) => println!(
                    "{}",
                    step_line(
                        style::LABEL,
                        "send",
                        format!(
                            "{}  {}",
                            store::join(&root, &item.path),
                            paint(style::MUTED, human(item.size))
                        )
                    )
                ),
                Action::Unchanged(_) => {}
            }
        }
        println!("{}", paint(style::MUTED, "(dry run: nothing changed)"));
        return Ok(());
    }

    if !removals.is_empty() && !yes {
        for path in &removals {
            eprintln!("{}", step_line(style::WARN, "remove", path));
        }
        if !connect::ask(&format!(
            "Remove what is listed above from {}?",
            session.node.name
        ))? {
            return Err("not confirmed; nothing was changed".to_string());
        }
    }

    let mut progress = Progress::new(json);
    for action in actions {
        match action {
            Action::Remove(_) => {
                call::<Done>(
                    session,
                    Command::FilesDelete {
                        paths: removals.clone(),
                        recursive: true,
                    },
                )?;
                for path in &removals {
                    progress.done(&step_line(style::WARN, "removed", path));
                }
                report.removed.extend(removals.iter().cloned());
            }
            Action::Mkdir(path) => {
                let path = store::join(&root, &path);
                call::<Done>(session, Command::FilesMkdir { path: path.clone() })?;
                report.made.push(path);
            }
            Action::Send(item) => {
                let path = store::join(&root, &item.path);
                send(session, &item, &path, &mut progress, &mut report)?;
            }
            Action::Unchanged(path) => report.unchanged.push(store::join(&root, &path)),
        }
    }
    print(json, &report, || report.show("sent"))
}

/// The listing's paths from `root` down, `root` itself left out.
fn relative(root: &str, entries: Vec<FileEntry>) -> Vec<FileEntry> {
    if root.is_empty() {
        return entries;
    }
    let prefix = format!("{root}/");
    entries
        .into_iter()
        .filter_map(|entry| {
            let path = entry.path.strip_prefix(&prefix)?.to_string();
            Some(FileEntry { path, ..entry })
        })
        .collect()
}

fn upload(
    session: &mut Session,
    local: &Path,
    remote: Option<&str>,
    json: bool,
) -> Result<(), String> {
    let meta = fs::metadata(local).map_err(|err| format!("{}: {err}", local.display()))?;
    let name = local
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("{}: no usable file name", local.display()))?
        .to_string();
    let mut report = Report::default();
    let mut progress = Progress::new(json);

    if meta.is_file() {
        let target = match remote {
            None => name.clone(),
            Some(remote) if remote.ends_with('/') => store::join(&store::normalize(remote)?, &name),
            Some(remote) => store::normalize(remote)?,
        };
        let item = Local {
            path: target.clone(),
            kind: FileKind::File,
            size: meta.len(),
            mtime: mtime_of(&meta),
            full: local.to_path_buf(),
        };
        send(session, &item, &target, &mut progress, &mut report)?;
        return print(json, &report, || report.show("sent"));
    }
    if !meta.is_dir() {
        return Err(format!("{} is not a file or a directory", local.display()));
    }

    let root = match remote {
        None => store::normalize(&name)?,
        Some(remote) => store::normalize(remote)?,
    };
    let mut ours = Vec::new();
    scan(local, "", &mut ours, &mut report.skipped)?;
    for line in &report.skipped {
        eprintln!("{} {line}", paint(style::WARN, "skipped:"));
    }
    if !root.is_empty() {
        call::<Done>(session, Command::FilesMkdir { path: root.clone() })?;
    }
    for item in &ours {
        let path = store::join(&root, &item.path);
        match item.kind {
            FileKind::Dir => {
                call::<Done>(session, Command::FilesMkdir { path: path.clone() })?;
                report.made.push(path);
            }
            FileKind::File => send(session, item, &path, &mut progress, &mut report)?,
        }
    }
    print(json, &report, || report.show("sent"))
}

/// One file to `path` on the device, from where the device says it has got
/// to: nowhere, part of it, or all of it already.
fn send(
    session: &mut Session,
    item: &Local,
    path: &str,
    progress: &mut Progress,
    report: &mut Report,
) -> Result<(), String> {
    let begun: FileBegun = call(
        session,
        Command::FilesBegin {
            path: path.to_string(),
            size: item.size,
            mtime: item.mtime,
        },
    )?;
    if begun.offset >= item.size && item.size > 0 {
        report.unchanged.push(path.to_string());
        return Ok(());
    }

    let mut file =
        File::open(&item.full).map_err(|err| format!("{}: {err}", item.full.display()))?;
    file.seek(SeekFrom::Start(begun.offset))
        .map_err(|err| format!("{}: {err}", item.full.display()))?;
    let mut rate = Rate::new(begun.offset);
    let mut buffer = vec![0u8; protocol::UPDATE_CHUNK];
    let mut offset = begun.offset;
    while offset < item.size {
        let want = ((item.size - offset) as usize).min(buffer.len());
        file.read_exact(&mut buffer[..want]).map_err(|err| {
            format!(
                "{}: {err} (did it change while it was sent?)",
                item.full.display()
            )
        })?;
        let data = data_encoding::BASE64.encode(&buffer[..want]);
        let received: FileReceived = call(
            session,
            Command::FilesChunk {
                path: path.to_string(),
                offset,
                data,
            },
        )
        .map_err(|err| {
            format!(
                "{path} stopped at {}: {err}; run the same command again to resume",
                mb(offset)
            )
        })?;
        offset = received.received;
        // Only a file of several chunks is worth a progress line of its own.
        if item.size > protocol::UPDATE_CHUNK as u64 {
            let (speed, eta) = rate.update(offset, item.size);
            progress.show(
                &step_line(
                    style::LABEL,
                    "sending",
                    format!(
                        "{path}  {}/{}  {:>3}%  {}/s  {} {eta}",
                        mb(offset),
                        mb(item.size),
                        percent(offset, item.size),
                        mb(speed as u64),
                        paint(style::LABEL, "ETA"),
                    ),
                ),
                offset,
                item.size,
            );
        }
    }
    progress.done(&step_line(
        style::OK,
        "sent",
        format!("{path}  {}", paint(style::MUTED, human(item.size))),
    ));
    report.bytes += item.size - begun.offset;
    report.sent.push(path.to_string());
    Ok(())
}

fn download(
    session: &mut Session,
    remote: &str,
    local: Option<PathBuf>,
    json: bool,
) -> Result<(), String> {
    let path = store::normalize(remote)?;
    let listing: FilesListing = call(
        session,
        Command::FilesList {
            path: path.clone(),
            recursive: true,
        },
    )?;
    let name = path
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("files");
    let mut report = Report::default();
    let mut progress = Progress::new(json);

    let single = listing.entries.len() == 1
        && listing.entries[0].path == path
        && listing.entries[0].kind == FileKind::File;
    if single {
        let target = match local {
            Some(local) if local.is_dir() => local.join(name),
            Some(local) => local,
            None => PathBuf::from(name),
        };
        fetch(
            session,
            &listing.entries[0],
            &target,
            &mut progress,
            &mut report,
        )?;
        return print(json, &report, || report.show("received"));
    }

    let base = match local {
        Some(local) if local.is_dir() && remote_named(&path) => local.join(name),
        Some(local) => local,
        None => PathBuf::from(name),
    };
    make_local_dir(&base)?;
    for entry in relative(&path, listing.entries) {
        let target = base.join(entry.path.split('/').collect::<PathBuf>());
        match entry.kind {
            FileKind::Dir => {
                make_local_dir(&target)?;
                report.made.push(target.display().to_string());
            }
            FileKind::File => {
                let entry = FileEntry {
                    path: store::join(&path, &entry.path),
                    ..entry
                };
                fetch(session, &entry, &target, &mut progress, &mut report)?;
            }
        }
    }
    print(json, &report, || report.show("received"))
}

/// A named directory, not the whole store: `download media .` makes
/// `./media`, while `download / .` fills `.` itself.
fn remote_named(path: &str) -> bool {
    !path.is_empty()
}

fn make_local_dir(dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))
}

/// One stored file to `target`, through a temporary file beside it, stamped
/// with the device's mtime. Skipped when `target` already has its size and
/// mtime.
fn fetch(
    session: &mut Session,
    entry: &FileEntry,
    target: &Path,
    progress: &mut Progress,
    report: &mut Report,
) -> Result<(), String> {
    let shown = target.display().to_string();
    if fs::metadata(target).is_ok_and(|meta| {
        meta.is_file() && meta.len() == entry.size && mtime_of(&meta) == entry.mtime
    }) {
        report.unchanged.push(shown);
        return Ok(());
    }
    let dir = target
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary = dir.join(format!(
        ".{}.part",
        target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("download")
    ));
    let fail = |err: std::io::Error| format!("{}: {err}", temporary.display());
    let mut file = File::create(&temporary).map_err(fail)?;
    let mut rate = Rate::new(0);
    let mut offset = 0u64;
    let mut mtime = entry.mtime;
    let mut size = entry.size;
    while offset < size {
        let data: FileData = call(
            session,
            Command::FilesRead {
                path: entry.path.clone(),
                offset,
                len: protocol::UPDATE_CHUNK as u64,
            },
        )?;
        (size, mtime) = (data.size, data.mtime);
        if data.data.is_empty() {
            break;
        }
        let bytes = data_encoding::BASE64
            .decode(data.data.as_bytes())
            .map_err(|_| "the device sent a chunk that is not base64".to_string())?;
        file.write_all(&bytes).map_err(fail)?;
        offset += bytes.len() as u64;
        if size > protocol::UPDATE_CHUNK as u64 {
            let (speed, eta) = rate.update(offset, size);
            progress.show(
                &step_line(
                    style::LABEL,
                    "receiving",
                    format!(
                        "{}  {}/{}  {:>3}%  {}/s  {} {eta}",
                        entry.path,
                        mb(offset),
                        mb(size),
                        percent(offset, size),
                        mb(speed as u64),
                        paint(style::LABEL, "ETA"),
                    ),
                ),
                offset,
                size,
            );
        }
    }
    if offset != size {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "{} changed on the device while it was read; run it again",
            entry.path
        ));
    }
    file.set_modified(time_of(mtime)).map_err(fail)?;
    file.sync_all().map_err(fail)?;
    drop(file);
    fs::rename(&temporary, target).map_err(|err| format!("{shown}: {err}"))?;
    progress.done(&step_line(
        style::OK,
        "received",
        format!("{shown}  {}", paint(style::MUTED, human(size))),
    ));
    report.bytes += size;
    report.received.push(shown);
    Ok(())
}

/// Names from `base`, the directory listed, the way `ls` shows them. A file
/// listed on its own keeps the path it was asked for.
fn show_listing(base: &str, listing: &FilesListing) {
    if listing.entries.is_empty() {
        println!("{}", paint(style::MUTED, "(empty)"));
        return;
    }
    let mut total = 0u64;
    for entry in &listing.entries {
        let size = match entry.kind {
            FileKind::Dir => String::new(),
            FileKind::File => {
                total += entry.size;
                human(entry.size)
            }
        };
        let name = shown_name(base, &entry.path);
        let path = match entry.kind {
            FileKind::Dir => paint(style::HEADING, format!("{name}/")),
            FileKind::File => name.to_string(),
        };
        println!(
            "{}  {:>9}  {path}",
            paint(style::MUTED, date(entry.mtime)),
            size
        );
    }
    println!("{} {}", pad(style::LABEL, "total", 18), human(total));
}

/// `path` from `base`: `media/a.mp4` listed from `media` is `a.mp4`.
fn shown_name<'a>(base: &str, path: &'a str) -> &'a str {
    if base.is_empty() {
        return path;
    }
    path.strip_prefix(base)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(path)
}

/// Bytes the way a person reads them.
fn human(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["kB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// `YYYY-MM-DD HH:MM`, UTC: the device's clock may not be the local one.
fn date(mtime: i64) -> String {
    let days = mtime.div_euclid(86_400);
    let seconds = mtime.rem_euclid(86_400);
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60
    )
}

/// Whole seconds since the epoch, rounded down, as the device keeps them.
fn mtime_of(meta: &fs::Metadata) -> i64 {
    match meta.modified() {
        Ok(time) => match time.duration_since(UNIX_EPOCH) {
            Ok(after) => after.as_secs() as i64,
            Err(before) => -(before.duration().as_secs_f64().ceil() as i64),
        },
        Err(_) => 0,
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

    fn file(path: &str, size: u64, mtime: i64) -> Local {
        Local {
            path: path.into(),
            kind: FileKind::File,
            size,
            mtime,
            full: PathBuf::from(path),
        }
    }

    fn dir(path: &str) -> Local {
        Local {
            path: path.into(),
            kind: FileKind::Dir,
            size: 0,
            mtime: 0,
            full: PathBuf::from(path),
        }
    }

    fn stored(path: &str, kind: FileKind, size: u64, mtime: i64) -> FileEntry {
        FileEntry {
            path: path.into(),
            kind,
            size,
            mtime,
        }
    }

    #[test]
    fn sync_sends_what_changed_and_removes_what_is_gone() {
        let local = vec![
            dir("media"),
            file("media/new.mp4", 10, 5),
            file("media/same.mp4", 20, 7),
            file("menu.json", 3, 9),
            dir("empty"),
        ];
        let remote = vec![
            stored("media", FileKind::Dir, 0, 1),
            stored("media/old.mp4", FileKind::File, 1, 1),
            stored("media/same.mp4", FileKind::File, 20, 7),
            stored("menu.json", FileKind::File, 3, 8),
            stored("stale", FileKind::Dir, 0, 1),
            stored("stale/a", FileKind::File, 1, 1),
            stored("stale/b", FileKind::Dir, 0, 1),
        ];

        assert_eq!(
            plan(&local, &remote),
            vec![
                Action::Remove(vec!["media/old.mp4".into(), "stale".into()]),
                Action::Send(file("media/new.mp4", 10, 5)),
                Action::Unchanged("media/same.mp4".into()),
                Action::Send(file("menu.json", 3, 9)),
                Action::Mkdir("empty".into()),
            ]
        );
    }

    #[test]
    fn sync_swaps_a_file_and_a_directory_of_the_same_name() {
        let local = vec![file("a", 1, 1), dir("b"), file("b/x", 1, 1)];
        let remote = vec![
            stored("a", FileKind::Dir, 0, 1),
            stored("a/inner", FileKind::File, 1, 1),
            stored("b", FileKind::File, 1, 1),
        ];
        assert_eq!(
            plan(&local, &remote),
            vec![
                Action::Remove(vec!["a".into(), "b".into()]),
                Action::Send(file("a", 1, 1)),
                Action::Mkdir("b".into()),
                Action::Send(file("b/x", 1, 1)),
            ]
        );
    }

    #[test]
    fn sync_of_the_same_tree_does_nothing() {
        let local = vec![dir("a"), file("a/x", 4, 2)];
        let remote = vec![
            stored("a", FileKind::Dir, 0, 99),
            stored("a/x", FileKind::File, 4, 2),
        ];
        assert_eq!(plan(&local, &remote), vec![Action::Unchanged("a/x".into())]);
        assert_eq!(plan(&[], &remote), vec![Action::Remove(vec!["a".into()])]);
    }

    #[test]
    fn listings_are_made_relative_to_the_synced_directory() {
        let entries = vec![
            stored("site/a", FileKind::File, 1, 1),
            stored("site/b/c", FileKind::File, 1, 1),
        ];
        let paths: Vec<_> = relative("site", entries)
            .into_iter()
            .map(|entry| entry.path)
            .collect();
        assert_eq!(paths, vec!["a", "b/c"]);
    }

    #[test]
    fn the_local_scan_skips_what_cannot_be_sent() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub/x.txt"), "xyz").unwrap();
        fs::write(dir.path().join("a.txt"), "a").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("a.txt", dir.path().join("link")).unwrap();

        let (mut found, mut skipped) = (Vec::new(), Vec::new());
        scan(dir.path(), "", &mut found, &mut skipped).unwrap();
        let paths: Vec<_> = found.iter().map(|item| item.path.as_str()).collect();
        assert_eq!(paths, vec!["a.txt", "sub", "sub/x.txt"]);
        assert_eq!(found[2].size, 3);
        #[cfg(unix)]
        assert!(skipped[0].ends_with("a symlink"), "{skipped:?}");
    }

    #[test]
    fn a_listing_shows_names_from_the_directory_listed() {
        assert_eq!(shown_name("", "media/a.mp4"), "media/a.mp4");
        assert_eq!(shown_name("media", "media/a.mp4"), "a.mp4");
        assert_eq!(shown_name("media", "media/sub/a.mp4"), "sub/a.mp4");
        // A file listed on its own keeps its path.
        assert_eq!(shown_name("media/a.mp4", "media/a.mp4"), "media/a.mp4");
    }

    #[test]
    fn ancestors_walk_up() {
        assert_eq!(ancestors("a/b/c").collect::<Vec<_>>(), vec!["a", "a/b"]);
        assert_eq!(ancestors("a").count(), 0);
    }

    #[test]
    fn dates_are_utc() {
        assert_eq!(date(0), "1970-01-01 00:00");
        assert_eq!(date(1_700_000_000), "2023-11-14 22:13");
        assert_eq!(date(951_782_400), "2000-02-29 00:00");
        assert_eq!(date(-60), "1969-12-31 23:59");
    }

    #[test]
    fn sizes_read_like_a_person_would() {
        assert_eq!(human(999), "999 B");
        assert_eq!(human(1_500), "1.5 kB");
        assert_eq!(human(12_300_000), "12.3 MB");
    }
}
