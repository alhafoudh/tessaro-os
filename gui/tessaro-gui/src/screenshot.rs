//! The README's screenshots of the app (`mise run docs:screenshots`): the
//! app as it boots, a device window opened on the made-up device the agent's
//! fixtures describe (`docs/screenshots/api/`), drawn headless. No worker
//! runs: the window gets what a worker would send, and every call a page
//! makes is answered from the fixtures. Ignored by `gui:test`; the task runs
//! it with `--ignored` and `shoot.mjs` turns the PNGs into the docs' JPEGs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use iced_test::Simulator;
use protocol::{KeyInfo, Settings, Status};
use tessaro_client::connect::Found;
use tessaro_client::nodes::Node;

use crate::device::{self, Scope};
use crate::worker::{Event, Request};
use crate::{discovery, mdi, App, Message, Prefs, CONFIG_SIZE, SETTINGS, WINDOW};

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

/// One of the agent's answers for the made-up device, by its API path.
fn answer<T: serde::de::DeserializeOwned>(path: &str) -> T {
    let file = format!("{ROOT}/docs/screenshots/{path}.json");
    let text = std::fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("{file}: {err}; run `mise run docs:screenshots`"));
    serde_json::from_str(&text).unwrap_or_else(|err| panic!("{file}: {err}"))
}

/// The answers to the calls the shot pages make, by their tag.
fn call_answers() -> BTreeMap<&'static str, &'static str> {
    BTreeMap::new()
}

/// Answer what the window asked, until it asks nothing more.
fn serve(app: &mut App, id: mdi::Id, requests: &mpsc::Receiver<Request>) {
    let answers = call_answers();
    while let Ok(request) = requests.try_recv() {
        if let Request::Call { tag, .. } = request {
            let path = answers
                .get(tag)
                .unwrap_or_else(|| panic!("no fixture for the {tag} call"));
            let event = Event::Answer(tag, Ok(answer(path)));
            let _ = app.update(Message::Worker(id, event));
        }
    }
}

/// The app as drawn now, as `<dir>/<name>-tiny-skia.png`.
fn shoot(app: &App, dir: &Path, name: &str) {
    let mut simulator = Simulator::with_size(crate::settings(), WINDOW, app.view());
    let snapshot = simulator.snapshot(&crate::theme::theme()).unwrap();
    let file = dir.join(format!("{name}-tiny-skia.png"));
    let _ = std::fs::remove_file(&file);
    // Writes the file, there being none to compare with.
    assert!(snapshot.matches_image(dir.join(name)).unwrap());
    assert!(file.exists(), "{} was not written", file.display());
}

#[test]
#[ignore = "renders the README's screenshots; run by `mise run docs:screenshots`"]
fn the_readme_screenshots() {
    let config = std::env::temp_dir().join(format!("tessaro-gui-shots-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&config);
    // SAFETY: the only test that sets either, run on its own by the task.
    unsafe {
        // No known nodes and no prefs but these: every window where it
        // opens by default, the settings window off the device window's
        // corner.
        std::env::set_var("TESSARO_CONFIG_DIR", &config);
        // The same pixels on every machine: no GPU.
        std::env::set_var("ICED_TEST_BACKEND", "tiny-skia");
    }
    Prefs {
        windows: [(
            SETTINGS.to_string(),
            mdi::Placement {
                x: 600.0,
                y: 360.0,
                width: CONFIG_SIZE.width,
                height: CONFIG_SIZE.height,
                maximized: false,
            },
        )]
        .into(),
        ..Prefs::default()
    }
    .save();

    let status: Status = answer("api/v1/device/status");
    let settings: Settings = answer("api/v1/config");
    let keys: Vec<KeyInfo> = answer("api/v1/config/keys");
    let info = status.node.clone();
    let address = "192.168.1.42:7400";

    let (mut app, _) = App::boot();
    // Announced on the network, unclaimed, as a fresh device is.
    let _ = app.update(Message::Discovery(discovery::Event::Seen(Found {
        name: info.name.clone(),
        address: address.parse().unwrap(),
        id: Some(info.id.clone()),
        fingerprint: Some(info.fingerprint.clone()),
        claimed: Some(false),
        tags: info.tags.clone(),
    })));
    app.open(Node {
        id: info.id.clone(),
        name: info.name.clone(),
        address: address.into(),
        fingerprint: String::new(),
        token: None,
        tags: info.tags.clone(),
    });
    let id = *app.devices.keys().next().unwrap();

    let (requests, asked) = mpsc::channel();
    for event in [
        Event::Ready(requests),
        Event::Connected(info),
        Event::Status(Box::new(status)),
        Event::Settings(settings),
        Event::Keys(keys),
    ] {
        let _ = app.update(Message::Worker(id, event));
    }
    serve(&mut app, id, &asked);

    let dir = PathBuf::from(format!("{ROOT}/build/screenshots"));
    std::fs::create_dir_all(&dir).unwrap();
    shoot(&app, &dir, "gui-overview");

    // The browser's: more of them than the device's own.
    let configure = device::Message::Configure(Scope::of("browser"));
    let _ = app.update(Message::Device(id, configure));
    serve(&mut app, id, &asked);
    shoot(&app, &dir, "gui-configure");

    let _ = std::fs::remove_dir_all(&config);
}
