//! `state.json`: what has been set on this device, and nothing else.
//!
//! Sparse on purpose. A key that was never set is not in the file, so it
//! follows the image's default in `/usr/lib/tessaro-kiosk/tessaro-kiosk.env`,
//! and a later image can still move that default. Keys are the registry's
//! dotted names (`browser.url`), never env names, so a rename of an env
//! variable is a registry change and not a migration.
//!
//! There is no clock anywhere in here. `revision` is a counter, which is all
//! compare-and-set needs, and it cannot be skewed.

use std::collections::{BTreeMap, HashMap};

use protocol::keys;
use serde::{Deserialize, Serialize};

use crate::config::Env;
use crate::log::Log;

pub const FILE: &str = "state.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub settings: BTreeMap<String, String>,
    /// A guarded change on probation. Survives an agent restart - which the
    /// change itself causes, by restarting Weston - but not a reboot: the
    /// boot oneshot reverts it, because nobody confirmed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<PendingChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingChange {
    pub key: String,
    pub value: String,
    /// What to go back to. `None` means the key was not set.
    pub previous: Option<String>,
}

impl State {
    /// Put a pending change back. Returns whether there was one.
    pub fn revert_pending(&mut self) -> Option<PendingChange> {
        let pending = self.pending.take()?;
        match &pending.previous {
            Some(value) => self.settings.insert(pending.key.clone(), value.clone()),
            None => self.settings.remove(&pending.key),
        };
        self.revision += 1;
        Some(pending)
    }

    /// Move settings an older image saved under names that have changed
    /// since (`keys::RENAMED`) to their new names, and rewrite the
    /// placeholders in URL and template values that name them. No value
    /// changes meaning, so nothing needs re-applying. Returns one line per
    /// change for the journal; empty when there was nothing to do.
    pub fn rename_keys(&mut self) -> Vec<String> {
        let mut done = Vec::new();
        for (old, new) in keys::RENAMED {
            let Some(value) = self.settings.remove(*old) else {
                continue;
            };
            if self.settings.contains_key(*new) {
                done.push(format!("{old} dropped: {new} is set already"));
            } else {
                self.settings.insert(new.to_string(), value);
                done.push(format!("{old} is now {new}"));
            }
        }
        for (name, value) in self.settings.iter_mut() {
            let template = keys::find(name)
                .is_some_and(|key| matches!(key.kind, keys::Kind::Url | keys::Kind::Template));
            if !template {
                continue;
            }
            let renamed = keys::rename_placeholders(value);
            if renamed != *value {
                done.push(format!("{name}: {value} is now {renamed}"));
                *value = renamed;
            }
        }
        if let Some(pending) = &mut self.pending {
            if let Some(new) = keys::renamed(&pending.key) {
                done.push(format!("{} on probation is now {new}", pending.key));
                pending.key = new.to_string();
            }
        }
        if !done.is_empty() {
            self.revision += 1;
        }
        done
    }
}

/// The settings as env variables, for the keys the registry knows. A key it
/// does not know - written by a newer agent, or renamed since - is skipped
/// and named in the journal rather than dropped from the file.
pub fn overrides(settings: &BTreeMap<String, String>, log: &Log) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for (name, value) in settings {
        match protocol::keys::find(name) {
            // A custom data.* has no variable of its own; it only exists
            // inside the expanded browser.url.
            Some(key) if key.env.is_empty() => {}
            Some(key) => out.push((key.env, value.clone())),
            None => log.info(format!("state.json: ignoring unknown key {name}")),
        }
    }
    out
}

/// A browser.url template filled in, and the placeholders nothing could fill.
///
/// A placeholder is a setting's full key. `{data.name}` is that custom value;
/// `{any.key}` is that setting's effective value - what is set, else the
/// image default, else empty - and `{device.name}` falls back to
/// `derived_name`, the name the device actually answers to when none was
/// set. A read-only key (`{network.ip}`, `{device.id}`) is whatever `live` says,
/// or empty. Only a `data.*` nobody set, or a name that is no setting at all
/// (a bare `{name}` included), counts as missing.
pub fn expand_url(
    template: &str,
    settings: &BTreeMap<String, String>,
    defaults: &dyn Env,
    live: &Live,
) -> (String, Vec<String>) {
    protocol::keys::expand(template, |name| resolve(name, settings, defaults, live))
}

