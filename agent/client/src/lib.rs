//! The client side of the Tessaro API, for every program that manages
//! devices: finding them, opening a pinned session that calls the endpoints
//! of `protocol::api` by type, the known nodes this machine keeps about
//! them - and everything a command does around its requests, so
//! `tessaro-ctl` and `tessaro-gui` do it once (docs/clients.md).
//!
//! Nothing in here prints or asks. Where the user has to decide - pinning a
//! certificate seen for the first time - the caller passes the decision in
//! (`connect::Trust::Pin`); a flow reports its progress to a
//! `report::Report`; and what is worth telling the user comes back as
//! `Session::notes` or as `text::Line`s, which each client paints its own
//! way.

pub mod access;
pub mod actions;
pub mod bulk;
pub mod camera;
pub mod cec;
pub mod certs;
pub mod clock;
pub mod config;
pub mod connect;
pub mod describe;
pub mod devtools;
pub mod files;
mod http;
pub mod journal;
pub mod network;
pub mod nodes;
pub mod ping;
pub mod playlist;
pub mod policies;
pub mod printer;
pub mod report;
pub mod schedule;
pub mod script;
pub mod sections;
pub mod speedtest;
pub mod ssh;
pub mod storage;
pub mod store;
pub mod tags;
pub mod text;
pub mod transfer;
pub mod tunnel;
pub mod update;
pub mod webconfig;

/// The same mDNS library `connect::browse` uses, for a caller that browses
/// for good instead (the GUI) and hands results to `connect::found_service`.
pub use mdns_sd;

/// Who is claiming a device, for its token list: `user@host`.
pub fn client_name() -> String {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "someone".to_string());
    let host = std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|host| host.trim().to_string())
        .filter(|host| !host.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .unwrap_or_else(|| "a laptop".to_string());
    format!("{user}@{host}")
}
