//! QEMU's control socket (QMP), for the two things the launcher asks of a
//! running VM: press the power button (`system_powerdown`, so the device
//! shuts down cleanly) and, when that is ignored, `quit`.
//!
//! A QMP conversation is JSON lines: QEMU greets, the client sends
//! `qmp_capabilities`, then commands; each gets a `return` or an `error`,
//! and `event` lines may come in between.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

const TIMEOUT: Duration = Duration::from_secs(3);

/// Run `command` over the QMP socket at `socket`.
#[cfg(unix)]
pub fn execute(socket: &Path, command: &str) -> Result<(), String> {
    let stream = std::os::unix::net::UnixStream::connect(socket)
        .map_err(|err| format!("the VM's control socket: {err}"))?;
    stream.set_read_timeout(Some(TIMEOUT)).ok();
    stream.set_write_timeout(Some(TIMEOUT)).ok();
    let reader = stream
        .try_clone()
        .map_err(|err| format!("the VM's control socket: {err}"))?;
    converse(BufReader::new(reader), stream, command)
}

#[cfg(not(unix))]
pub fn execute(_socket: &Path, _command: &str) -> Result<(), String> {
    Err("QMP over a named pipe is not done yet".to_string())
}

fn converse(mut reader: impl BufRead, mut writer: impl Write, command: &str) -> Result<(), String> {
    let greeting = read(&mut reader)?;
    if greeting.get("QMP").is_none() {
        return Err(format!("not a QMP greeting: {greeting}"));
    }
    for execute in ["qmp_capabilities", command] {
        let line = serde_json::json!({ "execute": execute }).to_string();
        writeln!(writer, "{line}").map_err(|err| format!("QMP: {err}"))?;
        writer.flush().map_err(|err| format!("QMP: {err}"))?;
        answer(&mut reader, execute)?;
    }
    Ok(())
}

/// The answer to `execute`, past any events.
fn answer(reader: &mut impl BufRead, execute: &str) -> Result<(), String> {
    loop {
        let message = read(reader)?;
        if message.get("return").is_some() {
            return Ok(());
        }
        if let Some(error) = message.get("error") {
            let why = error
                .get("desc")
                .and_then(Value::as_str)
                .unwrap_or("no reason given");
            return Err(format!("QMP {execute}: {why}"));
        }
    }
}

fn read(reader: &mut impl BufRead) -> Result<Value, String> {
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) => Err("QMP: the VM closed its control socket".to_string()),
        Ok(_) => serde_json::from_str(&line).map_err(|err| format!("QMP: {err}: {line}")),
        Err(err) => Err(format!("QMP: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GREETING: &str = r#"{"QMP": {"version": {}, "capabilities": []}}"#;

    fn run(server: &str, command: &str) -> (Result<(), String>, String) {
        let mut sent = Vec::new();
        let result = converse(server.as_bytes(), &mut sent, command);
        (result, String::from_utf8(sent).unwrap())
    }

    #[test]
    fn a_command_is_sent_after_the_capabilities_handshake() {
        let server = format!("{GREETING}\n{{\"return\": {{}}}}\n{{\"return\": {{}}}}\n");
        let (result, sent) = run(&server, "system_powerdown");
        assert_eq!(result, Ok(()));
        assert_eq!(
            sent,
            "{\"execute\":\"qmp_capabilities\"}\n{\"execute\":\"system_powerdown\"}\n"
        );
    }

    #[test]
    fn events_before_the_answer_are_passed_over() {
        let server = format!(
            "{GREETING}\n{{\"return\": {{}}}}\n{{\"event\": \"POWERDOWN\"}}\n{{\"return\": {{}}}}\n"
        );
        assert_eq!(run(&server, "system_powerdown").0, Ok(()));
    }

    #[test]
    fn an_error_answer_says_why() {
        let server =
            format!("{GREETING}\n{{\"return\": {{}}}}\n{{\"error\": {{\"desc\": \"nope\"}}}}\n");
        assert_eq!(run(&server, "quit").0, Err("QMP quit: nope".to_string()));
    }

    #[test]
    fn a_closed_socket_is_an_error() {
        assert!(run(GREETING, "quit").0.is_err());
    }
}
