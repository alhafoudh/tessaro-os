//! The settings: what has been set on this device, and nothing else. The
//! `settings` and `state` tables of `tessaro.db` (`db.rs`).
//!
//! Sparse on purpose. A key that was never set has no row, so it follows
//! the image's default in `/usr/lib/tessaro-kiosk/tessaro-kiosk.env`, and a
//! later image can still move that default. Keys are the registry's dotted
//! names (`browser.url`), never env names, so a rename of an env variable is
//! a registry change and not a migration.
//!
//! There is no clock anywhere in here. `revision` is a counter, which is all
//! compare-and-set needs, and it cannot be skewed.

use std::collections::{BTreeMap, HashMap};

use tessaro_db::rusqlite::{params, Connection, OptionalExtension};

use crate::config::Env;
use crate::db::Stored;
use crate::log::Log;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    pub revision: u64,
    pub settings: BTreeMap<String, String>,
    /// The guarded changes on probation, by key; empty when nothing waits.
    /// They were made together and are confirmed or reverted together.
    /// They survive an agent restart - which the change itself causes, by
    /// restarting Weston - but not a reboot: the boot oneshot reverts them,
    /// because nobody confirmed them.
    pub pending: Vec<PendingChange>,
}

impl Stored for State {
    const WHAT: &'static str = "the settings";

    fn load(db: &Connection) -> tessaro_db::rusqlite::Result<Self> {
        let mut settings = BTreeMap::new();
        let mut rows = db.prepare("SELECT key, value FROM settings")?;
        for row in rows.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))? {
            let (key, value): (String, String) = row?;
            settings.insert(key, value);
        }
        let revision = db
            .query_row("SELECT revision FROM state WHERE id = 1", [], |row| {
                row.get::<_, i64>(0)
            })
            .optional()?
            .unwrap_or_default() as u64;
        let mut pending = Vec::new();
        let mut rows = db.prepare("SELECT key, value, previous FROM pending ORDER BY key")?;
        for row in rows.query_map([], |row| {
            Ok(PendingChange {
                key: row.get(0)?,
                value: row.get(1)?,
                previous: row.get(2)?,
            })
        })? {
            pending.push(row?);
        }
        Ok(Self {
            revision,
            settings,
            pending,
        })
    }

    fn save(&self, db: &Connection) -> tessaro_db::rusqlite::Result<()> {
        db.execute("DELETE FROM settings", [])?;
        let mut insert = db.prepare("INSERT INTO settings (key, value) VALUES (?1, ?2)")?;
        for (key, value) in &self.settings {
            insert.execute(params![key, value])?;
        }
        db.execute(
            "INSERT OR REPLACE INTO state (id, revision) VALUES (1, ?1)",
            params![self.revision as i64],
        )?;
        db.execute("DELETE FROM pending", [])?;
        let mut insert =
            db.prepare("INSERT INTO pending (key, value, previous) VALUES (?1, ?2, ?3)")?;
        for change in &self.pending {
            insert.execute(params![change.key, change.value, change.previous])?;
        }
        Ok(())
    }

    fn clear(db: &Connection) -> tessaro_db::rusqlite::Result<()> {
        db.execute_batch("DELETE FROM settings; DELETE FROM state; DELETE FROM pending;")
    }
}

/// One guarded change on probation, as the API shows it.
pub use protocol::PendingChange;

