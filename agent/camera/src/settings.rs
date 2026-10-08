//! What a mirror is set to: camera.format, camera.size, camera.mirrors and
//! camera.presence.enable, as the agent renders them into
//! `/run/tessaro-camera/camera.env`.
//!
//! systemd hands the file to the mirror as its environment once, at start.
//! The mirror also reads the file back while it runs and starts again when
//! it no longer says what the mirror was started with, so a restart the
//! agent never got to send (a mirror started before the file was rendered
//! at boot, an agent stopped between rendering and restarting) cannot leave
//! a camera on stale settings.

use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

pub const FORMAT: &str = "KIOSK_CAMERA_FORMAT";
pub const SIZE: &str = "KIOSK_CAMERA_SIZE";
pub const MIRRORS: &str = "KIOSK_CAMERA_MIRRORS";
pub const VISION: &str = "KIOSK_CAMERA_VISION";

/// Each value as given; one missing is `None`, which every reader takes as
/// its default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    pub format: Option<String>,
    pub size: Option<String>,
    pub mirrors: Option<String>,
    pub vision: Option<String>,
}

impl Settings {
    /// What systemd started the mirror with.
    pub fn from_env() -> Settings {
        Settings::from_lookup(|key| env::var(key).ok())
    }

    /// What `camera.env` says now. A missing file is every default, as it
    /// is to systemd's `EnvironmentFile=-`; one that cannot be read is
    /// `None`, so a passing error does not restart the camera.
    pub fn from_file(path: &Path) -> Option<Settings> {
        match fs::read_to_string(path) {
            Ok(body) => Some(Settings::parse(&body)),
            Err(err) if err.kind() == ErrorKind::NotFound => Some(Settings::default()),
            Err(_) => None,
        }
    }

    /// `KEY=value` lines, the way the agent writes them: no quotes, since
    /// `config set` refuses them in a value.
    pub fn parse(body: &str) -> Settings {
        Settings::from_lookup(|key| {
            body.lines()
                .filter_map(|line| line.trim().split_once('='))
                .filter(|(name, _)| name.trim() == key)
                .map(|(_, value)| value.trim().to_string())
                .next_back()
        })
    }

    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Settings {
        let value = |key| lookup(key).filter(|value| !value.is_empty());
        Settings {
            format: value(FORMAT),
            size: value(SIZE),
            mirrors: value(MIRRORS),
            vision: value(VISION),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_file_the_agent_renders() {
        let settings = Settings::parse(
            "KIOSK_CAMERA_FORMAT=auto\nKIOSK_CAMERA_SIZE=640x480\nKIOSK_CAMERA_MIRRORS=2\nKIOSK_CAMERA_VISION=1\n",
        );
        assert_eq!(
            settings,
            Settings {
                format: Some("auto".into()),
                size: Some("640x480".into()),
                mirrors: Some("2".into()),
                vision: Some("1".into()),
            }
        );
    }

    #[test]
    fn a_missing_or_empty_value_is_the_default() {
        let settings = Settings::parse("KIOSK_CAMERA_FORMAT=\nOTHER=1\n");
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn the_last_line_for_a_key_wins_as_with_systemd() {
        let settings = Settings::parse("KIOSK_CAMERA_VISION=0\nKIOSK_CAMERA_VISION=1\n");
        assert_eq!(settings.vision.as_deref(), Some("1"));
    }

    #[test]
    fn a_missing_file_is_every_default() {
        let dir =
            std::env::temp_dir().join(format!("tessaro-camera-settings-{}", std::process::id()));
        assert_eq!(
            Settings::from_file(&dir.join("camera.env")),
            Some(Settings::default())
        );
    }

    #[test]
    fn a_mirror_started_before_the_file_differs_from_it() {
        // The boot race: the mirror's environment is empty, the file that
        // turns presence detection on appears after it.
        let started = Settings::default();
        let rendered = Settings::parse("KIOSK_CAMERA_VISION=1\n");
        assert_ne!(started, rendered);
    }
}
