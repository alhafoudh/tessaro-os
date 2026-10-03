//! The other ways in: `tessaro-gui`, a terminal with `tessaro-ctl` ready,
//! Webconfig in the browser, the docs.

use std::path::Path;
use std::process::{Command, Stdio};

use tessaro_client::webconfig;

use crate::device;
use crate::paths::Bundle;

pub const DOCS: &str = "https://github.com/alhafoudh/tessaro-os#readme";

/// The bundled `tessaro-gui`. The launcher has already put the device in
/// the client store, so it is in the node list.
pub fn gui(bundle: &Bundle) -> Result<(), String> {
    let app = bundle.gui_app();
    if !app.exists() {
        return Err(format!("{} is missing from the bundle", app.display()));
    }
    spawn(Command::new("open").arg("-n").arg(app))
}

/// A terminal where `tessaro-ctl` is on the `PATH` and `TESSARO_NODE` names
/// the device, so every command reaches it with no `-n`.
pub fn terminal(bundle: &Bundle, dir: &Path, api: u16) -> Result<(), String> {
    let script = dir.join("terminal.command");
    let body = terminal_script(&bundle.bin().to_string_lossy(), &device::address(api));
    std::fs::write(&script, body).map_err(|err| format!("{}: {err}", script.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .map_err(|err| format!("{}: {err}", script.display()))?;
    }
    spawn(Command::new("open").args(["-a", "Terminal"]).arg(&script))
}

fn terminal_script(bin: &str, node: &str) -> String {
    format!(
        "#!/bin/sh\n\
         # Written by Try Tessaro on every Open Terminal.\n\
         export PATH=\"{bin}:$PATH\"\n\
         export TESSARO_NODE={node}\n\
         clear\n\
         printf '\\033[1mTry Tessaro\\033[0m - tessaro-ctl talks to the device at %s\\n\\n' \"$TESSARO_NODE\"\n\
         printf '  tessaro-ctl device status     what it is doing\\n'\n\
         printf '  tessaro-ctl --help            everything else\\n\\n'\n\
         exec \"${{SHELL:-/bin/zsh}}\" -l\n",
        bin = quoted(bin),
    )
}

/// `text` safe inside double quotes in sh.
fn quoted(text: &str) -> String {
    text.chars()
        .flat_map(|c| match c {
            '"' | '\\' | '$' | '`' => vec!['\\', c],
            c => vec![c],
        })
        .collect()
}

/// Webconfig in the default browser, signed in on a claimed device this
/// computer holds a token for (`webconfig::address`).
pub fn webconfig(api: u16) -> Result<(), String> {
    let mut session = device::open(api)?;
    let address = webconfig::address(&mut session)?;
    webconfig::open(&address.url)
}

pub fn url(url: &str) -> Result<(), String> {
    webconfig::open(url)
}

/// The data directory in Finder: the logs and the disk.
pub fn folder(dir: &Path) -> Result<(), String> {
    spawn(Command::new("open").arg(dir))
}

fn spawn(command: &mut Command) -> Result<(), String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(drop)
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_terminal_has_the_ctl_and_the_node() {
        let script = terminal_script(
            "/Applications/Try Tessaro.app/Contents/Resources/bin",
            "127.0.0.1:7401",
        );
        assert!(script.contains(
            "export PATH=\"/Applications/Try Tessaro.app/Contents/Resources/bin:$PATH\""
        ));
        assert!(script.contains("export TESSARO_NODE=127.0.0.1:7401\n"));
        assert!(script.ends_with("exec \"${SHELL:-/bin/zsh}\" -l\n"));
    }

    #[test]
    fn a_path_cannot_break_out_of_its_quotes() {
        assert_eq!(quoted("a\"b$c`d\\e"), "a\\\"b\\$c\\`d\\\\e");
    }
}
