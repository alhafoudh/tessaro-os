//! `tessaro-ctl files ...`: the device's file store, `/data/files`, which
//! its local web server serves at `http://127.0.0.1/files/`.
//!
//! Files go up the way an image does: in `UPDATE_CHUNK` pieces, each
//! acknowledged before the next, and a file the device already has with the
//! same size and mtime is not sent again. `sync` is rsync `-r --delete`
//! compared on size and mtime alone: after it the store, or a directory in
//! it, holds exactly what the local directory does. The transfers are
//! `tessaro_client::files`, shared with the GUI.

use std::path::PathBuf;

use anstream::{eprintln, println};
use clap::Subcommand;
use protocol::api::{self, DeleteBody, Empty, FilesQuery, MoveBody};
use protocol::files::{self as store, FileKind, FilesListing};
use protocol::size_label;
use tessaro_client::files::{self as tree, shown_name, Summary};
use tessaro_client::transfer::date;

use crate::connect::Session;
use crate::progress::Progress;
use crate::style::{self, pad, paint};
use crate::{print, prompt};

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
            let query = FilesQuery {
                path: path.clone(),
                recursive,
            };
            let listing = session.call::<api::files::List>(query, ())?;
            print(json, &listing, || show_listing(&path, &listing))
        }
        FilesCmd::Upload { local, remote } => {
            let summary =
                tree::upload(session, &local, remote.as_deref(), &mut Progress::new(json))?;
            done(json, &summary, "sent")
        }
        FilesCmd::Download { remote, local } => {
            let summary = tree::download(session, &remote, local, &mut Progress::new(json))?;
            done(json, &summary, "received")
        }
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
                let done = session.send::<api::files::Move>(MoveBody { from, to })?;
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
            prompt::confirm(
                yes,
                &format!("Remove {} from {}?", paths.join(", "), session.node.name),
            )?;
            let body = DeleteBody { paths, recursive };
            crate::done::<api::files::Delete>(session, Empty {}, body, json)
        }
    }
}

/// Whether `path` is a directory in the store: its listing is not the file
/// itself. A missing path is an error, as it would be for `mv`.
fn is_dir(session: &mut Session, path: &str) -> Result<bool, String> {
    let query = FilesQuery {
        path: path.to_string(),
        recursive: false,
    };
    let listing = session.call::<api::files::List>(query, ())?;
    Ok(!(listing.entries.len() == 1
        && listing.entries[0].path == path
        && listing.entries[0].kind == FileKind::File))
}

/// A transfer's closing line, or its summary with `--json`.
fn done(json: bool, summary: &Summary, verb: &str) -> Result<(), String> {
    print(json, summary, || {
        println!("{}", style::line(&summary.line(verb)))
    })
}

fn sync(
    session: &mut Session,
    local: &std::path::Path,
    remote: Option<&str>,
    dry_run: bool,
    yes: bool,
    json: bool,
) -> Result<(), String> {
    let plan = tree::plan_sync(session, local, remote, dry_run)?;
    for line in &plan.skipped {
        eprintln!("{} {line}", paint(style::WARN, "skipped:"));
    }

    if dry_run {
        for line in plan.lines() {
            println!("{}", style::line(&line));
        }
        println!("{}", paint(style::MUTED, "(dry run: nothing changed)"));
        return Ok(());
    }

    if !plan.removals.is_empty() && !yes {
        for line in plan.lines().iter().take(plan.removals.len()) {
            eprintln!("{}", style::line(line));
        }
        if !prompt::ask(&format!(
            "Remove what is listed above from {}?",
            session.node.name
        ))? {
            return Err("not confirmed; nothing was changed".to_string());
        }
    }

    let summary = tree::sync(session, plan, &mut Progress::new(json))?;
    done(json, &summary, "sent")
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
                size_label(entry.size)
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
    println!("{} {}", pad(style::LABEL, "total", 18), size_label(total));
}
