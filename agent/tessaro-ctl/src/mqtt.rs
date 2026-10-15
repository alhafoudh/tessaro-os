//! `tessaro-ctl mqtt ...`: the device's link to its MQTT broker - how it
//! stands, a test of it, and the password and client certificate it
//! connects with. Where it connects is the mqtt.* settings
//! (`tessaro-ctl config set mqtt.url=...`); the words are
//! `tessaro_client::describe::mqtt`, shared with the GUI's MQTT page.
//!
//! Reaching a device through a broker is the global `--mqtt`, not this
//! group (`tessaro-ctl nodes brokers`).

use std::path::PathBuf;

use crate::out::println;
use clap::Subcommand;
use protocol::api;
use protocol::mqtt::{MqttCertBody, MqttPasswordBody, MqttTestBody};
use protocol::Secret;
use tessaro_client::describe::mqtt as describe;

use crate::connect::Session;
use crate::style;
use crate::{done, print, prompt};

#[derive(Subcommand)]
pub enum MqttCmd {
    /// The link: connected or not and why, the broker, the topics the
    /// device publishes under, what it is subscribed to and what it sent.
    Status,
    /// Connect to the broker with the saved settings, or with URL instead
    /// of mqtt.url, and say how far it got: the DNS, the connect, the
    /// proxy, TLS, the WebSocket, MQTT. Prints the broker certificate's
    /// SHA-256 for mqtt.tls.fingerprint. Nothing is saved.
    ///
    ///   tessaro-ctl mqtt test mqtts://broker.example:8883
    Test { url: Option<String> },
    /// The password the device gives the broker, kept as a secret and never
    /// shown; {key} placeholders are filled in, as in mqtt.username.
    #[command(subcommand)]
    Password(PasswordCmd),
    /// A client certificate the device shows the broker, for a broker that
    /// wants one (mutual TLS).
    #[command(subcommand)]
    Cert(CertCmd),
}

#[derive(Subcommand)]
pub enum PasswordCmd {
    /// Set it: prompted, or on stdin.
    ///
    ///   tessaro-ctl mqtt password set
    ///   printf %s "$PW" | tessaro-ctl mqtt password set --password-stdin
    Set {
        /// Read the password from stdin instead of prompting.
        #[arg(long)]
        password_stdin: bool,
    },
    /// Forget it: the device connects without one.
    Clear,
}

#[derive(Subcommand)]
pub enum CertCmd {
    /// Keep a certificate and its private key, both PEM files.
    ///
    ///   tessaro-ctl mqtt cert set --cert device.pem --key device.key
    Set {
        #[arg(long, value_name = "FILE")]
        cert: PathBuf,
        #[arg(long, value_name = "FILE")]
        key: PathBuf,
    },
    /// Forget them: the device connects without one.
    Clear,
}

pub fn run(session: &mut Session, command: MqttCmd, json: bool) -> Result<(), String> {
    match command {
        MqttCmd::Status => {
            let status = session.fetch::<api::mqtt::Status>()?;
            print(json, &status, || style::facts(&describe::facts(&status)))
        }
        MqttCmd::Test { url } => {
            let tested = session.send::<api::mqtt::Test>(MqttTestBody { url })?;
            print(json, &tested, || {
                for line in describe::tested(&tested) {
                    println!("{}", style::line(&line));
                }
            })?;
            match tested.ok {
                true => Ok(()),
                false => Err("the broker did not take the connection".to_string()),
            }
        }
        MqttCmd::Password(PasswordCmd::Set { password_stdin }) => {
            let password = prompt::password(password_stdin, "the broker's password: ")?;
            if password.is_empty() {
                return Err(
                    "the password is empty; `tessaro-ctl mqtt password clear` forgets it"
                        .to_string(),
                );
            }
            let body = MqttPasswordBody {
                password: Some(Secret(password)),
            };
            done::<api::mqtt::Password>(session, api::Empty {}, body, json)
        }
        MqttCmd::Password(PasswordCmd::Clear) => done::<api::mqtt::Password>(
            session,
            api::Empty {},
            MqttPasswordBody { password: None },
            json,
        ),
        MqttCmd::Cert(CertCmd::Set { cert, key }) => {
            let read = |path: &PathBuf| {
                std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))
            };
            let body = MqttCertBody {
                cert: read(&cert)?,
                key: Secret(read(&key)?),
            };
            done::<api::mqtt::CertSet>(session, api::Empty {}, body, json)
        }
        MqttCmd::Cert(CertCmd::Clear) => {
            done::<api::mqtt::CertClear>(session, api::Empty {}, (), json)
        }
    }
}
