//! Things to try on the device, each a card: what it shows, one click to do
//! it, and the `tessaro-ctl` commands that do the same, so trying it teaches
//! the CLI. The work is `agent/client`'s, as for both other clients.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use protocol::{api, keys};
use tessaro_client::text::{Line, Tone};
use tessaro_client::{config, describe};

use crate::device;

/// The welcome page, where the kiosk starts.
const WELCOME: &str = "http://127.0.0.1/";
/// The demo, beside it, which its button opens.
const DEMO: &str = "http://127.0.0.1/demo/";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Activity {
    Demo,
    Tone,
    Website,
    Maintenance,
    Screenshot,
}

impl Activity {
    pub const ALL: [Activity; 5] = [
        Activity::Demo,
        Activity::Tone,
        Activity::Website,
        Activity::Maintenance,
        Activity::Screenshot,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Activity::Demo => "Explore the demo",
            Activity::Tone => "Play a test tone",
            Activity::Website => "Show your own website",
            Activity::Maintenance => "Put up the maintenance screen",
            Activity::Screenshot => "Take a screenshot",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Activity::Demo => {
                "Every kiosk feature on the device's own screen: fonts, inputs, audio, video, devices and more."
            }
            Activity::Tone => "A one-second tone from the device, through this computer's speakers.",
            Activity::Website => {
                "Point the kiosk at any address. It stays there across restarts until you restore it."
            }
            Activity::Maintenance => {
                "Swap the page for a maintenance notice while you work, then switch it back."
            }
            Activity::Screenshot => "What the kiosk is showing right now, saved to your Downloads.",
        }
    }

    /// The same with `tessaro-ctl`, given `url` for the website card.
    pub fn commands(self, url: &str) -> Vec<String> {
        let url = if url.trim().is_empty() {
            "https://example.com"
        } else {
            url.trim()
        };
        match self {
            Activity::Demo => vec![
                format!("tessaro-ctl browser navigate {DEMO}"),
                format!("tessaro-ctl browser navigate {WELCOME}"),
            ],
            Activity::Tone => vec!["tessaro-ctl audio test".to_string()],
            Activity::Website => vec![
                format!("tessaro-ctl config set browser.url={url}"),
                "tessaro-ctl config unset browser.url".to_string(),
            ],
            Activity::Maintenance => vec![
                "tessaro-ctl browser maintenance on".to_string(),
                "tessaro-ctl browser maintenance off".to_string(),
            ],
            Activity::Screenshot => vec!["tessaro-ctl screen screenshot".to_string()],
        }
    }
}

/// What a card's button asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Demo,
    Welcome,
    Tone,
    ShowWebsite(String),
    RestoreWebsite,
    Maintenance(bool),
    Screenshot,
}

impl Action {
    pub fn activity(&self) -> Activity {
        match self {
            Action::Demo | Action::Welcome => Activity::Demo,
            Action::Tone => Activity::Tone,
            Action::ShowWebsite(_) | Action::RestoreWebsite => Activity::Website,
            Action::Maintenance(_) => Activity::Maintenance,
            Action::Screenshot => Activity::Screenshot,
        }
    }
}

/// What came of it, for the card to show.
#[derive(Debug, Clone)]
pub enum Outcome {
    Lines(Vec<Line>),
    Picture { jpeg: Vec<u8>, saved: PathBuf },
}

/// Do `action` on the device at `api`. Blocking.
pub fn run(api: u16, action: Action) -> Result<Outcome, String> {
    let mut session = device::open(api)?;
    if session.node.claimed && !session.has_token() {
        return Err(format!(
            "{} is claimed and this computer has no token for it: \
             `tessaro-ctl access login` with one, then try again",
            session.node.name
        ));
    }
    let said = |text: &str| Outcome::Lines(vec![Line::of(Tone::Ok, text)]);
    match action {
        Action::Demo | Action::Welcome => {
            let url = if action == Action::Demo {
                DEMO
            } else {
                WELCOME
            };
            let done = session.send::<api::browser::Navigate>(api::NavigateBody {
                url: url.to_string(),
            })?;
            Ok(said(&done.message))
        }
        Action::Tone => {
            let tested = session.send::<api::audio::Test>(api::AudioTestBody { input: false })?;
            Ok(Outcome::Lines(describe::audio::test(&tested)))
        }
        Action::ShowWebsite(url) => {
            let url = url.trim();
            if url.is_empty() {
                return Err("type an address first, e.g. https://example.com".to_string());
            }
            let values = BTreeMap::from([(keys::URL.to_string(), url.to_string())]);
            let applied = config::set(&mut session, values)?;
            Ok(Outcome::Lines(describe::device::applied(&applied, false)))
        }
        Action::RestoreWebsite => {
            let applied = config::unset(&mut session, vec![keys::URL.to_string()])?;
            Ok(Outcome::Lines(describe::device::applied(&applied, false)))
        }
        Action::Maintenance(on) => {
            let values = BTreeMap::from([(
                keys::MAINTENANCE_ENABLE.to_string(),
                if on { "1" } else { "0" }.to_string(),
            )]);
            config::set(&mut session, values)?;
            Ok(said(if on {
                "maintenance on - the screen shows the maintenance page"
            } else {
                "maintenance off - back on the kiosk page"
            }))
        }
        Action::Screenshot => {
            let jpeg = session
                .download::<api::screen::Screenshot>(api::Empty {})
                .into_result()?
                .body;
            let saved = save_screenshot(&jpeg)?;
            Ok(Outcome::Picture { jpeg, saved })
        }
    }
}

fn save_screenshot(jpeg: &[u8]) -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "no home directory".to_string())?;
    let dir = PathBuf::from(home).join("Downloads");
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let path = dir.join(format!("tessaro-screenshot-{stamp}.jpg"));
    std::fs::write(&path, jpeg).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_card_says_how_to_do_it_with_the_ctl() {
        for activity in Activity::ALL {
            let commands = activity.commands("");
            assert!(!commands.is_empty(), "{activity:?}");
            assert!(commands.iter().all(|line| line.starts_with("tessaro-ctl ")));
        }
    }

    #[test]
    fn the_website_card_shows_the_address_typed() {
        assert_eq!(
            Activity::Website.commands(" https://tessaro.example ")[0],
            "tessaro-ctl config set browser.url=https://tessaro.example"
        );
    }

    #[test]
    fn every_action_belongs_to_its_card() {
        assert_eq!(Action::Welcome.activity(), Activity::Demo);
        assert_eq!(Action::RestoreWebsite.activity(), Activity::Website);
        assert_eq!(Action::Maintenance(false).activity(), Activity::Maintenance);
    }
}
