//! Environment-driven configuration.
//!
//! systemd has already parsed `/usr/lib/tessaro-kiosk/tessaro-kiosk.env` -
//! the unit's `EnvironmentFile=` - by the time this runs, and `main` lays
//! the device's settings from `tessaro.db` over it (`state::Effective`), so
//! this only ever reads variables. The defaults file stays data, never code.
//! The defaults below mirror the ones in `tessaro-kiosk.env.in` and exist so
//! the binary is runnable by hand.
//!
//! A change to the settings builds a new `Config` in the control plane,
//! which publishes it as a `Current` on a watch channel: the state machine,
//! the watchdog and the log follow it without the process restarting. What
//! is set up once per process (the clients and their budgets, the listener)
//! is taken from the first one only; its keys restart the agent
//! (`Consumer::AgentRestart`).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use tokio::sync::watch;

/// Where the variables come from. Production reads the process environment;
/// tests hand in a map.
pub trait Env {
    fn get(&self, key: &str) -> Option<String>;
}

pub struct SystemEnv;

impl Env for SystemEnv {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

impl Env for HashMap<String, String> {
    fn get(&self, key: &str) -> Option<String> {
        HashMap::get(self, key).cloned()
    }
}

/// `browser.bridge.mode`. Ordered: each mode includes the one before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BridgeMode {
    Off,
    /// `window.tessaro.config` and the read-only calls.
    Config,
    /// Everything in `Config`, and the device actions.
    Actions,
}

impl BridgeMode {
    /// Anything unknown is `Off`: a page never gets more than was asked for.
    pub fn parse(value: &str) -> Self {
        match value.trim() {
            "config" => BridgeMode::Config,
            "actions" => BridgeMode::Actions,
            _ => BridgeMode::Off,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            BridgeMode::Off => "off",
            BridgeMode::Config => "config",
            BridgeMode::Actions => "actions",
        }
    }
}

/// The variables `Config` reads that are no setting: the image's own, from
/// the env file. `state::defaults` captures them with the registry's, so a
/// `Config` built again from the defaults and the settings matches the one
/// the process started with.
pub const IMAGE_ONLY: &[&str] = &[
    "KIOSK_CDP_URL",
    "KIOSK_UNIT",
    "KIOSK_OFFLINE_PAGE",
    "KIOSK_OFFLINE_PAGE_DEFAULT",
    "KIOSK_OFFLINE_DIR",
    "KIOSK_OFFLINE_MAX_BYTES",
    "KIOSK_PROXY_LISTEN",
    "KIOSK_PLAYER_URL",
];

/// The configuration as the running agent applies it, and the settings it
/// was built from: the debug screen fills its template in from those.
#[derive(Debug, Clone, PartialEq)]
pub struct Current {
    pub config: Config,
    pub settings: BTreeMap<String, String>,
}

