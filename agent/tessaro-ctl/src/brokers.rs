//! `tessaro-ctl nodes brokers ...`: the MQTT brokers this machine reaches
//! devices through, kept by a name so `--mqtt @NAME` stands for one
//! (`tessaro_client::brokers`). Needs no device.

use std::path::PathBuf;

use crate::out::println;
use clap::Subcommand;
use tessaro_client::brokers::{self, Broker, Brokers};

use crate::style::{self, pad, paint};
use crate::{print, print_json, prompt};

#[derive(Subcommand)]
pub enum BrokersCmd {
    /// The brokers kept on this machine.
    List,
    /// Keep a broker as NAME: then `--mqtt @NAME` reaches its devices.
    /// URL is mqtt://, mqtts://, ws:// or wss://host[:port][/path]; a
    /// user:password@ in it is taken out and kept like --username and a
    /// password.
    ///
    ///   tessaro-ctl nodes brokers add venue1 mqtts://broker.example --username ops --password-stdin
    ///   tessaro-ctl --mqtt @venue1 nodes list
    Add {
        name: String,
        url: String,
        #[command(flatten)]
        fields: Fields,
    },
    /// Change what is given of a kept broker.
    ///
    ///   tessaro-ctl nodes brokers set venue1 --prefix acme/venue1
    Set {
        name: String,
        /// Another URL.
        #[arg(long)]
        url: Option<String>,
        #[command(flatten)]
        fields: Fields,
        /// Forget its password.
        #[arg(long, conflicts_with_all = ["password", "password_stdin"])]
        no_password: bool,
        /// Forget its certificate authority.
        #[arg(long, conflicts_with = "ca")]
        no_ca: bool,
        /// Forget its client certificate.
        #[arg(long, conflicts_with_all = ["cert", "key"])]
        no_cert: bool,
    },
    /// Forget a kept broker.
    Remove { name: String },
}

/// What a broker is reached with.
#[derive(clap::Args)]
pub struct Fields {
    /// The user name the broker knows this machine by.
    #[arg(long)]
    username: Option<String>,
    /// Ask for the broker's password.
    #[arg(long, conflicts_with = "password_stdin")]
    password: bool,
    /// Read the broker's password from stdin.
    #[arg(long)]
    password_stdin: bool,
    /// The devices' mqtt.prefix there (default tessaro).
    #[arg(long)]
    prefix: Option<String>,
    /// A certificate authority to trust for the broker, a PEM file, beside
    /// this machine's own.
    #[arg(long, value_name = "FILE")]
    ca: Option<PathBuf>,
    /// A client certificate, PEM, for a broker that wants one; with --key.
    #[arg(long, value_name = "FILE", requires = "key")]
    cert: Option<PathBuf>,
    /// Its private key, PEM.
    #[arg(long, value_name = "FILE", requires = "cert")]
    key: Option<PathBuf>,
}

fn read(path: &PathBuf) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))
}

impl Fields {
    /// What was given, onto `broker`.
    fn apply(self, broker: &mut Broker) -> Result<(), String> {
        if let Some(username) = self.username {
            broker.username = Some(username).filter(|name| !name.is_empty());
        }
        if self.password || self.password_stdin {
            let password = prompt::password(self.password_stdin, "the broker's password: ")?;
            broker.password = Some(password).filter(|password| !password.is_empty());
        }
        if let Some(prefix) = self.prefix {
            broker.prefix = prefix;
        }
        if let Some(ca) = &self.ca {
            broker.ca = Some(read(ca)?);
        }
        if let (Some(cert), Some(key)) = (&self.cert, &self.key) {
            broker.client_cert = Some(read(cert)?);
            broker.client_key = Some(read(key)?);
        }
        Ok(())
    }
}

pub fn run(command: BrokersCmd, json: bool) -> Result<(), String> {
    let mut kept = Brokers::load()?;
    match command {
        BrokersCmd::List => {
            if json {
                let list: Vec<serde_json::Value> = kept
                    .brokers
                    .iter()
                    .map(|broker| {
                        serde_json::json!({
                            "name": broker.name,
                            "url": broker.url,
                            "username": broker.username,
                            "password": broker.password.is_some(),
                            "prefix": broker.prefix,
                            "ca": broker.ca.is_some(),
                            "client_cert": broker.client_cert.is_some(),
                        })
                    })
                    .collect();
                return print_json(&list);
            }
            if kept.brokers.is_empty() {
                println!(
                    "{} {}",
                    paint(style::MUTED, "no brokers kept;"),
                    paint(style::CMD, "tessaro-ctl nodes brokers add NAME URL")
                );
            }
            for broker in &kept.brokers {
                let mut notes = Vec::new();
                if let Some(user) = &broker.username {
                    notes.push(format!("user {user}"));
                }
                if broker.password.is_some() {
                    notes.push("password".to_string());
                }
                if broker.ca.is_some() {
                    notes.push("ca".to_string());
                }
                if broker.client_cert.is_some() {
                    notes.push("client cert".to_string());
                }
                println!(
                    "{} {:<36} {} {}",
                    pad(style::HEADING, format!("@{}", broker.name), 18),
                    broker.url,
                    pad(style::MUTED, format!("prefix {}", broker.prefix), 22),
                    paint(style::MUTED, notes.join(", "))
                );
            }
            Ok(())
        }
        BrokersCmd::Add { name, url, fields } => {
            brokers::check_name(&name)?;
            if kept.by_name(&name).is_some() {
                return Err(format!("a broker named {name} is kept already; `tessaro-ctl nodes brokers set {name}` changes it"));
            }
            let (url, username, password) = brokers::split_credentials(&url)?;
            let mut broker = Broker {
                name,
                url,
                username,
                password,
                prefix: protocol::mqtt::topic::DEFAULT_PREFIX.to_string(),
                ca: None,
                client_cert: None,
                client_key: None,
            };
            fields.apply(&mut broker)?;
            let label = broker.label();
            kept.put(broker)?;
            print(json, &serde_json::json!({ "kept": label }), || {
                println!(
                    "{} {}",
                    paint(style::OK, format!("kept {label};")),
                    paint(style::CMD, format!("tessaro-ctl --mqtt {label} nodes list"))
                );
            })
        }
        BrokersCmd::Set {
            name,
            url,
            fields,
            no_password,
            no_ca,
            no_cert,
        } => {
            let mut broker = kept.by_name(&name).cloned().ok_or_else(|| {
                format!("no broker named {name}; `tessaro-ctl nodes brokers list` lists them")
            })?;
            if let Some(url) = url {
                let (url, username, password) = brokers::split_credentials(&url)?;
                broker.url = url;
                if username.is_some() {
                    broker.username = username;
                    broker.password = password;
                }
            }
            if no_password {
                broker.password = None;
            }
            if no_ca {
                broker.ca = None;
            }
            if no_cert {
                broker.client_cert = None;
                broker.client_key = None;
            }
            fields.apply(&mut broker)?;
            let label = broker.label();
            kept.put(broker)?;
            print(json, &serde_json::json!({ "changed": label }), || {
                println!("{}", paint(style::OK, format!("changed {label}")))
            })
        }
        BrokersCmd::Remove { name } => {
            if !kept.remove(&name)? {
                return Err(format!("no broker named {name}"));
            }
            print(json, &serde_json::json!({ "removed": name }), || {
                println!("forgot @{name}")
            })
        }
    }
}
