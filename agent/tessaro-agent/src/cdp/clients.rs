//! Who else is on the DevTools port: a technician's DevTools through an SSH
//! tunnel (`tessaro-ctl browser devtools`), or anything on the device itself.
//!
//! Chromium does not say. `/json/list` has no client count, and
//! `Target.getTargets` reports `attached`, which the agent's own session
//! always makes true. So the kernel is asked instead, through
//! `crate::loopback`: the connections to the port that are not the agent's
//! own session belong to someone else.

pub use crate::loopback::foreign;

/// The port a DevTools base URL (`http://127.0.0.1:9222`) points at.
pub fn port_of(base_url: &str) -> Option<u16> {
    let rest = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest);
    let authority = rest.split('/').next()?;
    authority.rsplit_once(':')?.1.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_port_comes_from_the_base_url() {
        assert_eq!(port_of("http://127.0.0.1:9222"), Some(9222));
        assert_eq!(port_of("http://127.0.0.1:9333/"), Some(9333));
        assert_eq!(port_of("http://chromium:9222/json"), Some(9222));
        assert_eq!(port_of("http://127.0.0.1"), None);
    }
}
