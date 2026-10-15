//! Where everything lives on the broker. The shape is fixed: only the
//! prefix (`mqtt.prefix`, default `tessaro`) is the operator's, and a
//! device's topics sit under `<prefix>/<device.id>/`, so one wildcard
//! (`<prefix>/+/status`) lists every device under a prefix.

/// The prefix when `mqtt.prefix` is not set.
pub const DEFAULT_PREFIX: &str = "tessaro";

/// The level a device's tag topics hang from: `<prefix>/tag/<tag>/cmd/...`.
/// A device id is hex, so it never collides with it.
pub const TAG: &str = "tag";

/// The level clients take their answers on: `<prefix>/client/<random>/reply`.
pub const CLIENT: &str = "client";

/// A device's topics, from its prefix and id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    prefix: String,
    id: String,
}

/// A command topic taken apart: which endpoint, and the tag it came
/// through, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command<'a> {
    pub group: &'a str,
    pub name: &'a str,
    pub tag: Option<&'a str>,
}

impl Layout {
    pub fn new(prefix: &str, id: &str) -> Layout {
        Layout {
            prefix: prefix.to_string(),
            id: id.to_string(),
        }
    }

    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    fn device(&self, rest: &str) -> String {
        format!("{}/{}/{rest}", self.prefix, self.id)
    }

    /// Retained: online, name, tags; the will says offline.
    pub fn status(&self) -> String {
        self.device("status")
    }

    /// Retained snapshots: `device`, `config`, `mqtt`.
    pub fn state(&self, part: &str) -> String {
        self.device(&format!("state/{part}"))
    }

    pub fn commands(&self) -> String {
        self.device("cmd/#")
    }

    pub fn tag_commands(&self, tag: &str) -> String {
        format!("{}/{TAG}/{tag}/cmd/#", self.prefix)
    }

    pub fn job(&self, job: &str) -> String {
        self.device(&format!("jobs/{job}"))
    }

    pub fn event(&self, source: &str, name: &str) -> String {
        self.device(&format!("event/{}/{}", escape(source), escape(name)))
    }

    pub fn frame(&self) -> String {
        self.device("screen/frame")
    }

    pub fn logs(&self) -> String {
        self.device("logs")
    }

    /// What the page receives: everything under `page/in/`.
    pub fn page_in(&self) -> String {
        self.device("page/in/#")
    }

    /// Where the page publishes `sub`, which `check_sub` has passed.
    pub fn page_out(&self, sub: &str) -> String {
        self.device(&format!("page/out/{sub}"))
    }

    /// The part of a `page/in/...` topic below `page/in/`.
    pub fn page_in_sub<'a>(&self, topic: &'a str) -> Option<&'a str> {
        let head = self.device("page/in/");
        topic
            .strip_prefix(head.as_str())
            .filter(|rest| !rest.is_empty())
    }

    /// The endpoint a message on a command topic asks for, whether it came
    /// to this device or to one of its tags.
    pub fn command<'a>(&self, topic: &'a str) -> Option<Command<'a>> {
        let rest = topic
            .strip_prefix(self.prefix.as_str())?
            .strip_prefix('/')?;
        let (tag, rest) = match rest.split_once('/')? {
            (TAG, rest) => {
                let (tag, rest) = rest.split_once('/')?;
                (Some(tag), rest)
            }
            (id, rest) if id == self.id => (None, rest),
            _ => return None,
        };
        let rest = rest.strip_prefix("cmd/")?;
        let (group, name) = rest.split_once('/')?;
        if group.is_empty() || name.is_empty() || name.contains('/') {
            return None;
        }
        Some(Command { group, name, tag })
    }
}

/// The command topic of a device, for a client.
pub fn device_command(prefix: &str, id: &str, group: &str, name: &str) -> String {
    format!("{prefix}/{id}/cmd/{group}/{name}")
}

/// The command topic of a tag, for a client.
pub fn tag_command(prefix: &str, tag: &str, group: &str, name: &str) -> String {
    format!("{prefix}/{TAG}/{tag}/cmd/{group}/{name}")
}

/// Every device's status under a prefix.
pub fn statuses(prefix: &str) -> String {
    format!("{prefix}/+/status")
}

