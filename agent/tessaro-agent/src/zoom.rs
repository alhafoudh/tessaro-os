//! `tessaro-agent zoom`: `browser.zoom` written into Chromium's profile, as
//! the kiosk unit's `ExecStartPre`, while the browser is not running.
//!
//! It is the zoom Ctrl+/- sets: Chromium's `HostZoomMap`, which the profile
//! keeps in `Preferences` as `partition.default_zoom_level` (every site) and
//! `partition.per_host_zoom_levels` (one site each). The DevTools protocol
//! has no command for it, and an emulation override is a different thing: a
//! fixed viewport that outlives the session that set it. So the level goes
//! into the file before Chromium reads it, which is why a change restarts
//! the browser.
//!
//! Before every start, not once: Chromium writes its own copy of the file
//! back, and a Ctrl+/- on the device would otherwise stick to one site
//! forever. The per-site levels are dropped for the same reason - the kiosk
//! shows what `browser.zoom` says, on every site.
//!
//! Every failure logs and carries on: a zoom must never keep the browser
//! from starting.

use std::fs;
use std::io;
use std::path::Path;

use serde_json::{Map, Value};

use crate::config::Env;
use crate::log::Log;
use crate::store;

const PREFERENCES: &str = "/data/kiosk/chromium/Default/Preferences";

/// Chromium keys the zoom levels by storage partition, and the default one
/// is `x` (`ChromeZoomLevelPrefs`, measured on Chromium 147).
const PARTITION: &str = "x";

pub fn run(env: &dyn Env, log: &Log) {
    let percent = env
        .get("KIOSK_ZOOM")
        .and_then(|value| value.trim().parse::<u16>().ok())
        .unwrap_or(100)
        .clamp(25, 500);
    let path = env
        .get("KIOSK_CHROMIUM_PREFERENCES")
        .unwrap_or_else(|| PREFERENCES.to_string());
    let path = Path::new(&path);

    let existing = match fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => {
            log.info(format!("page zoom not set: {}: {err}", path.display()));
            return;
        }
    };
    let body = match preferences(existing.as_deref(), percent) {
        Ok(Some(body)) => body,
        Ok(None) => return,
        Err(err) => {
            log.info(format!("page zoom not set: {}: {err}", path.display()));
            return;
        }
    };
    match store::replace_if_changed(path, body.as_bytes(), 0o600) {
        Ok(_) => log.info(format!(
            "page zoom {percent}% (Chromium zoom level {:.3})",
            level(percent)
        )),
        Err(err) => log.info(format!("page zoom not set: {}: {err}", path.display())),
    }
}

/// Chromium's zoom level for a percentage: each step is a factor of 1.2,
/// and 100% is level 0.
pub fn level(percent: u16) -> f64 {
    (f64::from(percent) / 100.0).ln() / 1.2_f64.ln()
}

/// `Preferences` with `percent` as the zoom of every site, or `None` when it
/// already says exactly that. No file yet is a profile Chromium has not made:
/// the zoom alone is enough, and Chromium fills in the rest.
pub fn preferences(existing: Option<&str>, percent: u16) -> Result<Option<String>, String> {
    let before = match existing {
        Some(text) if !text.trim().is_empty() => {
            serde_json::from_str::<Value>(text).map_err(|err| format!("not JSON: {err}"))?
        }
        _ => Value::Object(Map::new()),
    };
    let Value::Object(mut root) = before.clone() else {
        return Err("not a JSON object".to_string());
    };

    // 100 is Chromium's own default: it only takes away, and so never makes
    // a file that was not there.
    let zoomed = percent != 100;
    if zoomed || root.contains_key("partition") {
        let partition = root
            .entry("partition")
            .or_insert_with(|| Value::Object(Map::new()));
        let Value::Object(partition) = partition else {
            return Err("partition is not an object".to_string());
        };
        if let Some(Value::Object(levels)) = partition.get_mut("per_host_zoom_levels") {
            levels.remove(PARTITION);
        }
        if zoomed || partition.contains_key("default_zoom_level") {
            let defaults = partition
                .entry("default_zoom_level")
                .or_insert_with(|| Value::Object(Map::new()));
            let Value::Object(defaults) = defaults else {
                return Err("partition.default_zoom_level is not an object".to_string());
            };
            if zoomed {
                defaults.insert(PARTITION.to_string(), Value::from(level(percent)));
            } else {
                defaults.remove(PARTITION);
            }
        }
    }

    let after = Value::Object(root);
    if after == before {
        return Ok(None);
    }
    serde_json::to_string(&after)
        .map(Some)
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parsed(text: Option<String>) -> Value {
        serde_json::from_str(&text.expect("a change")).unwrap()
    }

    #[test]
    fn a_level_is_steps_of_1_2() {
        assert_eq!(level(100), 0.0);
        assert!((level(120) - 1.0).abs() < 1e-12);
        assert!((level(150) - 2.2239).abs() < 1e-4);
        assert!(level(25) < 0.0);
    }

    #[test]
    fn the_zoom_goes_in_beside_what_chromium_keeps() {
        let existing = r#"{"browser":{"window_placement":{"top":0}},"partition":{"other":1}}"#;
        let after = parsed(preferences(Some(existing), 120).unwrap());

        let kept = json!({ "window_placement": { "top": 0 } });
        assert_eq!(after["browser"], kept);
        assert_eq!(after["partition"]["other"], 1);
        let level = after["partition"]["default_zoom_level"]["x"].as_f64();
        assert!((level.unwrap() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn a_ctrl_plus_on_one_site_does_not_survive_a_restart() {
        let existing = r#"{"partition":{"per_host_zoom_levels":{"x":{"example.com":{"zoom_level":3.0}},"y":{}}}}"#;
        let after = parsed(preferences(Some(existing), 100).unwrap());

        let left = json!({ "y": {} });
        assert_eq!(after["partition"]["per_host_zoom_levels"], left);
    }

    #[test]
    fn back_to_100_removes_the_level_and_an_unchanged_file_is_left_alone() {
        let zoomed = preferences(None, 150).unwrap().unwrap();
        let back = preferences(Some(&zoomed), 100).unwrap().unwrap();

        assert_eq!(
            serde_json::from_str::<Value>(&back).unwrap()["partition"]["default_zoom_level"],
            json!({})
        );
        assert_eq!(preferences(Some(&zoomed), 150).unwrap(), None);
        assert_eq!(preferences(Some(&back), 100).unwrap(), None);
    }

    #[test]
    fn no_zoom_never_makes_a_profile() {
        assert_eq!(preferences(None, 100).unwrap(), None);
        assert_eq!(preferences(Some(r#"{"browser":{}}"#), 100).unwrap(), None);
    }

    #[test]
    fn a_broken_file_is_not_overwritten() {
        assert!(preferences(Some("{nope"), 150).is_err());
        assert!(preferences(Some("[]"), 150).is_err());
        assert!(preferences(Some(r#"{"partition":3}"#), 150).is_err());
    }
}
