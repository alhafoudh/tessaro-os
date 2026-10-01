//! The made-up device the README's screenshots show (`mise run
//! docs:screenshots`), and the API answers Webconfig and tessaro-gui are
//! rendered from. Every answer is what this agent sends, from a control plane
//! in a sandbox on the image's defaults, with what a sandbox cannot know
//! (the hardware, the browser, the units) filled in for the made-up device.
//! `UPDATE_SCREENSHOTS=1 cargo test` writes the files.

use std::collections::{BTreeMap, HashMap};
use std::fs;

use protocol::keys;
use protocol::{
    AudioDevice, AudioSide, AudioStatus, Command, FsUsage, Hardware, KeyInfo, MemUsage, Net,
    NetAddress, NetInterface, NodeInfo, Settings, Source, Status, TimeSummary, Via, WebSession,
};

use crate::control::{fixture_with, Caller};
use crate::state;

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
pub(crate) const NAME: &str = "golden-thistle-5731";
const ID: &str = "d77857317a77452baadbbde45de78ba7";
const URL: &str = "https://menu.example.com/";
const FINGERPRINT: &str = "9c41e0d8b7a35f2e6c1d4b8a0f73e5d29a6c8b1e4f07d3a5c2b9e6f1a8d4c731";

/// The made-up device's network: wired, with the hotspot up beside it.
fn net() -> Net {
    let address = |address: &str, prefix, family: &str, scope: &str| NetAddress {
        address: address.into(),
        prefix,
        family: family.into(),
        scope: scope.into(),
    };
    let interface = |name: &str, kind: &str, mac: &str, default_route, addresses| NetInterface {
        name: name.into(),
        kind: kind.into(),
        mac: Some(mac.into()),
        state: "up".into(),
        carrier: Some(true),
        mtu: Some(1500),
        speed_mbps: None,
        default_route,
        addresses,
    };
    Net {
        hostname: "tessaro".into(),
        interface: Some("eth0".into()),
        gateway: Some("192.168.1.1".into()),
        dns: vec!["192.168.1.1".into()],
        interfaces: vec![
            interface(
                "eth0",
                "ethernet",
                "d8:9e:f3:1c:57:31",
                true,
                vec![
                    address("192.168.1.42", 24, "ipv4", "global"),
                    address("fe80::da9e:f3ff:fe1c:5731", 64, "ipv6", "link-local"),
                ],
            ),
            interface(
                "wlan0",
                "wireless",
                "dc:a6:32:4e:57:31",
                false,
                vec![address("10.42.0.1", 24, "ipv4", "global")],
            ),
        ],
        public_ip: Some("203.0.113.7".into()),
        proxy: None,
    }
}

/// What the made-up device reports for the read-only keys.
pub(crate) fn live() -> state::Live {
    let mut values = crate::net::values(&net());
    values.insert("device.id".into(), ID.into());
    values.insert(
        "network.wifi.hotspot_ssid".into(),
        format!("tessaro-{NAME}"),
    );
    values.insert("storage.data_free".into(), "7.6 GB".into());
    values.insert("storage.data_size".into(), "8.2 GB".into());
    values.insert("storage.data_used".into(), "7%".into());
    state::Live {
        derived_name: Some(NAME.into()),
        values,
    }
}

/// The image's defaults as `tessaro-kiosk.env.in` has them, the values
/// bitbake fills in (`@...@`) as the qemu image's.
pub(crate) fn image_defaults() -> HashMap<String, String> {
    let env_in = fs::read_to_string(format!(
        "{ROOT}/meta-tessaro-distro/recipes-browser/tessaro-kiosk/files/tessaro-kiosk.env.in"
    ))
    .unwrap();
    let mut defaults: HashMap<String, String> = env_in
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .filter(|(_, value)| !value.contains('@'))
        .map(|(name, value)| {
            let value = value
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
                .unwrap_or(value);
            (name.to_string(), value.to_string())
        })
        .collect();
    defaults.insert("KIOSK_URL".into(), "http://127.0.0.1/".into());
    defaults.insert(
        "KIOSK_MAINTENANCE_URL".into(),
        "http://127.0.0.1/maintenance.html".into(),
    );
    defaults
}

