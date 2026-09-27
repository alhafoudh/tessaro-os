//! `tessaro-ctl ssh connect`: a root shell on a device, by key, or by the
//! empty password of an unclaimed one.
//!
//! How the key is sent, the host key pinned and what that did is said is
//! `tessaro_client::ssh`, shared with the GUI; this prints it and then
//! becomes `ssh`.

use std::path::PathBuf;

use anstream::println;
use serde_json::json;
use tessaro_client::ssh::{self, shell_words};

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
    #[arg(long, default_value_t = ssh::PORT)]
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

    if json {
        return crate::print_json(&json!({ "access": &authorized.access, "command": argv }));
    }
    for line in authorized.lines(&session.node.name, options.key.is_some()) {
        anstream::eprintln!("{}", style::line(&line));
    }
    if options.print {
        println!("{}", paint(style::CMD, shell_words(&argv)));
        return Ok(());
    }
    exec(&argv)
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
