//! Environment-driven configuration.
//!
//! systemd has already parsed `/usr/lib/tessaro-kiosk/tessaro-kiosk.env` and
//! `/etc/default/tessaro-kiosk` by the time this runs - both are
//! `EnvironmentFile=` on the unit - so this only reads the resulting
//! variables. The defaults file stays data, never code. The defaults below
//! mirror the ones in `tessaro-kiosk.env.in` and exist so the binary is
//! runnable by hand.

use std::collections::HashMap;

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

#[derive(Debug, Clone)]
pub struct Config {
    pub kiosk_url: String,
    pub probe_url: String,

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
    pub agent_enable: bool,
    pub debug: bool,

    pub cdp_url: String,
}

impl Config {
    pub fn from_env() -> Self {
        Self::load(&SystemEnv)
    }

    pub fn load(env: &dyn Env) -> Self {
        Self {
            kiosk_url: string(env, "KIOSK_URL", ""),
            probe_url: string(env, "KIOSK_PROBE_URL", ""),

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
            agent_enable: flag(env, "KIOSK_AGENT_ENABLE", true),
            debug: flag(env, "KIOSK_DEBUG", false),

            cdp_url: string(env, "KIOSK_CDP_URL", "http://127.0.0.1:9222"),
        }
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

/// Unset, empty or unparseable all fall back to the default. A malformed
/// override in `/etc/default/tessaro-kiosk` must never stop the kiosk from
/// coming up - it is the one file a technician edits in the field.
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
    fn probing_is_disabled_for_a_non_http_target() {
        let config = config_with(&[
            ("KIOSK_PROBE_URL", ""),
            ("KIOSK_URL", "data:text/html,<h1>hi</h1>"),
        ]);

        assert!(!config.probe_enabled());
    }
}