/// The device a status topic belongs to.
pub fn status_device<'a>(prefix: &str, topic: &'a str) -> Option<&'a str> {
    let id = topic
        .strip_prefix(prefix)?
        .strip_prefix('/')?
        .strip_suffix("/status")?;
    (!id.is_empty() && !id.contains('/') && id != TAG && id != CLIENT).then_some(id)
}

/// Where a client takes its answers.
pub fn reply(prefix: &str, client: &str) -> String {
    format!("{prefix}/{CLIENT}/{client}/reply")
}

/// A value put into a topic level: a placeholder's value in the prefix, a
/// source or event name. `/` would add a level and `+` `#` are wildcards,
/// so they become `_`, as does a leading `$` (reserved for the broker) and
/// an empty value. NUL is dropped: MQTT refuses it.
pub fn escape(value: &str) -> String {
    let mut out: String = value
        .chars()
        .filter(|&ch| ch != '\0')
        .map(|ch| {
            if matches!(ch, '/' | '+' | '#') {
                '_'
            } else {
                ch
            }
        })
        .collect();
    if out.starts_with('$') {
        out.replace_range(..1, "_");
    }
    if out.is_empty() {
        out.push('_');
    }
    out
}

/// A prefix as it may be used: levels, none empty, no wildcard, not a
/// broker's `$` topic.
pub fn check_prefix(prefix: &str) -> Result<(), String> {
    if prefix.is_empty() {
        return Err("the prefix is empty".into());
    }
    if prefix.starts_with('$') {
        return Err(format!("{prefix:?} starts with $, which is the broker's"));
    }
    check_name(prefix)
}

/// A topic a message may be published to: no wildcard, no NUL, no empty
/// level.
pub fn check_name(topic: &str) -> Result<(), String> {
    if topic.is_empty() || topic.len() > 65535 {
        return Err("a topic is 1 to 65535 bytes".into());
    }
    if let Some(bad) = topic.chars().find(|&ch| matches!(ch, '+' | '#' | '\0')) {
        return Err(format!("{bad:?} is not allowed in a topic"));
    }
    if topic.split('/').any(str::is_empty) {
        return Err(format!("{topic:?} has an empty level"));
    }
    Ok(())
}

/// A subscription filter: `+` alone on its level, `#` alone and last.
pub fn check_filter(filter: &str) -> Result<(), String> {
    if filter.is_empty() || filter.len() > 65535 {
        return Err("a filter is 1 to 65535 bytes".into());
    }
    if filter.contains('\0') {
        return Err("NUL is not allowed in a filter".into());
    }
    let levels: Vec<&str> = filter.split('/').collect();
    for (i, level) in levels.iter().enumerate() {
        if level.contains('#') && (*level != "#" || i + 1 != levels.len()) {
            return Err(format!(
                "{filter:?}: # must be a level of its own, and the last"
            ));
        }
        if level.contains('+') && *level != "+" {
            return Err(format!("{filter:?}: + must be a level of its own"));
        }
    }
    Ok(())
}

/// A subtopic the page names under `page/out/`: levels, no wildcard.
pub fn check_sub(sub: &str) -> Result<(), String> {
    check_name(sub)?;
    if sub.starts_with('$') {
        return Err(format!("{sub:?} starts with $"));
    }
    Ok(())
}

