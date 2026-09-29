//! `tessaro-ctl browser policies ...`: named Chromium policy documents the
//! device merges into the policy it renders (docs/kiosk-browser.md,
//! **Policies**). `set` sends a file as it is, for scripts; `edit` opens the
//! stored text, or a commented example for a new one, in `$VISUAL` or
//! `$EDITOR` and saves it only if nobody changed it on the device meanwhile.
//! The policies are in priority order: position 1 wins a policy others set
//! too, and `move` changes it.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anstream::{eprintln, println};
use clap::Subcommand;
use protocol::api::{self, PolicyBody, PolicyPositionBody, PolicyRef};
use protocol::policy;
use tessaro_client::describe::browser as describe;

use crate::connect::Session;
use crate::print;
use crate::prompt;
use crate::style::{self, paint};

/// `tessaro-ctl browser policies ...`.
#[derive(Subcommand)]
pub enum PoliciesCmd {
    /// The stored policies in priority order, and the Chromium policies each
    /// one sets. Position 1 wins a policy others set too.
    List,
    /// One policy's text as stored, or without a name the merged policy
    /// Chromium reads, each entry with where it comes from: the image, a
    /// policy, or the device.
    ///
    ///   tessaro-ctl browser policies show
    ///   tessaro-ctl browser policies show lockdown > lockdown.json
    Show { name: Option<String> },
    /// Store FILE as the policy NAME, replacing one of that name: one JSON
    /// object of Chromium policies (chromeenterprise.google/policies),
    /// comments and trailing commas allowed. A new one goes to the bottom,
    /// the lowest priority. The browser restarts when the result changed.
    ///
    ///   tessaro-ctl browser policies set lockdown lockdown.json
    ///   cat lockdown.json | tessaro-ctl browser policies set lockdown -
    ///   tessaro-ctl browser policies set lockdown lockdown.json --position 1
    Set {
        name: String,
        /// The file, or - to read it from stdin.
        file: PathBuf,
        /// Put it at this position, from 1, the highest priority.
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
        position: Option<u32>,
    },
    /// Move the policy NAME to POSITION in the priority order, the others
    /// shifting to make room; past the last, the last. Position 1 wins a
    /// policy others set too. The browser restarts when the result changed.
    ///
    ///   tessaro-ctl browser policies move lockdown 1
    Move {
        name: String,
        #[arg(value_parser = clap::value_parser!(u32).range(1..))]
        position: u32,
    },
    /// Check FILE the way the device would, saving nothing. Needs no
    /// device.
    ///
    ///   tessaro-ctl browser policies check lockdown.json
    Check { file: PathBuf },
    /// Open the policy NAME in $VISUAL or $EDITOR, a commented example when
    /// there is none yet, and store it when the editor closes. A mistake
    /// is shown with its line, with the choice to edit it again.
    ///
    ///   tessaro-ctl browser policies edit lockdown
    Edit { name: String },
    /// Remove the policy NAME. The browser restarts when the result changed.
    Remove { name: String },
}

/// `check`, which needs no device.
pub fn check(file: &Path, json: bool) -> Result<(), String> {
    let text = tessaro_client::policies::read_file(file)?;
    let keys = policy::check(&text)
        .map(|entries| policy::keys(&entries))
        .unwrap_or_default();
    print(json, &serde_json::json!({ "keys": keys }), || {
        println!(
            "{} {}",
            paint(style::OK, "ok"),
            if keys.is_empty() {
                "sets nothing".to_string()
            } else {
                keys.join(", ")
            }
        );
    })
}