/// Where a `Current` is read: every cycle takes the latest.
pub type Follow = watch::Receiver<Arc<Current>>;
/// The control plane's end, which publishes a changed one.
pub type Publish = watch::Sender<Arc<Current>>;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub kiosk_url: String,
    pub probe_url: String,
    /// The device's local proxy, while network.proxy.url is set: what the
    /// probe and the public address go through. Their clients are built
    /// with it once, so switching the proxy on or off restarts the agent.
    pub proxy: Option<std::net::SocketAddr>,

    pub probe_interval: i64,
    pub probe_interval_fail: i64,
    pub probe_connect_timeout: i64,
    pub probe_timeout: i64,
    pub fail_threshold: i64,

    pub refresh_interval: i64,
    pub offline_refresh: i64,

    pub offline_url: String,
    pub offline_page: String,
    pub offline_page_default: String,
    pub offline_dir: String,
    pub offline_max_bytes: i64,

    pub ping_fails: i64,
    pub restart_after: i64,
    pub restart_backoff: i64,

    pub unit: String,
    /// The loopback pages' origin (`KIOSK_SELFTEST_ORIGIN`), whose `/demo/`
    /// the refresh timer reloads in place rather than leaving.
    pub selftest_origin: String,
    pub agent_enable: bool,
    pub enforce_origin: bool,
    pub debug: bool,
    /// Show the debug screen instead of the kiosk page (`browser.debug.enable`).
    pub debug_screen: bool,
    /// A file in the store run in every page (`browser.inject.script`),
    /// normalized, empty for none.
    pub inject_script: String,
    /// What the page gets as `window.tessaro` (`browser.bridge.mode`).
    pub bridge: BridgeMode,
    /// `printer.enable`: the bridge takes window.print() over.
    pub printing: bool,
    /// The player page is on screen, `kiosk_url` is it
    /// (`state::Effective::player`).
    pub player: bool,

    pub cdp_url: String,
    /// The whole budget for one DevTools command. Used to be
    /// `probe_connect_timeout` doing double duty, so one knob silently moved
    /// two unrelated budgets.
    pub cdp_timeout: i64,
    /// Websocket keepalive on the CDP session; two unanswered pings tear the
    /// session down, which is what turns a half-open socket into a reconnect.
    pub cdp_ping: i64,
    /// Ceiling on the session's reconnect backoff. Under the probe interval on
    /// purpose, so the session is normally back before a cycle needs it.
    pub cdp_reconnect_max: i64,
    /// Send `DeviceAccess.enable` when the session is primed - the hook the
    /// Web Bluetooth chooser work in TODO item 5 attaches to.
    pub device_access: bool,
    /// Judge the loop's pledges before pinging the systemd watchdog. Off still
    /// pings - see `watchdog::spawn` for why going quiet is not an option.
    pub watchdog: bool,
    /// `screen.cec.*`, which the CEC worker follows (`crate::cec`).
    pub cec: Cec,
    /// `camera.presence.*`, which the presence watcher follows
    /// (`control/presence.rs`).
    pub presence: Presence,
}

/// Presence detection as the settings have it. The decimals are kept in
/// hundredths, as `keys::Kind::Decimal` checks them, so the config stays
/// comparable whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presence {
    /// `camera.presence.enable`.
    pub enable: bool,
    /// `camera.presence.model`, for the status; the vision service runs it.
    pub model: String,
    /// `camera.presence.confidence`, in hundredths.
    pub confidence: i64,
    /// `camera.presence.near` in centimeters; `None` when it is off.
    pub near: Option<i64>,
    /// `camera.presence.fov`, degrees.
    pub fov: i64,
    /// `camera.presence.arrive` and `.linger`, in hundredths of a second.
    pub arrive: i64,
    pub linger: i64,
    /// `camera.presence.page`: `tessaro:presence` events and the faces.
    pub page: bool,
    /// `camera.presence.scripts`: the scripts that run on presence events.
    pub scripts: bool,
}

/// A `keys::Kind::Decimal` value in hundredths, or `default` when it is
/// missing or not a number.
fn hundredths(env: &dyn Env, name: &str, default: i64) -> i64 {
    env.get(name)
        .and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite())
        .map_or(default, |value| (value * 100.0).round() as i64)
}

/// HDMI-CEC as the settings have it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cec {
    /// `screen.cec.enable`.
    pub enable: bool,
    /// `screen.cec.source`.
    pub source: CecSource,
    /// `screen.cec.name` filled in and cut to what the TV shows.
    pub name: String,
    /// `screen.cec.keys`: remote keys as key presses in the page.
    pub keys: bool,
    /// `screen.cec.page`: `tessaro:cec` events.
    pub page: bool,
    /// `screen.cec.scripts`: the scripts that run on CEC events.
    pub scripts: bool,
}

/// `screen.cec.source`: whether the TV is switched to the device's input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CecSource {
    Off,
    /// Whenever the device wakes the TV.
    Wake,
    /// That, and taken back whenever someone switches away.
    Always,
}

impl CecSource {
    fn parse(value: &str) -> Self {
        match value.trim() {
            "off" => CecSource::Off,
            "always" => CecSource::Always,
            _ => CecSource::Wake,
        }
    }
}

/// The name the TV shows when screen.cec.name fills in to nothing.
pub const CEC_NAME_FALLBACK: &str = "Tessaro";