/// Whether a topic matches a filter, as a broker decides it. A filter
/// starting with a wildcard does not match a `$` topic.
pub fn matches(filter: &str, topic: &str) -> bool {
    if topic.starts_with('$') && (filter.starts_with('+') || filter.starts_with('#')) {
        return false;
    }
    let mut want = filter.split('/');
    let mut have = topic.split('/');
    loop {
        match (want.next(), have.next()) {
            (Some("#"), _) => return true,
            (Some("+"), Some(_)) => {}
            (Some(w), Some(h)) if w == h => {}
            (None, None) => return true,
            _ => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_s_topics_hang_from_its_prefix_and_id() {
        let at = Layout::new("acme/lobby", "ab12");
        assert_eq!(at.status(), "acme/lobby/ab12/status");
        assert_eq!(at.state("device"), "acme/lobby/ab12/state/device");
        assert_eq!(at.commands(), "acme/lobby/ab12/cmd/#");
        assert_eq!(at.tag_commands("north"), "acme/lobby/tag/north/cmd/#");
        assert_eq!(at.job("f00d"), "acme/lobby/ab12/jobs/f00d");
        assert_eq!(at.event("cec", "tv-on"), "acme/lobby/ab12/event/cec/tv-on");
        assert_eq!(at.event("a/b", "#"), "acme/lobby/ab12/event/a_b/_");
        assert_eq!(at.frame(), "acme/lobby/ab12/screen/frame");
        assert_eq!(
            at.page_out("menu/sold"),
            "acme/lobby/ab12/page/out/menu/sold"
        );
    }

    #[test]
    fn a_command_topic_names_its_endpoint_and_tag() {
        let at = Layout::new("tessaro", "ab12");
        assert_eq!(
            at.command("tessaro/ab12/cmd/device/Status"),
            Some(Command {
                group: "device",
                name: "Status",
                tag: None
            })
        );
        assert_eq!(
            at.command("tessaro/tag/north/cmd/screen/Power"),
            Some(Command {
                group: "screen",
                name: "Power",
                tag: Some("north")
            })
        );
        assert_eq!(at.command("tessaro/cd34/cmd/device/Status"), None);
        assert_eq!(at.command("tessaro/ab12/cmd/device"), None);
        assert_eq!(at.command("tessaro/ab12/cmd/device/Status/x"), None);
        assert_eq!(at.command("tessarox/ab12/cmd/device/Status"), None);
        assert_eq!(at.command("tessaro/ab12/status"), None);
    }

    #[test]
    fn the_page_reads_only_below_page_in() {
        let at = Layout::new("tessaro", "ab12");
        assert_eq!(at.page_in_sub("tessaro/ab12/page/in/menu"), Some("menu"));
        assert_eq!(at.page_in_sub("tessaro/ab12/page/in/"), None);
        assert_eq!(at.page_in_sub("tessaro/ab12/page/out/menu"), None);
    }

    #[test]
    fn a_status_topic_names_its_device() {
        assert_eq!(
            status_device("tessaro", "tessaro/ab12/status"),
            Some("ab12")
        );
        assert_eq!(status_device("acme/x", "acme/x/ab12/status"), Some("ab12"));
        assert_eq!(status_device("tessaro", "tessaro/tag/status"), None);
        assert_eq!(status_device("tessaro", "tessaro/a/b/status"), None);
        assert_eq!(status_device("tessaro", "other/ab12/status"), None);
    }

    #[test]
    fn escaping_keeps_a_value_on_its_level() {
        assert_eq!(escape("lobby"), "lobby");
        assert_eq!(escape("a/b+c#"), "a_b_c_");
        assert_eq!(escape("$SYS"), "_SYS");
        assert_eq!(escape(""), "_");
        assert_eq!(escape("a\0b"), "ab");
    }

    #[test]
    fn prefixes_names_and_filters_are_checked() {
        assert!(check_prefix("tessaro").is_ok());
        assert!(check_prefix("acme/site-1").is_ok());
        assert!(check_prefix("").is_err());
        assert!(check_prefix("$SYS").is_err());
        assert!(check_prefix("a//b").is_err());
        assert!(check_prefix("a/+").is_err());
        assert!(check_filter("a/+/c").is_ok());
        assert!(check_filter("a/#").is_ok());
        assert!(check_filter("#").is_ok());
        assert!(check_filter("a/#/c").is_err());
        assert!(check_filter("a/b#").is_err());
        assert!(check_filter("a/b+").is_err());
        assert!(check_sub("menu/sold").is_ok());
        assert!(check_sub("menu/#").is_err());
        assert!(check_sub("$x").is_err());
    }

    #[test]
    fn filters_match_as_a_broker_matches_them() {
        assert!(matches("a/+/c", "a/b/c"));
        assert!(!matches("a/+/c", "a/b/d"));
        assert!(matches("a/#", "a"));
        assert!(matches("a/#", "a/b/c"));
        assert!(matches("#", "a/b"));
        assert!(!matches("#", "$SYS/x"));
        assert!(!matches("+/x", "$SYS/x"));
        assert!(matches("$SYS/x", "$SYS/x"));
        assert!(!matches("a/b", "a/b/c"));
        assert!(!matches("a/b/c", "a/b"));
        assert!(matches("a/+", "a/"));
    }
}
