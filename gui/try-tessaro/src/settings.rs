//! What the user picks before Start: the screen and the sound. Kept in
//! `settings.json` in the data directory, applied on the next start.

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

const FILE: &str = "settings.json";

/// The screen the device gets: what Weston sees as the display's preferred
/// mode (`virtio-gpu-pci`'s `xres`/`yres`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Resolution {
    Hd,
    FullHd,
    Portrait,
    Xga,
}

impl Resolution {
    pub const ALL: [Resolution; 4] = [
        Resolution::Hd,
        Resolution::FullHd,
        Resolution::Portrait,
        Resolution::Xga,
    ];

    pub fn size(self) -> (u32, u32) {
        match self {
            Resolution::Hd => (1280, 720),
            Resolution::FullHd => (1920, 1080),
            Resolution::Portrait => (1080, 1920),
            Resolution::Xga => (1024, 768),
        }
    }
}

impl fmt::Display for Resolution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (width, height) = self.size();
        let kind = match self {
            Resolution::Hd => "HD",
            Resolution::FullHd => "Full HD",
            Resolution::Portrait => "portrait",
            Resolution::Xga => "4:3",
        };
        write!(f, "{width} x {height} ({kind})")
    }
}

/// The size picked until the user picks one: Full HD, unless this
/// computer's screen, in its own pixels, is known to be smaller. QEMU's
/// window shows the device pixel for pixel, so a bigger one would not fit.
/// A screen turned on its side counts the same.
pub fn default_for(screen: Option<(u32, u32)>) -> Resolution {
    match screen {
        Some((width, height)) if width.max(height) < 1920 || width.min(height) < 1080 => {
            Resolution::Hd
        }
        _ => Resolution::FullHd,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// A size the user picked; none is `default_for` this screen.
    pub resolution: Option<Resolution>,
    /// Sound through this computer's speakers.
    pub sound: bool,
    /// This computer's microphone as the device's input. Off by default:
    /// on, macOS asks for access on the first start.
    pub microphone: bool,
    /// The device draws on this computer's GPU, through VirGL, when the
    /// bundled QEMU has it. Off is software rendering, for a Mac whose GL
    /// path misbehaves.
    pub accelerated: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            resolution: None,
            sound: true,
            microphone: false,
            accelerated: true,
        }
    }
}

impl Settings {
    /// The size the device gets, and the picker shows, on a screen of
    /// `screen` pixels.
    pub fn resolution_on(&self, screen: Option<(u32, u32)>) -> Resolution {
        self.resolution.unwrap_or_else(|| default_for(screen))
    }

    /// The saved settings, or the defaults when there are none or they do
    /// not parse.
    pub fn load(dir: &Path) -> Self {
        std::fs::read(dir.join(FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> Result<(), String> {
        let path = dir.join(FILE);
        let body = serde_json::to_vec_pretty(self).map_err(|err| err.to_string())?;
        std::fs::create_dir_all(dir)
            .and_then(|()| std::fs::write(&path, body))
            .map_err(|err| format!("{}: {err}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_survive_a_save_and_load() {
        let dir = std::env::temp_dir().join(format!("try-tessaro-settings-{}", std::process::id()));
        let settings = Settings {
            resolution: Some(Resolution::Portrait),
            sound: false,
            microphone: true,
            accelerated: false,
        };
        settings.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir), settings);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn full_hd_unless_the_screen_is_known_to_be_smaller() {
        // A 14" MacBook Pro's Retina panel, a 1080p monitor turned on its
        // side, a screen not measured yet, and a 13" MacBook Air of old.
        assert_eq!(default_for(Some((3024, 1964))), Resolution::FullHd);
        assert_eq!(default_for(Some((1080, 1920))), Resolution::FullHd);
        assert_eq!(default_for(None), Resolution::FullHd);
        assert_eq!(default_for(Some((1440, 900))), Resolution::Hd);
    }

    #[test]
    fn a_picked_size_wins_over_the_default() {
        let mut settings = Settings::default();
        let retina = Some((3024, 1964));
        assert_eq!(settings.resolution_on(retina), Resolution::FullHd);
        settings.resolution = Some(Resolution::Xga);
        assert_eq!(settings.resolution_on(retina), Resolution::Xga);
    }

    #[test]
    fn a_file_from_before_acceleration_turns_it_on() {
        let settings: Settings =
            serde_json::from_str(r#"{"resolution":"hd","sound":true,"microphone":false}"#).unwrap();
        assert!(settings.accelerated);
    }

    #[test]
    fn a_missing_or_broken_file_gives_the_defaults() {
        let dir = std::env::temp_dir().join(format!("try-tessaro-broken-{}", std::process::id()));
        assert_eq!(Settings::load(&dir), Settings::default());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE), b"{not json").unwrap();
        assert_eq!(Settings::load(&dir), Settings::default());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
