//! `tessaro-ctl screen cec ...`: the TV and everything else on the HDMI-CEC
//! bus, acted on without switching the screen (docs/cec.md, Actions).
//!
//! Each action goes out on every adapter, or the one on `--connector`, and
//! answers with what each sent and whether it was acknowledged. `messages`
//! reads the device's log of the bus, both ways, and with `-f` follows it.

use crate::out::println;
use clap::{Args, Subcommand};
use protocol::api;
use protocol::CecActed;
use tessaro_client::cec;
use tessaro_client::describe::cec as describe;

use crate::connect::Session;
use crate::style;
use crate::{print, print_json};

#[derive(Subcommand)]
pub enum CecCmd {
    /// Wake the TV and switch it to the device's input, without touching
    /// the screen. Refused while the screen is off: `screen power on` wakes
    /// both.
    Wake {
        /// Leave the TV on the input it had.
        #[arg(long)]
        no_source: bool,
        #[command(flatten)]
        on: On,
    },
    /// Put the TV in standby, the screen staying as it is.
    ///
    ///   tessaro-ctl screen cec standby --all
    Standby {
        /// Everything on the bus: a sound bar, a receiver, the TV.
        #[arg(long)]
        all: bool,
        #[command(flatten)]
        on: On,
    },
    /// Switch the TV to the device's input.
    Source {
        #[command(flatten)]
        on: On,
    },
    /// Press and let go a key of the TV remote: volume-up, volume-down,
    /// mute, select, the arrows, the colour keys...
    ///
    ///   tessaro-ctl screen cec key volume-up --to audio
    Key {
        /// The key's name, as `screen.cec` events name it.
        key: String,
        /// Where it goes: tv (the default), audio, or 0 to 15.
        #[arg(long, value_name = "ADDRESS", default_value = "")]
        to: String,
        #[command(flatten)]
        on: On,
    },
    /// Poll the bus now and ask whoever answers what they are, for
    /// `screen show`.
    Scan {
        #[command(flatten)]
        on: On,
    },
    /// Send any message: the opcode and its operands in hex. With --reply,
    /// wait up to 2s for that opcode back.
    ///
    ///   tessaro-ctl screen cec send 8f --to tv --reply 90
    Send {
        /// The opcode and operands: 8f, 44 41, 0x89,0x01.
        data: String,
        /// Where it goes: tv, audio, all, or 0 to 15.
        #[arg(long, value_name = "ADDRESS")]
        to: String,
        /// An opcode to wait for from that address.
        #[arg(long, value_name = "OPCODE", default_value = "")]
        reply: String,
        #[command(flatten)]
        on: On,
    },
    /// What went over the bus, both ways, as the device's log has it: the
    /// last 500 messages.
    Messages {
        /// Keep printing messages as they come, until Ctrl-C.
        #[arg(long, short)]
        follow: bool,
    },
}

/// Which adapter an action goes out on.
#[derive(Args)]
pub struct On {
    /// Only the adapter on this connector (HDMI-A-2), as `screen show`
    /// names it; every adapter without it.
    #[arg(long, value_name = "CONNECTOR", default_value = "")]
    connector: String,
}

pub fn run(session: &mut Session, command: CecCmd, json: bool) -> Result<(), String> {
    let acted = match command {
        CecCmd::Wake { no_source, on } => {
            session.send::<api::screen::CecWake>(cec::wake_body(!no_source, &on.connector))?
        }
        CecCmd::Standby { all, on } => {
            session.send::<api::screen::CecStandby>(cec::standby_body(all, &on.connector))?
        }
        CecCmd::Source { on } => session.send::<api::screen::CecSource>(cec::on(&on.connector))?,
        CecCmd::Key { key, to, on } => {
            session.send::<api::screen::CecKey>(cec::key_body(&key, &to, &on.connector)?)?
        }
        CecCmd::Scan { on } => session.send::<api::screen::CecScan>(cec::on(&on.connector))?,
        CecCmd::Send {
            data,
            to,
            reply,
            on,
        } => session.send::<api::screen::CecSend>(cec::send_body(
            &data,
            &to,
            &reply,
            &on.connector,
        )?)?,
        CecCmd::Messages { follow } => {
            return cec::messages(session, 0, follow, &|| false, |message| {
                if json {
                    let _ = print_json(message);
                } else {
                    println!("{}", style::line(&describe::message(message)));
                }
            });
        }
    };
    print_acted(&acted, json)
}

fn print_acted(acted: &CecActed, json: bool) -> Result<(), String> {
    print(json, acted, || {
        for line in describe::acted(acted) {
            println!("{}", style::line(&line));
        }
    })
}