impl Config {
    pub fn load(env: &dyn Env) -> Self {
        Self {
            kiosk_url: string(env, "KIOSK_URL", ""),
            probe_url: string(env, "KIOSK_PROBE_URL", ""),
            proxy: (!string(env, "KIOSK_PROXY_URL", "").trim().is_empty())
                .then(|| crate::paths::proxy_listen(env)),

            probe_interval: int(env, "KIOSK_PROBE_INTERVAL", 30),
            probe_interval_fail: int(env, "KIOSK_PROBE_INTERVAL_FAIL", 10),
            probe_connect_timeout: int(env, "KIOSK_PROBE_CONNECT_TIMEOUT", 5),
            probe_timeout: int(env, "KIOSK_PROBE_TIMEOUT", 10),
            fail_threshold: int(env, "KIOSK_FAIL_THRESHOLD", 2),

            refresh_interval: int(env, "KIOSK_REFRESH_INTERVAL", 600),
            offline_refresh: int(env, "KIOSK_OFFLINE_REFRESH", 300),

            offline_url: string(env, "KIOSK_OFFLINE_URL", ""),
            offline_page: string(env, "KIOSK_OFFLINE_PAGE", "/data/kiosk/offline.html"),
            offline_page_default: string(
                env,
                "KIOSK_OFFLINE_PAGE_DEFAULT",
                "/usr/share/tessaro-kiosk/offline.html",
            ),
            offline_dir: string(env, "KIOSK_OFFLINE_DIR", "/run/tessaro-kiosk"),
            offline_max_bytes: int(env, "KIOSK_OFFLINE_MAX_BYTES", 262_144),

            ping_fails: int(env, "KIOSK_PING_FAILS", 3),
            restart_after: int(env, "KIOSK_RESTART_AFTER", 40),
            restart_backoff: int(env, "KIOSK_RESTART_BACKOFF", 300),

            unit: string(env, "KIOSK_UNIT", "tessaro-kiosk.service"),
            selftest_origin: string(env, "KIOSK_SELFTEST_ORIGIN", "http://127.0.0.1"),
            agent_enable: flag(env, "KIOSK_AGENT_ENABLE", true),
            enforce_origin: flag(env, "KIOSK_ENFORCE_ORIGIN", true),
            debug: flag(env, "KIOSK_DEBUG", false),
            debug_screen: flag(env, "KIOSK_DEBUG_SCREEN", false),
            inject_script: string(env, "KIOSK_INJECT_SCRIPT", "")
                .trim_matches('/')
                .to_string(),
            bridge: BridgeMode::parse(&string(env, "KIOSK_BRIDGE_MODE", "actions")),
            printing: flag(env, "KIOSK_PRINTING", false),
            player: flag(env, crate::state::PLAYER_MODE, false),

            cdp_url: string(env, "KIOSK_CDP_URL", "http://127.0.0.1:9222"),
            cdp_timeout: int(env, "KIOSK_CDP_TIMEOUT", 5),
            cdp_ping: int(env, "KIOSK_CDP_PING", 10),
            cdp_reconnect_max: int(env, "KIOSK_CDP_RECONNECT_MAX", 15),
            device_access: flag(env, "KIOSK_DEVICE_ACCESS", false),
            watchdog: flag(env, "KIOSK_WATCHDOG", true),
            cec: Cec {
                enable: flag(env, "KIOSK_CEC", false),
                source: CecSource::parse(&string(env, "KIOSK_CEC_SOURCE", "wake")),
                name: Some(protocol::cec::osd_name(&string(env, "KIOSK_CEC_NAME", "")))
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| CEC_NAME_FALLBACK.to_string()),
                keys: flag(env, "KIOSK_CEC_KEYS", false),
                page: flag(env, "KIOSK_CEC_PAGE", true),
                scripts: flag(env, "KIOSK_CEC_SCRIPTS", true),
            },
            presence: Presence {
                enable: flag(env, "KIOSK_PRESENCE", false),
                model: string(env, "KIOSK_PRESENCE_MODEL", "face-full"),
                confidence: hundredths(env, "KIOSK_PRESENCE_CONFIDENCE", 60),
                near: match string(env, "KIOSK_PRESENCE_NEAR", "1.5").trim() {
                    "off" => None,
                    _ => Some(hundredths(env, "KIOSK_PRESENCE_NEAR", 150)),
                },
                fov: int(env, "KIOSK_PRESENCE_FOV", 65).clamp(20, 170),
                arrive: hundredths(env, "KIOSK_PRESENCE_ARRIVE", 50).max(0),
                linger: hundredths(env, "KIOSK_PRESENCE_LINGER", 300).max(50),
                page: flag(env, "KIOSK_PRESENCE_PAGE", true),
                scripts: flag(env, "KIOSK_PRESENCE_SCRIPTS", true),
            },
        }
    }

    /// Every configured deadline longer than the watchdog's pledge ceiling.
    /// Such a call is clamped: if it legitimately runs past the ceiling, the
    /// watchdog calls it overdue and systemd restarts a working agent. Worth
    /// one loud line at startup, never a refusal to start.
    pub fn oversized_budgets(&self) -> Vec<(&'static str, i64)> {
        let ceiling = crate::watchdog::MAX_PLEDGE.as_secs() as i64;

        [
            ("KIOSK_PROBE_CONNECT_TIMEOUT", self.probe_connect_timeout),
            ("KIOSK_PROBE_TIMEOUT", self.probe_timeout),
            ("KIOSK_CDP_TIMEOUT", self.cdp_timeout),
        ]
        .into_iter()
        .filter(|(_, seconds)| *seconds > ceiling)
        .collect()
    }

    /// The URL the probe checks: an explicit health endpoint when the kiosk
    /// itself is not http(s), or when the site has a cheaper one; else the
    /// kiosk URL.
    pub fn probe_target(&self) -> &str {
        if self.probe_url.is_empty() {
            &self.kiosk_url
        } else {
            &self.probe_url
        }
    }

    /// Probing is meaningless against non-http(s) URLs (`file:`, `data:`); the
    /// agent then runs refresh-only.
    pub fn probe_enabled(&self) -> bool {
        let target = self.probe_target();
        target.starts_with("http://") || target.starts_with("https://")
    }
}