/// The debug screen's template filled in: the same placeholders as browser.url,
/// with values left raw rather than percent-encoded - the page escapes them
/// for HTML - and `{browser.url}` too, as the URL it expands to.
pub fn expand_text(
    template: &str,
    settings: &BTreeMap<String, String>,
    defaults: &dyn Env,
    live: &Live,
) -> (String, Vec<String>) {
    let kiosk_url = || {
        let url = settings
            .get("browser.url")
            .cloned()
            .or_else(|| defaults.get("KIOSK_URL"))
            .unwrap_or_default();
        expand_url(&url, settings, defaults, live).0
    };
    protocol::keys::expand_with(
        template,
        |name| match name {
            "browser.url" => Some(kiosk_url()),
            _ => resolve(name, settings, defaults, live),
        },
        str::to_string,
    )
}

/// What one placeholder stands for, as `expand_url` describes.
fn resolve(
    name: &str,
    settings: &BTreeMap<String, String>,
    defaults: &dyn Env,
    live: &Live,
) -> Option<String> {
    use protocol::keys::{Kind, Placeholder};

    match protocol::keys::placeholder(name) {
        // The placeholder is the key itself.
        Placeholder::Param(_) => settings.get(name).cloned(),
        Placeholder::Key(key) if key.kind == Kind::ReadOnly => {
            Some(live.values.get(key.name).cloned().unwrap_or_default())
        }
        Placeholder::Key(key) => {
            let value = settings
                .get(key.name)
                .cloned()
                .or_else(|| defaults.get(key.env))
                .unwrap_or_default();
            if key.name == "device.name" && value.is_empty() {
                return Some(live.derived_name.clone().unwrap_or_default());
            }
            Some(value)
        }
        Placeholder::Unknown => None,
    }
}

/// What the device reports rather than stores: the name derived from its
/// node id, and the read-only keys (`device.id`, `network.*`) as they are now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Live {
    pub derived_name: Option<String>,
    pub values: BTreeMap<String, String>,
}

impl Live {
    /// Does this template use anything that can change without a `set` -
    /// an address, a gateway?
    pub fn moves(template: &str) -> bool {
        protocol::keys::placeholders(template).iter().any(|name| {
            matches!(
                protocol::keys::placeholder(name),
                protocol::keys::Placeholder::Key(key) if key.kind == protocol::keys::Kind::ReadOnly
            )
        })
    }
}

/// A setting as set, else the image default.
fn setting(settings: &BTreeMap<String, String>, defaults: &dyn Env, name: &str) -> Option<String> {
    let key = protocol::keys::find(name)?;
    settings
        .get(name)
        .cloned()
        .or_else(|| defaults.get(key.env))
}

/// Is the device in maintenance mode?
pub fn maintenance(settings: &BTreeMap<String, String>, defaults: &dyn Env) -> bool {
    setting(settings, defaults, "browser.maintenance.enable").as_deref() == Some("1")
}

/// Is the debug screen up? It wins over maintenance mode: the agent shows it
/// instead of whatever `KIOSK_URL` is.
pub fn debug_screen(settings: &BTreeMap<String, String>, defaults: &dyn Env) -> bool {
    setting(settings, defaults, "browser.debug.enable").as_deref() == Some("1")
}

/// The URL template the screen follows, and the key it came from:
/// browser.maintenance.url in maintenance mode, else browser.url - each as set, else
/// the image default.
pub fn shown_template(
    settings: &BTreeMap<String, String>,
    defaults: &dyn Env,
) -> (&'static str, String) {
    let name = if maintenance(settings, defaults) {
        "browser.maintenance.url"
    } else {
        "browser.url"
    };
    (name, setting(settings, defaults, name).unwrap_or_default())
}

