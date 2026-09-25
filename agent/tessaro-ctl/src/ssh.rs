//! `tessaro-ctl ssh connect`: a root shell on a device, by key, or by the
//! empty password of an unclaimed one.
//!
//! How the key is sent and the host key pinned is in
//! `tessaro_client::ssh`; this prints what happened and then becomes `ssh`.

use std::path::PathBuf;

use anstream::println;
use serde_json::json;
use tessaro_client::ssh;

use crate::connect::Session;
use crate::style::{self, paint};

/// `ssh connect`, as it is typed.
#[derive(clap::Args)]
pub struct Options {
    /// The key to send: a .pub file, or a private key with its .pub
    /// next to it. Default: the first of ~/.ssh/id_ed25519.pub,
    /// id_ecdsa.pub, id_ecdsa_sk.pub, id_ed25519_sk.pub, id_rsa.pub.
    #[arg(long, short = 'i', value_name = "PATH")]
    pub key: Option<PathBuf>,
    /// The device's SSH port.
    #[arg(long, default_value_t = 22)]
    pub port: u16,
    /// Send the key and print the ssh command instead of running it.
    #[arg(long)]
    pub print: bool,
    #[arg(last = true, value_name = "SSH_ARGS")]
    pub args: Vec<String>,
}

pub fn run(session: &mut Session, options: Options, json: bool) -> Result<(), String> {
    let authorized = ssh::authorize(session, options.key.as_deref())?;
    let argv = authorized.argv(options.port, &options.args);
    let access = &authorized.access;

    if json {
        return crate::print_json(&json!({ "access": access, "command": argv }));
    }

    // Same shape as `claimed NAME (id)`: the verb, the device, the detail.
    let Some(access) = access else {
        anstream::eprintln!(
            "{} {} {}",
            paint(style::WARN, "unclaimed"),
            paint(style::HEADING, &session.node.name),
            paint(
                style::MUTED,
                "(root with an empty password, host key not checked)"
            )
        );
        if options.key.is_some() {
            anstream::eprintln!(
                "{}",
                paint(
                    style::WARN,
                    "--key is not used: an unclaimed device takes no key"
                )
            );
        }
        return finish(&argv, options.print);
    };
    let what = if access.added {
        paint(style::OK, "authorized on")
    } else {
        paint(style::MUTED, "already authorized on")
    };
    anstream::eprintln!(
        "{what} {} {}",
        paint(style::HEADING, &session.node.name),
        paint(style::MUTED, format!("({})", access.fingerprint))
    );
    if access.host_keys.is_empty() {
        anstream::eprintln!(
            "{}",
            paint(
                style::WARN,
                "the device did not send its host key; ssh will ask about it"
            )
        );
    }
    finish(&argv, options.print)
}

/// Print the command, or become it.
fn finish(argv: &[String], print: bool) -> Result<(), String> {
    if print {
        println!("{}", paint(style::CMD, shell_words(argv)));
        return Ok(());
    }
    exec(argv)
}

/// The command as a shell would need it typed.
pub fn shell_words(argv: &[String]) -> String {
    argv.iter()
        .map(|word| {
            let plain = !word.is_empty()
                && word
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || "@%+=:,./_-".contains(ch));
            if plain {
                word.clone()
            } else {
                format!("'{}'", word.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(unix)]
fn exec(argv: &[String]) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    // Only returns if ssh could not be started at all.
    let err = std::process::Command::new(&argv[0]).args(&argv[1..]).exec();
    Err(format!("{}: {err}", argv[0]))
}

#[cfg(not(unix))]
fn exec(argv: &[String]) -> Result<(), String> {
    let status = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .status()
        .map_err(|err| format!("{}: {err}", argv[0]))?;
    std::process::exit(status.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_words_quote_what_needs_it() {
        let argv = ["echo".to_string(), "a b".to_string(), "it's".to_string()];
        assert_eq!(shell_words(&argv), "echo 'a b' 'it'\\''s'");
    }
}
