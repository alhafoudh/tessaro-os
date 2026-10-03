//! Settings changed in one step, with no revision check: what a shorthand
//! command (`browser maintenance on`, `audio volume 40`) or a one-click
//! action (Try Tessaro's activities) is. A form that edits what it read
//! sends its revision itself (`SetConfig::if_revision`).

use std::collections::BTreeMap;

use protocol::{api, Applied};

use crate::connect::Session;

/// `KEY=VALUE ...` set and applied.
pub fn set(session: &mut Session, values: BTreeMap<String, String>) -> Result<Applied, String> {
    session.send::<api::config::Set>(api::SetConfig {
        values,
        if_revision: None,
        apply: true,
        verify: Default::default(),
    })
}

/// `KEYS` back to their defaults, applied.
pub fn unset(session: &mut Session, keys: Vec<String>) -> Result<Applied, String> {
    session.send::<api::config::Unset>(api::UnsetConfig {
        keys,
        if_revision: None,
        apply: true,
        verify: Default::default(),
    })
}
