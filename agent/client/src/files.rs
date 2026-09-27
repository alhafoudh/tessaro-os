//! Moving whole trees to and from the device's file store, `/data/files`,
//! for `tessaro-ctl files` and the GUI's Files page alike.
//!
//! Each file goes through `transfer`: in acknowledged chunks, and not at all
//! when the other side has it with the same size and mtime. `sync` is rsync
//! `-r --delete` compared on size and mtime alone. What cannot be sent -
//! symlinks, special files, names the device would refuse - is skipped and
//! said so, never followed: a symlink loop would never end.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use protocol::api::{self, DeleteBody, FilesQuery, PathBody};
use protocol::files::{self as store, FileEntry, FileKind};
use protocol::size_label;
use serde::Serialize;

use crate::connect::Session;
use crate::report::{step_line, Rate, Report};
use crate::text::{Line, Tone};
use crate::transfer::{self, mb, mtime_of};

/// What a transfer did, for `--json` and the closing line.
#[derive(Debug, Default, Serialize)]
pub struct Summary {
    pub sent: Vec<String>,
    pub received: Vec<String>,
    pub removed: Vec<String>,
    pub made: Vec<String>,
    pub unchanged: Vec<String>,
    pub skipped: Vec<String>,
    pub bytes: u64,
}

impl Summary {
    /// `done: 3 sent (1.2 MB), 2 unchanged`, with `verb` for what moved.
    pub fn line(&self, verb: &str) -> Line {
        let moved = self.sent.len() + self.received.len();
        let mut line =
            Line::of(Tone::Ok, "done:").text(format!(" {moved} {verb} ({})", mb(self.bytes)));
        if !self.removed.is_empty() {
            line = line.text(format!(", {} removed", self.removed.len()));
        }
        line = line.text(format!(", {} unchanged", self.unchanged.len()));
        if !self.skipped.is_empty() {
            line = line
                .text(", ")
                .add(Tone::Warn, format!("{} skipped", self.skipped.len()));
        }
        line
    }
}

/// A file or directory on this machine, by its path from the directory
/// being sent, in the device's spelling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Local {
    pub path: String,
    pub kind: FileKind,
    pub size: u64,
    pub mtime: i64,
    pub full: PathBuf,
}