pub fn run(session: &mut Session, what: PoliciesCmd, json: bool) -> Result<(), String> {
    match what {
        PoliciesCmd::List => {
            let policies = session.fetch::<api::browser::Policies>()?;
            print(json, &policies, || lines(describe::policies(&policies)))
        }
        PoliciesCmd::Show { name: None } => {
            let entries = session.fetch::<api::browser::Effective>()?;
            print(json, &entries, || lines(describe::effective(&entries)))
        }
        PoliciesCmd::Show { name: Some(name) } => {
            let doc = session.call::<api::browser::Policy>(PolicyRef { name }, ())?;
            // The document itself, unstyled, so it can be saved and set again.
            print(json, &doc, || anstream::print!("{}", doc.text))
        }
        PoliciesCmd::Set {
            name,
            file,
            position,
        } => {
            policy::check_name(&name)?;
            let text = tessaro_client::policies::read_file(&file)?;
            save(session, name, text, None, position, json)
        }
        PoliciesCmd::Move { name, position } => {
            let moved = session.call::<api::browser::PolicyMove>(
                PolicyRef { name },
                PolicyPositionBody { position },
            )?;
            print(json, &moved, || lines(describe::policy_moved(&moved)))
        }
        PoliciesCmd::Check { file } => check(&file, json),
        PoliciesCmd::Edit { name } => edit(session, name, json),
        PoliciesCmd::Remove { name } => {
            let removed = session.call::<api::browser::PolicyRemove>(PolicyRef { name }, ())?;
            print(json, &removed, || lines(describe::policy_removed(&removed)))
        }
    }
}

fn save(
    session: &mut Session,
    name: String,
    text: String,
    if_revision: Option<String>,
    position: Option<u32>,
    json: bool,
) -> Result<(), String> {
    let saved = session.call::<api::browser::PolicySet>(
        PolicyRef { name },
        PolicyBody {
            text,
            if_revision,
            position,
        },
    )?;
    print(json, &saved, || lines(describe::policy_saved(&saved)))
}

fn edit(session: &mut Session, name: String, json: bool) -> Result<(), String> {
    policy::check_name(&name)?;
    let stored = session
        .fetch::<api::browser::Policies>()?
        .into_iter()
        .any(|known| known.name == name);
    let (start, revision) = if stored {
        let doc = session.call::<api::browser::Policy>(PolicyRef { name: name.clone() }, ())?;
        (doc.text, doc.revision)
    } else {
        // "" saves only while there is still none of that name.
        (policy::TEMPLATE.to_string(), String::new())
    };

    let file = scratch(&name, &start)?;
    let text = loop {
        run_editor(&file)?;
        let text = fs::read_to_string(&file).map_err(|err| format!("{}: {err}", file.display()))?;
        if text == start {
            let _ = fs::remove_file(&file);
            eprintln!("{}", paint(style::MUTED, "unchanged; nothing saved"));
            return Ok(());
        }
        match policy::check(&text) {
            Ok(_) => break text,
            Err(err) => {
                eprintln!("{} {err}", paint(style::BAD, "refused:"));
                if !prompt::ask("Edit it again?")? {
                    return Err(format!("not saved; your text is in {}", file.display()));
                }
            }
        }
    };

    match save(session, name.clone(), text, Some(revision), None, json) {
        Ok(()) => {
            let _ = fs::remove_file(&file);
            Ok(())
        }
        Err(err) => Err(format!(
            "{err}\nyour text is in {}; `tessaro-ctl browser policies set {name} {}` stores it as it is",
            file.display(),
            file.display()
        )),
    }
}

/// A file only this user can read, holding `text`, for the editor.
fn scratch(name: &str, text: &str) -> Result<PathBuf, String> {
    let path =
        std::env::temp_dir().join(format!("tessaro-policy-{name}-{}.json", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options
        .open(&path)
        .map_err(|err| format!("{}: {err}", path.display()))?;
    file.write_all(text.as_bytes())
        .map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(path)
}

/// `$VISUAL`, else `$EDITOR`, else vi (Notepad on Windows), on `file`,
/// until it exits. The variable may carry arguments: `code --wait`.
fn run_editor(file: &Path) -> Result<(), String> {
    let editor = std::env::var("VISUAL")
        .ok()
        .or_else(|| std::env::var("EDITOR").ok())
        .filter(|editor| !editor.trim().is_empty())
        .unwrap_or_else(|| if cfg!(windows) { "notepad" } else { "vi" }.to_string());
    let mut words = editor.split_whitespace();
    let program = words.next().expect("not empty");
    let status = std::process::Command::new(program)
        .args(words)
        .arg(file)
        .status()
        .map_err(|err| format!("{program}: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "{program} exited with {status}; your text is in {}",
            file.display()
        ))
    }
}

fn lines(lines: Vec<tessaro_client::text::Line>) {
    for line in lines {
        println!("{}", style::line(&line));
    }
}