fn string(env: &dyn Env, name: &str, default: &str) -> String {
    env.get(name).unwrap_or_else(|| default.to_string())
}

/// Unset, empty or unparseable all fall back to the default. `tessaro-ctl`
/// validates what it stores, but a setting written by another version or
/// by hand with `sqlite3`, or a hand-edited image default, must never stop
/// the kiosk from coming up.
fn int(env: &dyn Env, name: &str, default: i64) -> i64 {
    match env.get(name) {
        None => default,
        Some(value) => value.trim().parse::<i64>().unwrap_or(default),
    }
}

/// Strict `== "1"`, as the shell agent this descends from had it. "true" and
/// "yes" are false; the shipped env file only ever writes 0 or 1.
fn flag(env: &dyn Env, name: &str, default: bool) -> bool {
    match env.get(name) {
        None => default,
        Some(value) => value == "1",
    }
}

#[cfg(test)]
pub mod test_support {
    use super::*;

    /// The shipped defaults, spelled out, so a test can override one variable
    /// without inheriting whatever the developer's shell happens to export.
    pub fn default_env() -> HashMap<String, String> {
        [
            ("KIOSK_URL", "http://kiosk.test/"),
            ("KIOSK_PROBE_URL", "http://kiosk.test/health"),
            ("KIOSK_PROBE_INTERVAL", "30"),
            ("KIOSK_PROBE_INTERVAL_FAIL", "10"),
            ("KIOSK_PROBE_CONNECT_TIMEOUT", "5"),
            ("KIOSK_PROBE_TIMEOUT", "10"),
            ("KIOSK_FAIL_THRESHOLD", "2"),
            ("KIOSK_REFRESH_INTERVAL", "600"),
            ("KIOSK_OFFLINE_REFRESH", "300"),
            ("KIOSK_PING_FAILS", "3"),
            ("KIOSK_RESTART_AFTER", "40"),
            ("KIOSK_RESTART_BACKOFF", "300"),
            ("KIOSK_OFFLINE_PAGE", "/data/kiosk/offline.html"),
            (
                "KIOSK_OFFLINE_PAGE_DEFAULT",
                "/usr/share/tessaro-kiosk/offline.html",
            ),
            ("KIOSK_OFFLINE_DIR", "/run/tessaro-kiosk"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    pub fn config_with(overrides: &[(&str, &str)]) -> Config {
        let mut env = default_env();
        for (key, value) in overrides {
            env.insert(key.to_string(), value.to_string());
        }
        Config::load(&env)
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::config_with;

    #[test]
    fn reads_the_shipped_defaults() {
        let config = config_with(&[]);

        assert_eq!(config.probe_interval, 30);
        assert_eq!(config.fail_threshold, 2);
        assert_eq!(config.refresh_interval, 600);
        assert_eq!(config.unit, "tessaro-kiosk.service");
        assert_eq!(config.cdp_url, "http://127.0.0.1:9222");
        assert!(config.agent_enable);
        assert!(!config.debug);
    }

    #[test]
    fn overrides_win() {
        let config = config_with(&[
            ("KIOSK_PROBE_INTERVAL", "7"),
            ("KIOSK_AGENT_ENABLE", "0"),
            ("KIOSK_DEBUG", "1"),
        ]);

        assert_eq!(config.probe_interval, 7);
        assert!(!config.agent_enable);
        assert!(config.debug);
    }

    #[test]
    fn garbage_numbers_fall_back_to_the_default() {
        let config = config_with(&[("KIOSK_PROBE_INTERVAL", "soon")]);

        assert_eq!(config.probe_interval, 30);
    }

    #[test]
    fn empty_numbers_fall_back_to_the_default() {
        // The shipped env file writes KIOSK_PROBE_URL= and friends as empty,
        // and an operator commenting out a value leaves the same shape.
        let config = config_with(&[("KIOSK_REFRESH_INTERVAL", "")]);

        assert_eq!(config.refresh_interval, 600);
    }

    #[test]
    fn a_non_one_flag_is_false() {
        let config = config_with(&[("KIOSK_DEBUG", "true")]);

        assert!(!config.debug);
    }

    #[test]
    fn the_cdp_and_watchdog_keys_default_to_todays_behaviour() {
        let config = config_with(&[]);

        // 5 is what KIOSK_PROBE_CONNECT_TIMEOUT used to give CDP.
        assert_eq!(config.cdp_timeout, 5);
        assert_eq!(config.cdp_ping, 10);
        assert_eq!(config.cdp_reconnect_max, 15);
        assert!(!config.device_access);
        assert!(config.watchdog);
    }

    #[test]
    fn the_shipped_budgets_fit_under_the_pledge_ceiling() {
        assert!(config_with(&[]).oversized_budgets().is_empty());
    }

    #[test]
    fn an_enormous_probe_timeout_is_called_out() {
        let config = config_with(&[("KIOSK_PROBE_TIMEOUT", "300")]);

        assert_eq!(
            config.oversized_budgets(),
            vec![("KIOSK_PROBE_TIMEOUT", 300)]
        );
    }

    #[test]
    fn probe_target_prefers_the_probe_url() {
        let config = config_with(&[]);

        assert_eq!(config.probe_target(), "http://kiosk.test/health");
        assert!(config.probe_enabled());
    }

    #[test]
    fn probe_target_falls_back_to_the_kiosk_url() {
        let config = config_with(&[("KIOSK_PROBE_URL", "")]);

        assert_eq!(config.probe_target(), "http://kiosk.test/");
    }

    #[test]
    fn built_again_from_the_defaults_it_is_the_config_the_process_started_with() {
        use crate::config::Config;
        use crate::log::Log;
        use crate::state::{defaults, Effective};

        let mut env = super::test_support::default_env();
        env.insert("KIOSK_CDP_URL".into(), "http://127.0.0.1:9333".into());
        env.insert("KIOSK_UNIT".into(), "kiosk-test.service".into());
        env.insert("KIOSK_PROXY_LISTEN".into(), "127.0.0.1:3129".into());
        env.insert("KIOSK_OFFLINE_MAX_BYTES".into(), "1000".into());
        env.insert("UNRELATED".into(), "x".into());
        let settings: std::collections::BTreeMap<String, String> = [
            ("agent.probe_interval", "7"),
            ("network.proxy.url", "http://proxy.test:8080"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let log = Log::buffered(false);

        let started = Config::load(&Effective::new(&env, &settings, &log));
        let rebuilt = Config::load(&Effective::new(&defaults(&env), &settings, &log));

        assert_eq!(rebuilt, started);
        assert_eq!(rebuilt.cdp_url, "http://127.0.0.1:9333");
        assert_eq!(rebuilt.proxy, Some("127.0.0.1:3129".parse().unwrap()));
    }

    #[test]
    fn probing_is_disabled_for_a_non_http_target() {
        let config = config_with(&[
            ("KIOSK_PROBE_URL", ""),
            ("KIOSK_URL", "data:text/html,<h1>hi</h1>"),
        ]);

        assert!(!config.probe_enabled());
    }
}