/// The made-up device's answer to every request the shot pages make, by
/// the API path it answers.
async fn answers() -> BTreeMap<&'static str, serde_json::Value> {
    let fx = fixture_with(image_defaults());
    let ask = |command: Command| {
        let control = fx.control.clone();
        async move {
            control
                .handle(&Caller::Local, command)
                .await
                .result
                .expect("the sandbox answers")
        }
    };
    let saved: protocol::Applied = serde_json::from_value(
        ask(Command::Set {
            values: [("browser.url".to_string(), URL.to_string())].into(),
            if_revision: None,
            // Saved only: there is no systemd in the sandbox to restart.
            apply: false,
            verify: Default::default(),
        })
        .await,
    )
    .unwrap();
    assert_eq!(saved.changed, ["browser.url"]);

    let live = live();
    let read_only =
        |name: &str| keys::find(name).is_some_and(|key| key.kind == keys::Kind::ReadOnly);
    let live_value = |name: &str| Some(live.values.get(name).cloned().unwrap_or_default());

    let mut settings: Settings =
        serde_json::from_value(ask(Command::Get { key: None }).await).unwrap();
    for setting in &mut settings.settings {
        if setting.source == Source::Live {
            setting.value = live_value(&setting.key);
        }
    }
    let mut keys: Vec<KeyInfo> = serde_json::from_value(ask(Command::Keys).await).unwrap();
    for key in &mut keys {
        if read_only(&key.name) {
            key.value = live_value(&key.name);
        }
    }

    let mut status: Status = serde_json::from_value(ask(Command::Status).await).unwrap();
    status.node = NodeInfo {
        id: ID.into(),
        name: NAME.into(),
        machine: "raspberrypi5".into(),
        fingerprint: FINGERPRINT.into(),
        claimed: false,
        ..status.node
    };
    status.current_url = Some(URL.into());
    status.browser_answering = true;
    for state in status.units.values_mut() {
        *state = "active".into();
    }
    status.os = Some("Tessaro 0.1.0".into());
    status.image_version = Some("0.1.0".into());
    status.screen_on = Some(true);
    status.devtools = false;
    let hdmi = AudioDevice {
        name: "alsa_output.platform-107c701400.hdmi.hdmi-stereo".into(),
        description: "Built-in Audio Stereo".into(),
        kind: "hdmi".into(),
        available: Some(true),
        in_use: true,
        needs_profile: false,
    };
    status.audio = Some(AudioStatus {
        running: true,
        error: None,
        output: AudioSide {
            setting: "auto".into(),
            using: Some(hdmi.clone()),
            fallback: None,
            volume: 80,
            muted: false,
            devices: vec![hdmi],
        },
        input: AudioSide {
            setting: "auto".into(),
            using: None,
            fallback: Some("no microphone".into()),
            volume: 100,
            muted: false,
            devices: Vec::new(),
        },
    });
    status.data = Some(FsUsage {
        mountpoint: "/data".into(),
        source: "/dev/mmcblk0p3".into(),
        fstype: "ext4".into(),
        size: 8_200_000_000,
        used: 574_000_000,
        available: 7_626_000_000,
    });
    status.time = Some(TimeSummary {
        timezone: Some("Europe/Bratislava".into()),
        synchronized: Some(true),
        ntp: Some(true),
    });
    status.hardware = Some(Hardware {
        vendor: Some("Raspberry Pi".into()),
        model: Some("5 Model B Rev 1.0".into()),
        board: Some("Sony UK".into()),
        firmware: None,
        serial: Some("d77857317a77452b".into()),
        cpu: Some("BCM2712".into()),
        cores: Some(4),
        arch: "aarch64".into(),
    });
    status.memory = Some(MemUsage {
        total: 4_000_000_000,
        available: 2_900_000_000,
    });
    status.cpu_percent = Some(12);

    let session = WebSession {
        claimed: false,
        fresh: false,
        via: Via::Anonymous,
        token: None,
        timeout: 604_800,
    };

    let json = |value: serde_json::Result<serde_json::Value>| value.unwrap();
    [
        ("api/v1/access/session", json(serde_json::to_value(session))),
        ("api/v1/config", json(serde_json::to_value(settings))),
        ("api/v1/config/keys", json(serde_json::to_value(keys))),
        ("api/v1/device/status", json(serde_json::to_value(status))),
    ]
    .into()
}

#[tokio::test]
async fn the_checked_in_api_screenshot_fixtures_are_current() {
    let dir = format!("{ROOT}/docs/screenshots");
    let update = std::env::var_os("UPDATE_SCREENSHOTS").is_some();
    let mut stale = Vec::new();
    for (path, value) in answers().await {
        let file = format!("{dir}/{path}.json");
        let text = format!("{}\n", serde_json::to_string_pretty(&value).unwrap());
        if update {
            fs::create_dir_all(std::path::Path::new(&file).parent().unwrap()).unwrap();
            fs::write(&file, &text).unwrap();
        } else if fs::read_to_string(&file).unwrap_or_default() != text {
            stale.push(path);
        }
    }
    assert!(
        stale.is_empty(),
        "docs/screenshots/{stale:?} are out of date; run `mise run \
         docs:screenshots` and commit them with the screenshots"
    );
}
