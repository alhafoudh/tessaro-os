//! `tessaro-ctl scanner ...`: the barcode scanners the device reads.
//!
//! A scanner is a USB device read one of three ways: a keyboard, whose keys
//! the device takes from the page and the screen, a serial port, or a HID
//! POS device. Its scans become events for the page and for scripts.
//! scanner.enable decides whether any is read; the commands here set them
//! up either way.

use crate::out::println;
use clap::{Args, Subcommand};
use protocol::api::{self, ScannerRef};
use protocol::scanner::ScannerChange;
use tessaro_client::describe::scanner as describe;
use tessaro_client::scanner::{self as shared, Typed};

use crate::connect::Session;
use crate::style;
use crate::{done, print, print_json_line, prompt};

#[derive(Subcommand)]
pub enum ScannerCmd {
    /// Every scanner, its state and its device, and whether scanners are
    /// read.
    List,
    /// One scanner in full: its device, how its scans end, its last scan.
    Show { scanner: String },
    /// The USB keyboards, serial ports and HID POS devices plugged in, each
    /// with the device id to create a scanner by.
    Discover,
    /// Wait for a scan on any of them and name the device it came from,
    /// with what it scanned. Nothing is taken from the page while it
    /// listens.
    Identify,
    /// Add a scanner, by a device id from `tessaro-ctl scanner discover` or
    /// `tessaro-ctl scanner identify`.
    ///
    ///   tessaro-ctl scanner create front --device keyboard:0c2e:0b61:@1-1.2
    ///   tessaro-ctl scanner create till --device serial:1a86:7523:@1-1.3 --terminator lf --baud 115200
    ///   tessaro-ctl scanner create door --device keyboard:0c2e:0b61:S12345 --layout de
    Create {
        /// Lower-case letters, digits, - and _.
        name: String,
        /// Which USB device, as discover names it.
        #[arg(long, value_name = "DEVICE")]
        device: String,
        #[command(flatten)]
        settings: Settings,
        /// Create it switched off.
        #[arg(long)]
        disabled: bool,
    },
    /// Change how a scanner is read; an empty value goes back to the
    /// default.
    ///
    ///   tessaro-ctl scanner set till --terminator crlf
    ///   tessaro-ctl scanner set front --layout ''
    Set {
        scanner: String,
        #[command(flatten)]
        settings: Settings,
    },
    /// Read a scanner again.
    Enable { scanner: String },
    /// Stop reading a scanner: a keyboard one types into the page again.
    Disable { scanner: String },
    /// Remove a scanner: its device goes back to whoever else reads it.
    Remove {
        scanner: String,
        #[arg(long, short)]
        yes: bool,
    },
    /// Print a scanner's scans with what they say, for a minute or until
    /// Ctrl-C. They reach the page and the scripts as well.
    Test { scanner: String },
    /// What happened to the scanners: plugged in and out, failures, and
    /// each scan's length, never what it said. The last 500 entries.
    Logs {
        /// Keep printing entries as they come, until Ctrl-C.
        #[arg(long, short)]
        follow: bool,
    },
}

/// How a scanner is read.
#[derive(Args)]
pub struct Settings {
    /// Keyboard: the layout the scanner types in, as xkb names it: us (the
    /// default), de, sk(qwerty).
    #[arg(long, value_name = "LAYOUT")]
    layout: Option<String>,
    /// What ends a scan besides a pause. Keyboard: auto (Enter), enter,
    /// tab, none. Serial: auto (CR or LF), cr, lf, crlf, none, or a byte
    /// as 0x03.
    #[arg(long, value_name = "WHAT")]
    terminator: Option<String>,
    /// Milliseconds of quiet that end a scan.
    #[arg(long, value_name = "MS")]
    gap_ms: Option<String>,
    /// Serial: the baud rate, 9600 by default. A USB CDC scanner ignores it.
    #[arg(long, value_name = "RATE")]
    baud: Option<String>,
    /// Text taken off the start of every scan.
    #[arg(long, value_name = "TEXT")]
    strip_prefix: Option<String>,
    /// Text taken off the end of every scan.
    #[arg(long, value_name = "TEXT")]
    strip_suffix: Option<String>,
}

impl Settings {
    fn typed(&self) -> Typed {
        Typed {
            layout: self.layout.clone().unwrap_or_default(),
            terminator: self.terminator.clone().unwrap_or_default(),
            gap_ms: self.gap_ms.clone().unwrap_or_default(),
            baud: self.baud.clone().unwrap_or_default(),
            strip_prefix: self.strip_prefix.clone().unwrap_or_default(),
            strip_suffix: self.strip_suffix.clone().unwrap_or_default(),
        }
    }