/// `a=1, b=2`: the changes on probation, for the journal and an error.
pub fn listed(changes: &[PendingChange]) -> String {
    changes
        .iter()
        .map(|change| format!("{}={}", change.key, change.value))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `a back to 1, b back to the default`: what a revert of them did.
pub fn listed_back(changes: &[PendingChange]) -> String {
    changes
        .iter()
        .map(|change| format!("{} back to {}", change.key, change.previous_or_default()))
        .collect::<Vec<_>>()
        .join(", ")
}

impl State {
    /// Put every pending change back, all at once. Returns them; empty when
    /// nothing was pending.
    pub fn revert_pending(&mut self) -> Vec<PendingChange> {
        if self.pending.is_empty() {
            return Vec::new();
        }
        self.settings = self.confirmed();
        self.revision += 1;
        std::mem::take(&mut self.pending)
    }

    /// The settings as last confirmed: each key on probation at what it goes
    /// back to. What must not follow a change until it is kept (the splash)
    /// is rendered from these.
    pub fn confirmed(&self) -> BTreeMap<String, String> {
        let mut settings = self.settings.clone();
        for change in &self.pending {
            match &change.previous {
                Some(value) => settings.insert(change.key.clone(), value.clone()),
                None => settings.remove(&change.key),
            };
        }
        settings
    }
}

/// The settings as env variables, for the keys the registry knows. A key it
/// does not know - written by a newer agent, or by hand - is skipped and
/// named in the journal rather than dropped from the store.
pub fn overrides(settings: &BTreeMap<String, String>, log: &Log) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for (name, value) in settings {
        match protocol::keys::find(name) {
            // A custom data.* has no variable of its own; it only exists
            // inside the expanded browser.url.
            Some(key) if key.env.is_empty() => {}
            Some(key) => out.push((key.env, value.clone())),
            None => log.info(format!("settings: ignoring unknown key {name}")),
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

/// One of `keys::TEMPLATES` filled in the way it is shown: a URL
/// percent-encoded, the debug screen's text raw.
pub fn expand(
    expansion: protocol::keys::Expansion,
    template: &str,
    settings: &BTreeMap<String, String>,
    defaults: &dyn Env,
    live: &Live,
) -> (String, Vec<String>) {
    match expansion {
        protocol::keys::Expansion::Url => expand_url(template, settings, defaults, live),
        protocol::keys::Expansion::Text => expand_text(template, settings, defaults, live),
    }
}

/// What one placeholder stands for, as `expand_url` describes. `None` for a
/// name that is no placeholder, and for a `data.*` nobody set.
pub fn resolve(
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
    /// What the playlists' store holds (`playlists::Shared`): whether the
    /// timetable has an entry, and the interactive items' origins.
    pub playlists: crate::playlists::Shared,
}

impl Live {
    /// Does this template use anything that can change without a `set` -
    /// an address, a gateway? Not `storage.*`: free space changes all the
    /// time, and following it would re-render and restart onto every write
    /// to `/data`. A URL that shows it is filled in at the next restart; the
    /// debug screen re-renders on its own anyway.
    pub fn moves(template: &str) -> bool {
        protocol::keys::placeholders(template).iter().any(|name| {
            matches!(
                protocol::keys::placeholder(name),
                protocol::keys::Placeholder::Key(key)
                    if key.kind == protocol::keys::Kind::ReadOnly && !key.name.starts_with("storage.")
            )
        })
    }
}

/// A setting as set, else the image default.
pub fn setting(
    settings: &BTreeMap<String, String>,
    defaults: &dyn Env,
    name: &str,
) -> Option<String> {
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

/// Is the player on screen instead of browser.url? While playlist.default is
/// set or the timetable has an entry - the configuration decides, never the
/// clock, so the device does not move between the two at a timetable
/// entry's edges (docs/playlists.md).
pub fn player(settings: &BTreeMap<String, String>, defaults: &dyn Env, live: &Live) -> bool {
    live.playlists.timetable
        || setting(settings, defaults, protocol::keys::PLAYLIST_DEFAULT)
            .is_some_and(|name| !name.is_empty())
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

/// The player page, when the image does not say (`KIOSK_PLAYER_URL`).
pub const PLAYER_URL: &str = "http://127.0.0.1/player.html";

/// Not a setting and never in an env file: what `Effective` answers for
/// whether the player is on screen, so `Config` reads it like the rest.
pub const PLAYER_MODE: &str = "KIOSK_PLAYER_MODE";

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

    /// The player is on screen (`player`), unless maintenance mode puts its
    /// page there instead.
    pub fn player(&self) -> bool {
        !self.maintenance() && player(&self.settings, self.base, &self.live)
    }

    /// The player page: `KIOSK_PLAYER_URL`, an image-only variable.
    pub fn player_url(&self) -> String {
        self.base
            .get("KIOSK_PLAYER_URL")
            .filter(|url| !url.is_empty())
            .unwrap_or_else(|| PLAYER_URL.to_string())
    }

    /// The origins of the playlists' interactive items, which get the device
    /// grants with browser.url's.
    pub fn player_origins(&self) -> &[String] {
        &self.live.playlists.origins
    }

    /// browser.url expanded, maintenance mode or not. The device-API grants
    /// follow this one: toggling maintenance must not rewrite the policy,
    /// which would restart the browser and take the site's grants away.
    pub fn kiosk_url(&self) -> Option<String> {
        self.raw("KIOSK_URL").map(|template| self.expand(&template))
    }

    /// browser.maintenance.url expanded, maintenance mode or not, for the
    /// pages a URL block must never shut out (`render::own_pages`).
    pub fn maintenance_url(&self) -> Option<String> {
        self.raw("KIOSK_MAINTENANCE_URL")
            .map(|template| self.expand(&template))
    }
}

impl Env for Effective<'_> {
    fn get(&self, key: &str) -> Option<String> {
        match key {
            "KIOSK_URL" if self.maintenance() => {
                Some(self.expand(&self.raw("KIOSK_MAINTENANCE_URL").unwrap_or_default()))
            }
            PLAYER_MODE => Some(if self.player() { "1" } else { "0" }.to_string()),
            "KIOSK_URL" if self.player() => Some(self.player_url()),
            "KIOSK_URL" => self.kiosk_url(),
            // The player is the device's own page, always there: the probe
            // checks it, and a source that is down is the player's to skip.
            "KIOSK_PROBE_URL" if self.maintenance() || self.player() => Some(String::new()),
            _ => self.raw(key),
        }
    }
}

/// The image defaults for every registry key, and the image-only variables
/// `Config` reads (`config::IMAGE_ONLY`), captured once from the process
/// environment, which systemd filled from the `/usr/lib` env file. Captured
/// rather than read live so a test, or a host run, is deterministic.
pub fn defaults(env: &dyn Env) -> HashMap<String, String> {
    protocol::keys::KEYS
        .iter()
        .map(|key| key.env)
        .chain(crate::config::IMAGE_ONLY.iter().copied())
        .filter_map(|name| env.get(name).map(|value| (name.to_string(), value)))
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
    fn the_player_is_on_screen_while_a_playlist_or_the_timetable_says() {
        let log = Log::buffered(true);
        let base: HashMap<String, String> = [
            ("KIOSK_URL".to_string(), "https://shop.test/".to_string()),
            (
                "KIOSK_MAINTENANCE_URL".to_string(),
                "http://127.0.0.1/maintenance.html".to_string(),
            ),
        ]
        .into();
        let direct = Effective::new(&base, &settings(&[]), &log);
        assert_eq!(direct.get("KIOSK_URL").unwrap(), "https://shop.test/");
        assert_eq!(direct.get(PLAYER_MODE).unwrap(), "0");

        let set = settings(&[(protocol::keys::PLAYLIST_DEFAULT, "lobby")]);
        let default = Effective::new(&base, &set, &log);
        assert_eq!(default.get("KIOSK_URL").unwrap(), PLAYER_URL);
        assert_eq!(default.get("KIOSK_PROBE_URL").unwrap(), "");
        assert_eq!(default.get(PLAYER_MODE).unwrap(), "1");
        // The grants stay browser.url's.
        assert_eq!(default.kiosk_url().unwrap(), "https://shop.test/");

        let timetable = Effective::new(&base, &settings(&[]), &log).with_live(Live {
            playlists: crate::playlists::Shared {
                timetable: true,
                ..Default::default()
            },
            ..Live::default()
        });
        assert_eq!(timetable.get("KIOSK_URL").unwrap(), PLAYER_URL);

        let maintenance = settings(&[
            (protocol::keys::PLAYLIST_DEFAULT, "lobby"),
            ("browser.maintenance.enable", "1"),
        ]);
        let maintenance = Effective::new(&base, &maintenance, &log);
        assert_eq!(
            maintenance.get("KIOSK_URL").unwrap(),
            "http://127.0.0.1/maintenance.html"
        );
        assert_eq!(maintenance.get(PLAYER_MODE).unwrap(), "0");
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
            ..Live::default()
        };

        let effective = Effective::new(&base, &set, &log).with_live(live);

        // An unknown read-only value is empty, never missing.
        assert_eq!(
            effective.get("KIOSK_URL").unwrap(),
            "https://menu.test/?ip=10.0.0.20&gw=&id=abc"
        );
        assert!(Live::moves("https://x.test/?ip={network.ip}"));
        assert!(!Live::moves("https://x.test/?n={device.name}&t={data.t}"));
        assert!(!Live::moves("https://x.test/?free={storage.data_free}"));
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
            ..Live::default()
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

    fn change(key: &str, value: &str, previous: Option<&str>) -> PendingChange {
        PendingChange {
            key: key.to_string(),
            value: value.to_string(),
            previous: previous.map(str::to_string),
        }
    }

    #[test]
    fn reverting_restores_or_removes() {
        let mut state = State {
            revision: 4,
            settings: settings(&[("screen.resolution", "1280x720")]),
            pending: vec![change("screen.resolution", "1280x720", None)],
        };
        assert_eq!(state.revert_pending().len(), 1);
        assert!(state.settings.is_empty());
        assert_eq!(state.revision, 5);
        assert!(state.revert_pending().is_empty());
        assert_eq!(state.revision, 5);

        state
            .settings
            .insert("screen.resolution".to_string(), "800x600".to_string());
        state.pending = vec![change("screen.resolution", "800x600", Some("1920x1080"))];
        state.revert_pending();
        assert_eq!(state.settings["screen.resolution"], "1920x1080");
    }

    #[test]
    fn changes_made_together_revert_together() {
        let mut state = State {
            revision: 1,
            settings: settings(&[
                ("browser.url", "https://a.test/"),
                ("screen.resolution", "1280x720"),
                ("screen.rotation", "90"),
            ]),
            pending: vec![
                change("screen.resolution", "1280x720", Some("1920x1080")),
                change("screen.rotation", "90", None),
            ],
        };
        // What is confirmed leaves the pending values out, without reverting.
        assert_eq!(
            state.confirmed(),
            settings(&[
                ("browser.url", "https://a.test/"),
                ("screen.resolution", "1920x1080"),
            ])
        );
        assert_eq!(state.pending.len(), 2);

        assert_eq!(state.revert_pending().len(), 2);
        assert_eq!(
            state.settings,
            settings(&[
                ("browser.url", "https://a.test/"),
                ("screen.resolution", "1920x1080"),
            ])
        );
        assert_eq!(state.revision, 2);
    }

    #[test]
    fn the_store_gives_back_what_was_saved() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::buffered(true);
        let db = crate::db::Db::open(dir.path(), &log);
        let saved = State {
            revision: 7,
            settings: BTreeMap::from([
                ("browser.url".to_string(), "https://a.test/".to_string()),
                ("data.store".to_string(), "42".to_string()),
            ]),
            pending: vec![
                change("screen.resolution", "800x600", None),
                change("screen.rotation", "270", Some("90")),
            ],
        };
        db.update(|state: &mut State| {
            *state = saved.clone();
            Ok(())
        })
        .unwrap();

        assert_eq!(db.read::<State>(&log), saved);

        db.update(|state: &mut State| {
            state.pending.clear();
            Ok(())
        })
        .unwrap();
        assert!(db.read::<State>(&log).pending.is_empty());
    }
}
