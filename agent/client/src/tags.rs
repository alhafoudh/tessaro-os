//! A device's tags: device.tags as announced, stored and edited, plus the
//! reserved `unclaimed` every client adds to a device nobody has claimed, so
//! fresh devices can be picked out together (`nodes list --tag unclaimed`).
//!
//! What a tag looks like is the setting's rule (`keys::parse_tags`); this
//! is what the clients do around it, once for all of them.

use std::collections::BTreeMap;

use protocol::keys::{self, UNCLAIMED_TAG};
use protocol::{api, Applied};

use crate::connect::Session;

/// How many colours a tag's badge is picked from (`colour`). Each client
/// keeps a palette this long, and Webconfig's port of `colour` the same.
pub const COLOURS: usize = 8;

/// Tags from a comma-separated value as the device announces or the store
/// keeps them. Not checked: the device checked them when they were set.
pub fn split(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(str::to_string)
        .collect()
}

/// The tags a client shows and filters by: `unclaimed` first when the
/// device says nobody has claimed it, then its own. A device whose claim is
/// not known (`None`) gets no `unclaimed`.
pub fn effective(tags: &[String], claimed: Option<bool>) -> Vec<String> {
    let mut all = Vec::with_capacity(tags.len() + 1);
    if claimed == Some(false) {
        all.push(UNCLAIMED_TAG.to_string());
    }
    all.extend(tags.iter().filter(|tag| *tag != UNCLAIMED_TAG).cloned());
    all
}

/// Whether a device with `tags` has every one of `wanted`. Nothing wanted
/// matches every device.
pub fn matches(tags: &[String], wanted: &[String]) -> bool {
    wanted
        .iter()
        .all(|want| tags.iter().any(|tag| tag.eq_ignore_ascii_case(want)))
}

/// Whether `tag` is the one that follows the claim rather than a setting.
pub fn is_reserved(tag: &str) -> bool {
    tag == UNCLAIMED_TAG
}

/// Which of the `COLOURS` a tag's badge has: the same tag the same colour
/// on every client, picked by FNV-1a over its bytes. `unclaimed` is drawn in
/// the warning colour instead; see `is_reserved`.
pub fn colour(tag: &str) -> usize {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in tag.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    (hash % COLOURS as u32) as usize
}

/// An edit of a device's tags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    Add(Vec<String>),
    Remove(Vec<String>),
}

/// `current` with `edit` made, as device.tags would store it: checked,
/// sorted, without repeats. Removing a tag the device does not have is not
/// an error.
pub fn edited(current: &[String], edit: &Edit) -> Result<Vec<String>, String> {
    let next: Vec<String> = match edit {
        Edit::Add(tags) => current.iter().chain(tags).cloned().collect(),
        Edit::Remove(tags) => {
            let gone: Vec<String> = tags.iter().map(|tag| tag.to_ascii_lowercase()).collect();
            current
                .iter()
                .filter(|tag| !gone.contains(tag))
                .cloned()
                .collect()
        }
    };
    keys::parse_tags(&next.join(","))
}

/// What `apply` did: the tags after it, and the change when anything moved.
#[derive(Debug, Clone)]
pub struct Tagged {
    pub tags: Vec<String>,
    pub applied: Option<Applied>,
}

/// The device's tags, as it has them now.
pub fn read(session: &mut Session) -> Result<(u64, Vec<String>), String> {
    let settings = session.call::<api::config::Get>(
        api::ConfigQuery {
            key: Some(keys::TAGS.to_string()),
        },
        (),
    )?;
    let value = settings
        .settings
        .iter()
        .find(|setting| setting.key == keys::TAGS)
        .and_then(|setting| setting.value.clone())
        .unwrap_or_default();
    Ok((settings.revision, split(&value)))
}

/// Make `edit` on the device: read its tags, edit them, and set the result
/// at the revision read, so an edit made meanwhile by another client is
/// refused rather than lost. Nothing is sent when nothing changes.
pub fn apply(session: &mut Session, edit: &Edit) -> Result<Tagged, String> {
    let (revision, current) = read(session)?;
    let tags = edited(&current, edit)?;
    if tags == current {
        return Ok(Tagged {
            tags,
            applied: None,
        });
    }
    let applied = session.send::<api::config::Set>(api::SetConfig {
        values: BTreeMap::from([(keys::TAGS.to_string(), tags.join(","))]),
        if_revision: Some(revision),
        apply: true,
        verify: Default::default(),
    })?;
    Ok(Tagged {
        tags,
        applied: Some(applied),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(list: &[&str]) -> Vec<String> {
        list.iter().map(|tag| tag.to_string()).collect()
    }

    #[test]
    fn an_announced_value_splits_without_empty_tags() {
        assert_eq!(split("floor-2, lobby,,"), tags(&["floor-2", "lobby"]));
        assert!(split("").is_empty());
    }

    #[test]
    fn unclaimed_comes_first_and_only_when_the_device_says_so() {
        let own = tags(&["lobby"]);
        assert_eq!(effective(&own, Some(false)), tags(&["unclaimed", "lobby"]));
        assert_eq!(effective(&own, Some(true)), own);
        assert_eq!(effective(&own, None), own);
    }

    #[test]
    fn a_filter_wants_every_tag_it_names() {
        let have = tags(&["unclaimed", "floor-2", "lobby"]);
        assert!(matches(&have, &[]));
        assert!(matches(&have, &tags(&["lobby", "Unclaimed"])));
        assert!(!matches(&have, &tags(&["lobby", "menu"])));
    }

    #[test]
    fn edits_keep_the_setting_rules() {
        let current = tags(&["lobby"]);
        assert_eq!(
            edited(&current, &Edit::Add(tags(&["Floor-2", "lobby"]))).unwrap(),
            tags(&["floor-2", "lobby"])
        );
        assert_eq!(
            edited(&current, &Edit::Remove(tags(&["LOBBY", "menu"]))).unwrap(),
            Vec::<String>::new()
        );
        assert!(edited(&current, &Edit::Add(tags(&["unclaimed"]))).is_err());
        assert!(edited(&current, &Edit::Add(tags(&["no_underscore"]))).is_err());
    }

    #[test]
    fn a_tag_keeps_its_colour() {
        assert_eq!(colour("lobby"), colour("lobby"));
        assert!((0..100)
            .map(|n| colour(&format!("tag-{n}")))
            .all(|c| c < COLOURS));
    }
}