    /// Only what was given, `''` included: a given empty value resets.
    fn change(&self) -> Result<ScannerChange, String> {
        let number = |typed: &Option<String>, what: &str| -> Result<Option<u32>, String> {
            match typed.as_deref().map(str::trim) {
                None => Ok(None),
                Some("") => Ok(Some(0)),
                Some(typed) => typed
                    .parse()
                    .map(Some)
                    .map_err(|_| format!("{typed:?} is not a {what}; a whole number")),
            }
        };
        Ok(ScannerChange {
            layout: self.layout.clone(),
            terminator: self.terminator.clone(),
            gap_ms: number(&self.gap_ms, "gap")?,
            baud: number(&self.baud, "baud rate")?,
            strip_prefix: self.strip_prefix.clone(),
            strip_suffix: self.strip_suffix.clone(),
            enabled: None,
        })
    }
}

pub fn run(session: &mut Session, command: ScannerCmd, json: bool) -> Result<(), String> {
    match command {
        ScannerCmd::List => {
            let list = session.fetch::<api::scanner::List>()?;
            print(json, &list, || lines(describe::list(&list)))
        }
        ScannerCmd::Show { scanner } => {
            let info = session.call::<api::scanner::Show>(ScannerRef { scanner }, ())?;
            print(json, &info, || style::facts(&describe::show(&info)))
        }
        ScannerCmd::Discover => {
            let found = shared::discover(session, &|| false, |candidate| {
                if !json {
                    lines(describe::candidate(candidate));
                }
            })?;
            print(json, &found, || {
                println!();
                println!("{}", style::line(&describe::candidates_hint(&found)));
            })
        }
        ScannerCmd::Identify => {
            if !json {
                println!(
                    "{}",
                    style::paint(
                        style::MUTED,
                        "scan a code with the scanner; the device listens for 30 seconds..."
                    )
                );
            }
            let heard = shared::identify(session, &|| false)?;
            print(json, &heard, || lines(describe::identified(heard.as_ref())))
        }
        ScannerCmd::Create {
            name,
            device,
            settings,
            disabled,
        } => {
            let spec = settings.typed().spec(&name, &device, !disabled)?;
            let info = session.send::<api::scanner::Create>(spec)?;
            print(json, &info, || {
                println!(
                    "{}",
                    style::paint(style::OK, format!("created {}", info.spec.name))
                );
                style::facts(&describe::show(&info));
            })
        }
        ScannerCmd::Set { scanner, settings } => {
            let change = settings.change()?;
            changed(session, scanner, change, json)
        }
        ScannerCmd::Enable { scanner } => changed(
            session,
            scanner,
            ScannerChange {
                enabled: Some(true),
                ..ScannerChange::default()
            },
            json,
        ),
        ScannerCmd::Disable { scanner } => changed(
            session,
            scanner,
            ScannerChange {
                enabled: Some(false),
                ..ScannerChange::default()
            },
            json,
        ),
        ScannerCmd::Remove { scanner, yes } => {
            prompt::confirm(yes, &format!("Remove scanner {scanner}?"))?;
            done::<api::scanner::Remove>(session, ScannerRef { scanner }, (), json)
        }
        ScannerCmd::Test { scanner } => {
            if !json {
                println!(
                    "{}",
                    style::paint(
                        style::MUTED,
                        format!("scan with {scanner}; the device shows its scans for a minute...")
                    )
                );
            }
            let scans = shared::test(session, &scanner, &|| false, |scan| {
                if json {
                    let _ = print_json_line(scan);
                } else {
                    println!("{}", style::line(&describe::scan(scan)));
                }
            })?;
            if !json && scans.is_empty() {
                println!("{}", style::paint(style::WARN, "nothing was scanned"));
            }
            Ok(())
        }
        ScannerCmd::Logs { follow } => shared::logs(session, 0, follow, &|| false, |entry| {
            if json {
                let _ = print_json_line(entry);
            } else {
                println!("{}", style::line(&describe::log_entry(entry)));
            }
        }),
    }
}

fn changed(
    session: &mut Session,
    scanner: String,
    change: ScannerChange,
    json: bool,
) -> Result<(), String> {
    let info = session.call::<api::scanner::Change>(ScannerRef { scanner }, change)?;
    print(json, &info, || {
        println!(
            "{}",
            style::paint(style::OK, format!("changed {}", info.spec.name))
        );
        style::facts(&describe::show(&info));
    })
}

fn lines(lines: Vec<tessaro_client::text::Line>) {
    for line in lines {
        println!("{}", style::line(&line));
    }
}