/// Everything under `dir`, depth first, names sorted, a directory before
/// what is in it. What cannot be sent - symlinks, sockets, names the device
/// would refuse - is left out, with a line saying so in `skipped`.
pub fn scan(
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
pub enum Action {
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
pub fn plan(local: &[Local], remote: &[FileEntry]) -> Vec<Action> {
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

/// A sync worked out, not yet done: what goes, so the caller can list it
/// and ask before anything is removed.
pub struct Sync {
    pub root: String,
    pub actions: Vec<Action>,
    /// Every path the sync removes, from the top of the store.
    pub removals: Vec<String>,
    /// What the local scan left out.
    pub skipped: Vec<String>,
}

impl Sync {
    /// What the sync would do, as lines: for a dry run, or before asking.
    pub fn lines(&self) -> Vec<Line> {
        let mut lines = Vec::new();
        for action in &self.actions {
            match action {
                Action::Remove(_) => {
                    for path in &self.removals {
                        lines.push(step_line(Tone::Warn, "remove", path.as_str()));
                    }
                }
                Action::Mkdir(path) => lines.push(step_line(
                    Tone::Label,
                    "mkdir",
                    store::join(&self.root, path),
                )),
                Action::Send(item) => lines.push(step_line(
                    Tone::Label,
                    "send",
                    Line::plain(format!("{}  ", store::join(&self.root, &item.path)))
                        .add(Tone::Muted, size_label(item.size)),
                )),
                Action::Unchanged(_) => {}
            }
        }
        lines
    }
}

/// Work out what makes REMOTE (by default the whole store) hold exactly
/// what `local` does. Unless `dry_run`, REMOTE is made if it is missing.
pub fn plan_sync(
    session: &mut Session,
    local: &Path,
    remote: Option<&str>,
    dry_run: bool,
) -> Result<Sync, String> {
    let root = store::normalize(remote.unwrap_or(""))?;
    if !local.is_dir() {
        return Err(format!("{} is not a directory", local.display()));
    }
    let (mut ours, mut skipped) = (Vec::new(), Vec::new());
    scan(local, "", &mut ours, &mut skipped)?;

    // A directory that is not there yet lists as empty; making it is the
    // first change, and a dry run makes nothing.
    let tree = FilesQuery {
        path: root.clone(),
        recursive: true,
    };
    let listed = if dry_run || root.is_empty() {
        session.call::<api::files::List>(tree, ())
    } else {
        mkdir(session, &root).and_then(|()| session.call::<api::files::List>(tree, ()))
    };
    let theirs = match listed {
        Ok(listing) => relative(&root, listing.entries),
        Err(err) if dry_run && err.ends_with("does not exist") => Vec::new(),
        Err(err) => return Err(err),
    };

    let actions = plan(&ours, &theirs);
    let removals = actions
        .iter()
        .find_map(|action| match action {
            Action::Remove(paths) => {
                Some(paths.iter().map(|path| store::join(&root, path)).collect())
            }
            _ => None,
        })
        .unwrap_or_default();
    Ok(Sync {
        root,
        actions,
        removals,
        skipped,
    })
}

/// Do what `plan_sync` worked out.
pub fn sync(session: &mut Session, sync: Sync, report: &mut dyn Report) -> Result<Summary, String> {
    let mut summary = Summary {
        skipped: sync.skipped,
        ..Summary::default()
    };
    for action in sync.actions {
        stop(report)?;
        match action {
            Action::Remove(_) => {
                session.send::<api::files::Delete>(DeleteBody {
                    paths: sync.removals.clone(),
                    recursive: true,
                })?;
                for path in &sync.removals {
                    report.line(step_line(Tone::Warn, "removed", path.as_str()));
                }
                summary.removed.extend(sync.removals.iter().cloned());
            }
            Action::Mkdir(path) => {
                let path = store::join(&sync.root, &path);
                mkdir(session, &path)?;
                summary.made.push(path);
            }
            Action::Send(item) => {
                let path = store::join(&sync.root, &item.path);
                send(session, &item, &path, report, &mut summary)?;
            }
            Action::Unchanged(path) => summary.unchanged.push(store::join(&sync.root, &path)),
        }
    }
    Ok(summary)
}

/// The listing's paths from `root` down, `root` itself left out.
pub fn relative(root: &str, entries: Vec<FileEntry>) -> Vec<FileEntry> {
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

/// Store `local`, a file or a directory with everything in it, as REMOTE.
/// A file goes to REMOTE (a trailing / keeps its name), by default its own
/// name at the top; a directory's contents go into REMOTE, by default a
/// directory of its own name. Nothing on the device is removed.
pub fn upload(
    session: &mut Session,
    local: &Path,
    remote: Option<&str>,
    report: &mut dyn Report,
) -> Result<Summary, String> {
    let meta = fs::metadata(local).map_err(|err| format!("{}: {err}", local.display()))?;
    let name = local
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("{}: no usable file name", local.display()))?
        .to_string();
    let mut summary = Summary::default();

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
        send(session, &item, &target, report, &mut summary)?;
        return Ok(summary);
    }
    if !meta.is_dir() {
        return Err(format!("{} is not a file or a directory", local.display()));
    }

    let root = match remote {
        None => store::normalize(&name)?,
        Some(remote) => store::normalize(remote)?,
    };
    let mut ours = Vec::new();
    scan(local, "", &mut ours, &mut summary.skipped)?;
    for line in &summary.skipped {
        report.line(Line::of(Tone::Warn, "skipped:").text(format!(" {line}")));
    }
    if !root.is_empty() {
        mkdir(session, &root)?;
    }
    for item in &ours {
        stop(report)?;
        let path = store::join(&root, &item.path);
        match item.kind {
            FileKind::Dir => {
                mkdir(session, &path)?;
                summary.made.push(path);
            }
            FileKind::File => send(session, item, &path, report, &mut summary)?,
        }
    }
    Ok(summary)
}

/// Make `path` in the store, and any missing directory above it.
pub fn mkdir(session: &mut Session, path: &str) -> Result<(), String> {
    session
        .send::<api::files::Mkdir>(PathBody {
            path: path.to_string(),
        })
        .map(drop)
}

/// One file to `path` on the device, from where the device says it has got
/// to: nowhere, part of it, or all of it already.
fn send(
    session: &mut Session,
    item: &Local,
    path: &str,
    report: &mut dyn Report,
    summary: &mut Summary,
) -> Result<(), String> {
    let mut rate: Option<Rate> = None;
    let sent = transfer::send_file(session, &item.full, path, item.size, item.mtime, |offset| {
        // Only a file of several chunks is worth a progress line of its own.
        if item.size > protocol::UPDATE_CHUNK as u64 {
            let rate = rate.get_or_insert_with(|| Rate::new(offset));
            report.progress(
                step_line(
                    Tone::Label,
                    "sending",
                    Line::plain(format!("{path}  ")).join(rate.line(offset, item.size)),
                ),
                offset,
                item.size,
            );
        }
    })?;
    let Some(bytes) = sent else {
        summary.unchanged.push(path.to_string());
        return Ok(());
    };
    report.line(step_line(
        Tone::Ok,
        "sent",
        Line::plain(format!("{path}  ")).add(Tone::Muted, size_label(item.size)),
    ));
    summary.bytes += bytes;
    summary.sent.push(path.to_string());
    Ok(())
}

/// Fetch REMOTE, a file or a directory with everything in it, to `local`:
/// by default its own name here, and into `local` if that is a directory.
pub fn download(
    session: &mut Session,
    remote: &str,
    local: Option<PathBuf>,
    report: &mut dyn Report,
) -> Result<Summary, String> {
    let path = store::normalize(remote)?;
    let query = FilesQuery {
        path: path.clone(),
        recursive: true,
    };
    let listing = session.call::<api::files::List>(query, ())?;
    let name = path
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("files");
    let mut summary = Summary::default();

    let single = listing.entries.len() == 1
        && listing.entries[0].path == path
        && listing.entries[0].kind == FileKind::File;
    if single {
        let target = match local {
            Some(local) if local.is_dir() => local.join(name),
            Some(local) => local,
            None => PathBuf::from(name),
        };
        fetch(session, &listing.entries[0], &target, report, &mut summary)?;
        return Ok(summary);
    }

    // A named directory, not the whole store: `download media .` makes
    // `./media`, while `download / .` fills `.` itself.
    let base = match local {
        Some(local) if local.is_dir() && !path.is_empty() => local.join(name),
        Some(local) => local,
        None => PathBuf::from(name),
    };
    make_local_dir(&base)?;
    for entry in relative(&path, listing.entries) {
        stop(report)?;
        let target = base.join(entry.path.split('/').collect::<PathBuf>());
        match entry.kind {
            FileKind::Dir => {
                make_local_dir(&target)?;
                summary.made.push(target.display().to_string());
            }
            FileKind::File => {
                let entry = FileEntry {
                    path: store::join(&path, &entry.path),
                    ..entry
                };
                fetch(session, &entry, &target, report, &mut summary)?;
            }
        }
    }
    Ok(summary)
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
    report: &mut dyn Report,
    summary: &mut Summary,
) -> Result<(), String> {
    let shown = target.display().to_string();
    let mut rate = Rate::new(0);
    let mut size = entry.size;
    let fetched = transfer::fetch_file(session, entry, target, |offset, total| {
        size = total;
        if total > protocol::UPDATE_CHUNK as u64 {
            report.progress(
                step_line(
                    Tone::Label,
                    "receiving",
                    Line::plain(format!("{}  ", entry.path)).join(rate.line(offset, total)),
                ),
                offset,
                total,
            );
        }
    })?;
    if !fetched {
        summary.unchanged.push(shown);
        return Ok(());
    }
    report.line(step_line(
        Tone::Ok,
        "received",
        Line::plain(format!("{shown}  ")).add(Tone::Muted, size_label(size)),
    ));
    summary.bytes += size;
    summary.received.push(shown);
    Ok(())
}

/// `path` from `base`, the directory listed, the way `ls` shows it:
/// `media/a.mp4` listed from `media` is `a.mp4`. A file listed on its own
/// keeps the path it was asked for.
pub fn shown_name<'a>(base: &str, path: &'a str) -> &'a str {
    if base.is_empty() {
        return path;
    }
    path.strip_prefix(base)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(path)
}

fn stop(report: &dyn Report) -> Result<(), String> {
    if report.stopped() {
        Err("stopped".to_string())
    } else {
        Ok(())
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

    #[cfg(unix)]
    #[test]
    fn a_symlink_loop_is_skipped_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        std::os::unix::fs::symlink("..", dir.path().join("sub/up")).unwrap();

        let (mut found, mut skipped) = (Vec::new(), Vec::new());
        scan(dir.path(), "", &mut found, &mut skipped).unwrap();
        assert_eq!(found.len(), 1);
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
    fn the_closing_line_counts_what_happened() {
        let summary = Summary {
            sent: vec!["a".into(), "b".into()],
            unchanged: vec!["c".into()],
            skipped: vec!["d".into()],
            bytes: 1_500_000,
            ..Summary::default()
        };
        assert_eq!(
            summary.line("sent").to_string(),
            "done: 2 sent (1.5 MB), 1 unchanged, 1 skipped"
        );
    }
}
