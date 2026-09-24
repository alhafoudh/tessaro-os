//! The modes the connected displays advertise, straight from DRM.
//!
//! `/sys/class/drm/card0-HDMI-A-1/modes` lists every mode the connector's
//! EDID offers, one `WIDTHxHEIGHT` per line (repeated once per refresh rate),
//! preferred first. That list is what makes a resolution change safe to
//! offer at all: `display.resolution` only accepts a mode some connected
//! display actually listed, so a typo cannot drive a panel out of range.
//! `tessaro-weston-config` checks the same list again before it writes a
//! `mode=` line, and the change still reverts unless confirmed.

use std::fs;
use std::path::Path;

use protocol::Connector;

pub fn connectors(drm: &Path) -> Vec<Connector> {
    let Ok(entries) = fs::read_dir(drm) else {
        return Vec::new();
    };

    let mut found: Vec<Connector> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let file_name = entry.file_name();
            let file_name = file_name.to_str()?;
            // card0-HDMI-A-1 -> HDMI-A-1; plain card0 and renderD128 are not
            // connectors.
            let (card, name) = file_name.split_once('-')?;
            // A writeback connector is always "connected" with no modes; it
            // captures to memory and is not a display.
            if !card.starts_with("card") || name.starts_with("Writeback-") {
                return None;
            }

            let status = fs::read_to_string(entry.path().join("status")).ok()?;
            if status.trim() != "connected" {
                return None;
            }

            let mut modes: Vec<String> = Vec::new();
            for line in fs::read_to_string(entry.path().join("modes")).ok()?.lines() {
                let mode = line.trim();
                // Interlaced modes are listed as 1920x1080i; skip them.
                if protocol::keys::parse_mode(mode).is_some() && !modes.iter().any(|m| m == mode) {
                    modes.push(mode.to_string());
                }
            }

            Some(Connector {
                name: name.to_string(),
                modes,
            })
        })
        .collect();

    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// Is `mode` something a connected display offers? `preferred` always is.
pub fn offered(connectors: &[Connector], mode: &str) -> bool {
    mode == "preferred" || connectors.iter().any(|c| c.modes.iter().any(|m| m == mode))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connector(root: &Path, name: &str, status: &str, modes: &str) {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("status"), format!("{status}\n")).unwrap();
        fs::write(dir.join("modes"), modes).unwrap();
    }

    #[test]
    fn only_connected_connectors_and_progressive_modes() {
        let dir = tempfile::tempdir().unwrap();
        connector(
            dir.path(),
            "card0-HDMI-A-1",
            "connected",
            "1920x1080\n1920x1080\n1920x1080i\n1280x720\n",
        );
        connector(dir.path(), "card0-DP-1", "disconnected", "3840x2160\n");
        connector(dir.path(), "card0-Writeback-1", "connected", "");
        fs::create_dir_all(dir.path().join("card0")).unwrap();
        fs::create_dir_all(dir.path().join("renderD128")).unwrap();

        let found = connectors(dir.path());

        assert_eq!(
            found,
            vec![Connector {
                name: "HDMI-A-1".to_string(),
                modes: vec!["1920x1080".to_string(), "1280x720".to_string()],
            }]
        );
        assert!(offered(&found, "1280x720"));
        assert!(offered(&found, "preferred"));
        assert!(!offered(&found, "3840x2160"));
    }

    #[test]
    fn no_drm_is_no_connectors() {
        assert!(connectors(Path::new("/nonexistent/drm")).is_empty());
    }
}
