//! Everything that asks the person at the keyboard: yes or no, the device's
//! name before something that cannot be undone, and passwords.

use std::io::{Read, Write};

use crate::connect::Session;
// Shadow the std macros: these strip colors when stderr is not a terminal.
use crate::out::{self, eprint, eprintln};
use crate::style::{self, paint};

/// Nobody can answer at the keyboard for one of several devices: stdin is
/// one, and the devices run at once. (A command with `-y` is refused before
/// it runs on any of them without it, `run_several` in main.rs.)
fn keyboard() -> Result<(), String> {
    if out::capturing() {
        return Err(
            "this reads the keyboard or stdin, which a run on several devices \
                    cannot share; name one device"
                .to_string(),
        );
    }
    Ok(())
}

/// `question [y/N]`, and whether the answer was yes.
pub fn ask(question: &str) -> Result<bool, String> {
    keyboard()?;
    eprint!("{question} {} ", paint(style::LABEL, "[y/N]"));
    std::io::stderr().flush().ok();
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .map_err(|err| err.to_string())?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes"))
}

/// `ask`, unless `--yes` already said so; refused with `not confirmed`.
pub fn confirm(yes: bool, question: &str) -> Result<(), String> {
    if yes || ask(question)? {
        Ok(())
    } else {
        Err("not confirmed".to_string())
    }
}

/// For what cannot be undone: the device's name, typed out, unless `--yes`.
pub fn confirm_destructive(session: &Session, yes: bool, what: &str) -> Result<(), String> {
    if yes {
        return Ok(());
    }
    keyboard()?;
    eprintln!(
        "{} {}.",
        paint(style::WARN, format!("This will {what} on")),
        paint(style::HEADING, &session.node.name)
    );
    eprint!(
        "{}",
        paint(style::LABEL, "Type the device name to go ahead: ")
    );
    let mut typed = String::new();
    std::io::stdin()
        .read_line(&mut typed)
        .map_err(|err| err.to_string())?;
    if typed.trim() == session.node.name {
        Ok(())
    } else {
        Err("not confirmed".to_string())
    }
}

/// A password: all of stdin with `--password-stdin` (less the line end), else
/// typed at `prompt` without echo.
pub fn password(from_stdin: bool, prompt: &str) -> Result<String, String> {
    keyboard()?;
    if from_stdin {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|err| format!("reading the password: {err}"))?;
        return Ok(text.trim_end_matches(['\n', '\r']).to_string());
    }
    rpassword::prompt_password(prompt).map_err(|err| err.to_string())
}

/// A new password typed twice, so a slip is caught before it is set.
pub fn new_password(prompt: &str) -> Result<String, String> {
    let first = password(false, prompt)?;
    let again = password(false, "again: ")?;
    tessaro_client::actions::root_password(&first, &again)?;
    Ok(first)
}