/// The image's defaults with this device's settings on top - what every
/// consumer ends up seeing. `KIOSK_URL` comes out expanded: the browser, the
/// agent's origin checks and the probe all see the same URL, and none of them
/// ever sees a `{placeholder}`.
///
/// In maintenance mode `KIOSK_URL` *is* the maintenance page, so every one of
/// them follows it without knowing the mode exists: the browser starts on it
/// after a reboot, the agent navigates to it and keeps the browser on its
/// origin. `KIOSK_PROBE_URL` reads as empty then, so a site that is down does
/// not put the offline page over the maintenance page. The one consumer that
/// must not follow it is the device-API policy - see `kiosk_url`.
pub struct Effective<'a> {
    base: &'a dyn Env,
    overrides: HashMap<&'static str, String>,
    settings: BTreeMap<String, String>,
    live: Live,
}

impl<'a> Effective<'a> {
    pub fn new(base: &'a dyn Env, settings: &BTreeMap<String, String>, log: &Log) -> Self {
        Self {
            base,
            overrides: overrides(settings, log).into_iter().collect(),
            settings: settings.clone(),
            live: Live::default(),
        }
    }

    /// The name derived from the node id, for `{device.name}` when none is set.
    #[cfg(test)]
    pub fn with_derived_name(mut self, name: Option<String>) -> Self {
        self.live.derived_name = name;
        self
    }

    /// What the device reports - derived name, read-only keys.
    pub fn with_live(mut self, live: Live) -> Self {
        self.live = live;
        self
    }

    fn raw(&self, key: &str) -> Option<String> {
        match self.overrides.get(key) {
            Some(value) => Some(value.clone()),
            None => self.base.get(key),
        }
    }

    // A placeholder with no value expands to nothing. `set` refuses that;
    // only an image default with a placeholder nobody set can get here, and
    // an empty segment beats a literal brace.
    fn expand(&self, template: &str) -> String {
        expand_url(template, &self.settings, self.base, &self.live).0
    }

    pub fn maintenance(&self) -> bool {
        maintenance(&self.settings, self.base)
    }

    /// browser.url expanded, maintenance mode or not. The device-API grants
    /// follow this one: toggling maintenance must not rewrite the policy,
    /// which would restart the browser and take the site's grants away.
    pub fn kiosk_url(&self) -> Option<String> {
        self.raw("KIOSK_URL").map(|template| self.expand(&template))
    }
}

impl Env for Effective<'_> {
    fn get(&self, key: &str) -> Option<String> {
        match key {
            "KIOSK_URL" if self.maintenance() => {
                Some(self.expand(&self.raw("KIOSK_MAINTENANCE_URL").unwrap_or_default()))
            }
            "KIOSK_URL" => self.kiosk_url(),
            "KIOSK_PROBE_URL" if self.maintenance() => Some(String::new()),
            _ => self.raw(key),
        }
    }
}

