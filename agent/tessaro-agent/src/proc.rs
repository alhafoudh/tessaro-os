//! Running the programs the agent drives - sfdisk, mount, nft, wpctl, the
//! Weston config generator - the same way every time: stdin fed or closed,
//! stdout and stderr captured, and a failure to run at all said as
//! `program: why`. Whether the program's exit status is a failure, and how
//! to put it in words, is the caller's: `said` has what it wrote.

use std::io::Write;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use crate::deadline::within;

/// Blocking: call it from `spawn_blocking` or a thread of its own.
pub fn run(command: &mut Command, input: Option<&[u8]>) -> Result<Output, String> {
    let program = program(command.get_program());
    let fail = |err: std::io::Error| format!("{program}: {err}");
    let mut child = command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(fail)?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin.write_all(input).map_err(fail)?;
    }
    child.wait_with_output().map_err(fail)
}

/// On the agent's runtime: the whole run, input included, is under `limit`,
/// and a child still running when it expires is killed.
pub async fn run_async(
    command: &mut tokio::process::Command,
    input: Option<&[u8]>,
    what: &'static str,
    limit: Duration,
) -> Result<Output, String> {
    let program = program(command.as_std().get_program());
    let fail = |err: std::io::Error| format!("{program}: {err}");
    let mut child = command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(fail)?;
    let mut stdin = child.stdin.take();
    let run = async move {
        if let (Some(input), Some(stdin)) = (input, stdin.as_mut()) {
            use tokio::io::AsyncWriteExt;
            // naked: bounded by the within() below
            stdin.write_all(input).await?;
        }
        drop(stdin);
        // naked: bounded by the within() below
        child.wait_with_output().await
    };
    match within(what, limit, run).await {
        Ok(outcome) => outcome.map_err(fail),
        Err(expired) => Err(expired.to_string()),
    }
}

/// What a finished program wrote to stderr, trimmed.
pub fn said(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_string()
}

fn program(name: &std::ffi::OsStr) -> String {
    name.to_string_lossy().into_owned()
}
