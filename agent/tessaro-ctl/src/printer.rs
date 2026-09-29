//! `tessaro-ctl printer ...`: the printers the device prints on, through
//! its own CUPS.
//!
//! The device keeps the printers and sets each up as a CUPS queue: a
//! driverless one (`ipp`) is asked what it takes when it is created, a raw
//! one gets the bytes as they are. printer.enable decides whether the page
//! may print - `window.print()` to the default printer, the page bridge to
//! any; the commands here print either way.

use std::path::PathBuf;

use anstream::println;
use clap::Subcommand;
use protocol::api::{self, PrintJobRef, PrintJobsQuery, PrinterRef};
use protocol::{PrinterKind, PrinterSpec, PRINT_COPIES_MAX};
use tessaro_client::describe::printer as describe;
use tessaro_client::printer::{self as shared, Options};

use crate::connect::Session;
use crate::style;
use crate::{done, print, prompt};

#[derive(Subcommand)]
pub enum PrinterCmd {
    /// Every printer, its state and waiting jobs, the default marked, and
    /// whether the page may print.
    List,
    /// One printer in full: what CUPS says about it, and its ink or toner
    /// when it reports them.
    Show { printer: String },
    /// Look for printers on USB and the network, each with the URI to
    /// create it by.
    Discover,
    /// Add a printer. A driverless one must answer now: the device asks it
    /// what it takes.
    ///
    ///   tessaro-ctl printer create office --uri ipp://10.0.0.5/ipp/print
    ///   tessaro-ctl printer create receipt --raw --uri socket://10.0.0.9:9100
    ///   tessaro-ctl printer create label --raw --uri 'usb://Zebra/ZD421?serial=D4J2'
    Create {
        /// Lower-case letters, digits, - and _.
        name: String,
        /// Where it is: ipp://, ipps://, socket://, usb://, or a dnssd://
        /// URI from `tessaro-ctl printer discover`.
        #[arg(long)]
        uri: String,
        /// Send documents as they are, with no driver: a receipt printer's
        /// ESC/POS, a label printer's ZPL.
        #[arg(long)]
        raw: bool,
        /// The paper a job gets when it names none, as the printer names it:
        /// iso_a4_210x297mm, na_letter_8.5x11in.
        #[arg(long, value_name = "PAPER")]
        media: Option<String>,
    },
    /// Remove a printer and the jobs it still holds.
    Remove {
        printer: String,
        #[arg(long, short)]
        yes: bool,
    },
    /// Make a printer the default: the one `window.print()` prints on, and
    /// the page bridge's when it names none.
    Default { printer: String },
    /// Print a test page: a PDF for a driverless printer, a few lines of
    /// text for a raw one.
    Test { printer: String },
    /// Print a document: a PDF for a driverless printer, the printer's own
    /// bytes for a raw one.
    ///
    ///   tessaro-ctl printer print office ticket.pdf --copies 2
    ///   tessaro-ctl printer print receipt --stored receipts/today.bin
    Print {
        printer: String,
        /// A file on this machine, sent with the request.
        #[arg(required_unless_present = "stored", conflicts_with = "stored")]
        file: Option<PathBuf>,
        /// A file already in the device's store, by its path there: for one
        /// too large to send with the request.
        #[arg(long, value_name = "PATH")]
        stored: Option<String>,
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=PRINT_COPIES_MAX as i64))]
        copies: Option<u32>,
        /// The paper, as the printer names it.
        #[arg(long, value_name = "PAPER")]
        media: Option<String>,
        /// What CUPS calls the job; the file's name by default.
        #[arg(long)]
        title: Option<String>,
    },
    /// The jobs not printed yet, of one printer or all of them.
    Jobs { printer: Option<String> },
    /// Cancel a job, by its id from `tessaro-ctl printer jobs`.
    Cancel { job: String },
}

pub fn run(session: &mut Session, command: PrinterCmd, json: bool) -> Result<(), String> {
    match command {
        PrinterCmd::List => {
            let list = session.fetch::<api::printer::List>()?;
            print(json, &list, || lines(describe::list(&list)))
        }
        PrinterCmd::Show { printer } => {
            let info = session.call::<api::printer::Show>(PrinterRef { printer }, ())?;
            print(json, &info, || style::facts(&describe::show(&info)))
        }
        PrinterCmd::Discover => {
            if !json {
                println!(
                    "{}",
                    style::paint(style::MUTED, "looking on USB and the network...")
                );
            }
            let found = shared::discover(session, &|| false, |printer| {
                if !json {
                    lines(describe::found(printer));
                }
            })?;
            print(json, &found, || {
                println!();
                println!("{}", style::line(&describe::found_hint(&found)));
            })
        }
        PrinterCmd::Create {
            name,
            uri,
            raw,
            media,
        } => {
            let spec = PrinterSpec {
                name,
                uri,
                kind: if raw {
                    PrinterKind::Raw
                } else {
                    PrinterKind::Ipp
                },
                media,
            };
            let info = session.send::<api::printer::Create>(spec)?;
            print(json, &info, || {
                println!(
                    "{}",
                    style::paint(style::OK, format!("created {}", info.spec.name))
                );
                style::facts(&describe::show(&info));
            })
        }
        PrinterCmd::Remove { printer, yes } => {
            prompt::confirm(yes, &format!("Remove printer {printer} and its jobs?"))?;
            done::<api::printer::Remove>(session, PrinterRef { printer }, (), json)
        }
        PrinterCmd::Default { printer } => {
            done::<api::printer::SetDefault>(session, PrinterRef { printer }, (), json)
        }
        PrinterCmd::Test { printer } => {
            let queued = session.call::<api::printer::Test>(PrinterRef { printer }, ())?;
            print(json, &queued, || lines(vec![describe::queued(&queued)]))
        }
        PrinterCmd::Print {
            printer,
            file,
            stored,
            copies,
            media,
            title,
        } => {
            let options = Options {
                copies,
                media,
                title,
            };
            let queued = match (file, stored) {
                (_, Some(path)) => shared::print_stored(session, &printer, &path, options)?,
                (Some(file), None) => shared::print_file(session, &printer, &file, options)?,
                (None, None) => return Err("print a FILE or --stored PATH".to_string()),
            };
            print(json, &queued, || lines(vec![describe::queued(&queued)]))
        }
        PrinterCmd::Jobs { printer } => {
            let jobs = session.call::<api::printer::Jobs>(PrintJobsQuery { printer }, ())?;
            print(json, &jobs, || lines(describe::jobs(&jobs)))
        }
        PrinterCmd::Cancel { job } => {
            done::<api::printer::Cancel>(session, PrintJobRef { job }, (), json)
        }
    }
}

fn lines(lines: Vec<tessaro_client::text::Line>) {
    for line in lines {
        println!("{}", style::line(&line));
    }
}
