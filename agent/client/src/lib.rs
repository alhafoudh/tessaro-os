//! The client side of the Tessaro control protocol, for every program that
//! manages devices: finding them, opening a pinned session, and the
//! `nodes.json` this machine keeps about them.
//!
//! Nothing in here prints or asks. Where the user has to decide - pinning a
//! certificate seen for the first time - the caller passes the decision in
//! (`connect::Trust::Pin`), and what is worth telling the user comes back as
//! `Session::notes`.

pub mod certs;
pub mod clock;
pub mod connect;
pub mod journal;
pub mod nodes;
pub mod ssh;
pub mod transfer;
pub mod tunnel;

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