/// The image defaults for every registry key, captured once from the
/// process environment, which systemd filled from the `/usr/lib` env file.
/// Captured rather than read live so a test, or a host run, is deterministic.
pub fn defaults(env: &dyn Env) -> HashMap<String, String> {
    protocol::keys::KEYS
        .iter()
        .filter_map(|key| env.get(key.env).map(|value| (key.env.to_string(), value)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn settings_win_over_defaults_and_the_rest_falls_through() {
        let log = Log::buffered(true);
        let base: HashMap<String, String> = [
            ("KIOSK_URL".to_string(), "http://127.0.0.1/".to_string()),
            ("KIOSK_OSK".to_string(), "auto".to_string()),
        ]
        .into();

        let set = settings(&[("browser.url", "https://a.test/")]);
        let effective = Effective::new(&base, &set, &log);

        assert_eq!(effective.get("KIOSK_URL").unwrap(), "https://a.test/");
        assert_eq!(effective.get("KIOSK_OSK").unwrap(), "auto");
        assert_eq!(effective.get("KIOSK_NOPE"), None);
    }

    #[test]
    fn the_kiosk_url_comes_out_expanded() {
        let log = Log::buffered(true);
        let base: HashMap<String, String> =
            [("KIOSK_URL".to_string(), "http://127.0.0.1/".to_string())].into();
        let set = settings(&[
            ("browser.url", "https://{data.shop}.test/?lang={data.lang}"),
            ("data.shop", "north"),
            ("data.lang", "sk"),
        ]);

        let effective = Effective::new(&base, &set, &log);

        assert_eq!(
            effective.get("KIOSK_URL").unwrap(),
            "https://north.test/?lang=sk"
        );
        // Parameters are not variables, and are not "unknown keys" either.
        assert_eq!(overrides(&set, &log).len(), 1);
        assert!(log.lines().is_empty());
    }

    #[test]
    fn any_setting_is_a_placeholder_with_its_effective_value() {
        let log = Log::buffered(true);
        let base: HashMap<String, String> = [
            ("KIOSK_URL".to_string(), "http://127.0.0.1/".to_string()),
            ("KIOSK_OSK".to_string(), "auto".to_string()),
        ]
        .into();
        let set = settings(&[
            (
                "browser.url",
                "https://{device.name}.test/?osk={screen.osk}&scale={screen.scale}&s={browser.fps_counter}",
            ),
            ("browser.fps_counter", "1"),
        ]);

        let effective =
            Effective::new(&base, &set, &log).with_derived_name(Some("brave-otter-3fa2".into()));

        assert_eq!(
            effective.get("KIOSK_URL").unwrap(),
            "https://brave-otter-3fa2.test/?osk=auto&scale=&s=1"
        );

        let named = settings(&[
            ("browser.url", "https://{device.name}.test/"),
            ("device.name", "lobby"),
        ]);
        let effective =
            Effective::new(&base, &named, &log).with_derived_name(Some("brave-otter-3fa2".into()));
        assert_eq!(effective.get("KIOSK_URL").unwrap(), "https://lobby.test/");
    }

    #[test]
    fn only_unset_parameters_and_non_settings_are_missing() {
        let base: HashMap<String, String> = HashMap::new();
        let (_, missing) = expand_url(
            "https://x.test/{data.store}/{store}/{no.such}/{browser.url}/{screen.scale}/{network.ip}/{browser.maintenance.url}",
            &BTreeMap::new(),
            &base,
            &Live::default(),
        );
        assert_eq!(
            missing,
            [
                "data.store",
                "store",
                "no.such",
                "browser.url",
                "browser.maintenance.url"
            ]
        );
    }

    #[test]
    fn maintenance_mode_shows_the_maintenance_page_and_keeps_the_kiosk_url() {
        let log = Log::buffered(true);
        let base: HashMap<String, String> = [
            ("KIOSK_URL".to_string(), "http://127.0.0.1/".to_string()),
            ("KIOSK_MAINTENANCE".to_string(), "0".to_string()),
            (
                "KIOSK_MAINTENANCE_URL".to_string(),
                "http://127.0.0.1/maintenance.html".to_string(),
            ),
        ]
        .into();
        let deployed = [
            ("browser.url", "https://shop.test/"),
            ("browser.probe_url", "https://shop.test/health"),
        ];

        let off = settings(&deployed);
        let effective = Effective::new(&base, &off, &log);
        assert!(!effective.maintenance());
        assert_eq!(effective.get("KIOSK_URL").unwrap(), "https://shop.test/");
        assert_eq!(
            effective.get("KIOSK_PROBE_URL").unwrap(),
            "https://shop.test/health"
        );

        let mut on = off.clone();
        on.insert("browser.maintenance.enable".into(), "1".into());
        let effective = Effective::new(&base, &on, &log);
        assert!(effective.maintenance());
        assert_eq!(
            effective.get("KIOSK_URL").unwrap(),
            "http://127.0.0.1/maintenance.html"
        );
        // The probe follows the page on screen, not the site's health check.
        assert_eq!(effective.get("KIOSK_PROBE_URL").unwrap(), "");
        // What the device-API grants follow does not move.
        assert_eq!(effective.kiosk_url().unwrap(), "https://shop.test/");
        assert_eq!(shown_template(&on, &base).0, "browser.maintenance.url");
        assert_eq!(shown_template(&off, &base).0, "browser.url");
    }

    #[test]
    fn the_maintenance_url_is_a_template_too() {
        let log = Log::buffered(true);
        let base: HashMap<String, String> =
            [("KIOSK_URL".to_string(), "http://127.0.0.1/".to_string())].into();
        let set = settings(&[
            ("browser.maintenance.enable", "1"),
            (
                "browser.maintenance.url",
                "http://127.0.0.1/maintenance.html?title={data.title}&n={device.name}",
            ),
            ("data.title", "Back at 14:00"),
            ("device.name", "lobby"),
        ]);

        let effective = Effective::new(&base, &set, &log);

        assert_eq!(
            effective.get("KIOSK_URL").unwrap(),
            "http://127.0.0.1/maintenance.html?title=Back%20at%2014%3A00&n=lobby"
        );
    }

    #[test]
    fn read_only_keys_expand_to_what_the_device_reports() {
        let log = Log::buffered(true);
        let base: HashMap<String, String> =
            [("KIOSK_URL".to_string(), "http://127.0.0.1/".to_string())].into();
        let set = settings(&[(
            "browser.url",
            "https://menu.test/?ip={network.ip}&gw={network.gateway}&id={device.id}",
        )]);
        let live = Live {
            derived_name: None,
            values: [
                ("network.ip".to_string(), "10.0.0.20".to_string()),
                ("device.id".to_string(), "abc".to_string()),
            ]
            .into(),
        };

        let effective = Effective::new(&base, &set, &log).with_live(live);

        // An unknown read-only value is empty, never missing.
        assert_eq!(
            effective.get("KIOSK_URL").unwrap(),
            "https://menu.test/?ip=10.0.0.20&gw=&id=abc"
        );
        assert!(Live::moves("https://x.test/?ip={network.ip}"));
        assert!(!Live::moves("https://x.test/?n={device.name}&t={data.t}"));
    }

    #[test]
    fn the_debug_text_is_raw_and_can_name_the_kiosk_url() {
        let base: HashMap<String, String> =
            [("KIOSK_URL".to_string(), "http://127.0.0.1/".to_string())].into();
        let set = settings(&[
            ("browser.url", "https://menu.test/?t={data.table}"),
            ("data.table", "a b"),
        ]);
        let live = Live {
            derived_name: Some("brave-otter-3fa2".into()),
            values: [("network.cidr".to_string(), "10.0.0.20/24".to_string())].into(),
        };

        let (text, missing) = expand_text(
            "{device.name}\\nip {network.cidr}\\nurl {browser.url}\\nt {data.table}\\n{browser.debug.template}",
            &set,
            &base,
            &live,
        );

        assert_eq!(
            text,
            "brave-otter-3fa2\\nip 10.0.0.20/24\\nurl https://menu.test/?t=a%20b\\nt a b\\n"
        );
        assert_eq!(missing, ["browser.debug.template"]);
    }

    #[test]
    fn unknown_keys_are_named_not_fatal() {
        let log = Log::buffered(true);
        let set = settings(&[("browser.url", "https://a.test/"), ("future.thing", "x")]);

        let env = overrides(&set, &log);

        assert_eq!(env, vec![("KIOSK_URL", "https://a.test/".to_string())]);
        assert!(log.lines().iter().any(|line| line.contains("future.thing")));
    }

    #[test]
    fn reverting_restores_or_removes() {
        let mut state = State {
            revision: 4,
            settings: settings(&[("screen.resolution", "1280x720")]),
            pending: Some(PendingChange {
                key: "screen.resolution".to_string(),
                value: "1280x720".to_string(),
                previous: None,
            }),
        };
        assert!(state.revert_pending().is_some());
        assert!(state.settings.is_empty());
        assert_eq!(state.revision, 5);
        assert!(state.revert_pending().is_none());

        state
            .settings
            .insert("screen.resolution".to_string(), "800x600".to_string());
        state.pending = Some(PendingChange {
            key: "screen.resolution".to_string(),
            value: "800x600".to_string(),
            previous: Some("1920x1080".to_string()),
        });
        state.revert_pending();
        assert_eq!(state.settings["screen.resolution"], "1920x1080");
    }

    #[test]
    fn an_old_file_without_newer_fields_still_parses() {
        let state: State = serde_json::from_str(r#"{"settings":{"browser.url":"x"}}"#).unwrap();
        assert_eq!(state.revision, 0);
        assert!(state.pending.is_none());
    }
}
