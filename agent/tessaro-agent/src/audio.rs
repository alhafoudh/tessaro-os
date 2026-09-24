//! Sound: which output plays, which input records, and how loud, on PipeWire.
//!
//! PipeWire and WirePlumber run as the `weston` user in units of their own
//! (`tessaro-pipewire`, `tessaro-wireplumber`, `tessaro-pipewire-pulse`),
//! with their sockets in `/run/tessaro-audio`. Chromium reaches them through
//! the Pulse socket there. The audio.* settings are the only truth:
//! WirePlumber is told not to remember anything, and the agent applies the
//! settings to the running server - on a `set`, at startup, and whenever the
//! hardware changes (a USB speaker, a screen on HDMI).
//!
//! The graph is read with `pw-dump` (JSON) and changed with `wpctl`, by
//! object id. Both are small clients that connect, do one thing and exit;
//! every run is under a deadline. Choosing is pure (`resolve`), so what
//! `auto` picks is tested here, not on a device.
//!
//! Volumes are `wpctl`'s cubic scale, the one every desktop's slider uses: a
//! setting of 50 sounds about half as loud as 100. PipeWire itself stores the
//! linear value, the cube of that.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use protocol::keys;
use protocol::{AudioDevice, AudioSide, AudioStatus, AudioTested};
use serde_json::Value;

use crate::config::Env;
use crate::deadline::{blocking, within};
use crate::log::Log;
use crate::paths::Paths;

/// One `wpctl` or `pw-dump`: they connect over a local socket and exit.
const CALL: Duration = Duration::from_secs(5);
/// Playing the test tone, which is a second long.
const PLAY: Duration = Duration::from_secs(10);
/// How long `audio test --input` records.
const RECORD: Duration = Duration::from_secs(3);
/// A card switched to another profile brings its new node up a moment later.
const PROFILE_SETTLE: Duration = Duration::from_millis(500);

const DEFAULT_VOLUME: u8 = 80;
const DEFAULT_INPUT_VOLUME: u8 = 100;

/// What the audio.* settings ask for, effective (set, else image default).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    pub output: String,
    pub volume: u8,
    pub mute: bool,
    pub input: String,
    pub input_volume: u8,
}

impl Wanted {
    pub fn from_env(env: &dyn Env) -> Self {
        let text = |name: &str| {
            env.get(name)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "auto".to_string())
        };
        let percent = |name: &str, default: u8| {
            env.get(name)
                .and_then(|value| value.trim().parse::<u8>().ok())
                .map(|value| value.min(100))
                .unwrap_or(default)
        };
        Self {
            output: text("KIOSK_AUDIO_OUTPUT"),
            volume: percent("KIOSK_AUDIO_VOLUME", DEFAULT_VOLUME),
            mute: env.get("KIOSK_AUDIO_MUTE").as_deref() == Some("1"),
            input: text("KIOSK_AUDIO_INPUT"),
            input_volume: percent("KIOSK_AUDIO_INPUT_VOLUME", DEFAULT_INPUT_VOLUME),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Hdmi,
    Jack,
    Usb,
    Bluetooth,
    Other,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Hdmi => "hdmi",
            Kind::Jack => "jack",
            Kind::Usb => "usb",
            Kind::Bluetooth => "bluetooth",
            Kind::Other => "other",
        }
    }

    fn parse(word: &str) -> Option<Kind> {
        match word {
            "hdmi" => Some(Kind::Hdmi),
            "jack" => Some(Kind::Jack),
            "usb" => Some(Kind::Usb),
            "bluetooth" => Some(Kind::Bluetooth),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Output,
    Input,
}

impl Direction {
    fn word(self) -> &'static str {
        match self {
            Direction::Output => "output",
            Direction::Input => "input",
        }
    }
}

/// What audio.output or audio.input says, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Want {
    Auto,
    Off,
    Kind(Kind),
    Name(String),
}

impl Want {
    pub fn parse(value: &str) -> Want {
        match value {
            "" | "auto" => Want::Auto,
            "off" => Want::Off,
            word => match Kind::parse(word) {
                Some(kind) => Want::Kind(kind),
                None => Want::Name(word.to_string()),
            },
        }
    }
}

/// How something is brought into use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Via {
    /// A node that exists: made the default.
    Node(u32),
    /// A sound card profile that would bring the output up: switched to
    /// first. HDMI and analog are often profiles of one Intel HDA card.
    Profile { device: u32, index: u32 },
}

/// One output or input `resolve` can choose.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub name: String,
    pub description: String,
    pub kind: Kind,
    pub available: Option<bool>,
    /// PipeWire's object serial: never reused, so higher is newer - which is
    /// how `auto` knows the speaker plugged in last.
    pub serial: u64,
    pub via: Via,
    /// Cubic, 0 to 1 and a little over. Nodes only.
    pub volume: Option<f64>,
    pub muted: Option<bool>,
}

/// The graph, as far as sound goes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Graph {
    pub outputs: Vec<Candidate>,
    pub inputs: Vec<Candidate>,
    pub default_output: Option<String>,
    pub default_input: Option<String>,
    /// A screen is connected to an HDMI or DisplayPort connector, for
    /// outputs whose card cannot tell whether anything is plugged in (the
    /// Pi's vc4-hdmi).
    pub screen: bool,
}

impl Graph {
    pub fn candidates(&self, direction: Direction) -> &[Candidate] {
        match direction {
            Direction::Output => &self.outputs,
            Direction::Input => &self.inputs,
        }
    }

    /// The node WirePlumber uses now.
    fn current(&self, direction: Direction) -> Option<&str> {
        match direction {
            Direction::Output => self.default_output.as_deref(),
            Direction::Input => self.default_input.as_deref(),
        }
    }
}

/// What `resolve` chose, and why it is not what the setting says, if not.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub target: Option<Candidate>,
    pub fallback: Option<String>,
    /// The setting is `off`: the target, if any, is muted.
    pub off: bool,
}

/// Something is plugged in: the card says so, or it cannot tell and this is
/// HDMI with a screen connected, or it cannot tell at all.
fn connected(candidate: &Candidate, screen: bool) -> bool {
    match (candidate.available, candidate.kind) {
        (Some(available), _) => available,
        (None, Kind::Hdmi) => screen,
        (None, _) => true,
    }
}

/// Lower is better for `auto`. Something plugged in on purpose (USB,
/// Bluetooth) beats what is built in; HDMI with a screen beats the jack,
/// which cannot tell whether anything is plugged in on most boards.
fn rank(candidate: &Candidate, direction: Direction, screen: bool) -> u8 {
    let plugged = connected(candidate, screen);
    let base = match (direction, candidate.kind) {
        (_, Kind::Usb | Kind::Bluetooth) => 0,
        (Direction::Output, Kind::Hdmi) if plugged => 1,
        (_, Kind::Jack) => 2,
        (Direction::Output, Kind::Hdmi) => 4,
        (Direction::Input, Kind::Hdmi) => 5,
        (_, Kind::Other) => 3,
    };
    // A node that exists beats a profile switch of the same rank, and
    // anything known to be unplugged comes last.
    let profile = matches!(candidate.via, Via::Profile { .. }) as u8;
    let unplugged = if plugged { 0 } else { 10 };
    base * 2 + profile + unplugged
}

fn best<'a>(
    candidates: impl Iterator<Item = &'a Candidate>,
    direction: Direction,
    screen: bool,
) -> Option<&'a Candidate> {
    candidates.min_by(|a, b| {
        rank(a, direction, screen)
            .cmp(&rank(b, direction, screen))
            // The newest first.
            .then(b.serial.cmp(&a.serial))
    })
}

pub fn resolve(want: &Want, graph: &Graph, direction: Direction) -> Choice {
    let candidates = graph.candidates(direction);
    let auto = || best(candidates.iter(), direction, graph.screen).cloned();
    let choice = |target: Option<Candidate>, fallback: Option<String>| Choice {
        target,
        fallback,
        off: false,
    };
    match want {
        Want::Auto => choice(auto(), None),
        Want::Off => Choice {
            off: true,
            ..choice(auto(), None)
        },
        Want::Kind(kind) => {
            let of_kind: Vec<&Candidate> = candidates.iter().filter(|c| c.kind == *kind).collect();
            let plugged = best(
                of_kind
                    .iter()
                    .copied()
                    .filter(|c| connected(c, graph.screen)),
                direction,
                graph.screen,
            );
            match plugged {
                Some(target) => choice(Some(target.clone()), None),
                None => {
                    let why = if of_kind.is_empty() {
                        format!("there is no {} {}", kind.as_str(), direction.word())
                    } else {
                        format!("nothing is plugged into {}", kind.as_str())
                    };
                    choice(auto(), Some(why))
                }
            }
        }
        Want::Name(name) => match candidates.iter().find(|c| &c.name == name) {
            Some(target) => choice(Some(target.clone()), None),
            None => choice(auto(), Some(format!("{name} is not connected"))),
        },
    }
}

// --- reading pw-dump ---------------------------------------------------------

fn text(value: &Value, key: &str) -> String {
    match value.get(key) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        _ => String::new(),
    }
}

fn number(value: &Value, key: &str) -> Option<u64> {
    match value.get(key)? {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

/// `yes`, `no` or `unknown`, as PipeWire reports a port or a profile.
fn availability(value: &Value) -> Option<bool> {
    match value.get("available").and_then(Value::as_str) {
        Some("yes") => Some(true),
        Some("no") => Some(false),
        _ => None,
    }
}

#[derive(Debug, Default)]
struct Card {
    name: String,
    description: String,
    api: String,
    bus: String,
    alsa_name: String,
    profiles: Vec<Value>,
    current_profile: Option<u64>,
    routes: Vec<Value>,
}

fn classify(props: &Value, card: Option<&Card>) -> Kind {
    let api = card
        .map(|card| card.api.clone())
        .filter(|api| !api.is_empty())
        .unwrap_or_else(|| text(props, "device.api"));
    let name = text(props, "node.name");
    if api == "bluez5" || name.starts_with("bluez_") {
        return Kind::Bluetooth;
    }
    let bus = card
        .map(|card| card.bus.clone())
        .filter(|bus| !bus.is_empty())
        .unwrap_or_else(|| text(props, "device.bus"));
    if bus == "usb" || name.starts_with("alsa_output.usb-") || name.starts_with("alsa_input.usb-") {
        return Kind::Usb;
    }
    let haystack = [
        name.clone(),
        text(props, "device.profile.name"),
        text(props, "api.alsa.path"),
        text(props, "node.description"),
        card.map(|card| card.alsa_name.clone()).unwrap_or_default(),
    ]
    .join(" ")
    .to_ascii_lowercase();
    if haystack.contains("hdmi") || haystack.contains("displayport") {
        return Kind::Hdmi;
    }
    if api == "alsa" || name.starts_with("alsa_") {
        return Kind::Jack;
    }
    Kind::Other
}

/// The kind of output a profile brings up: `output:hdmi-stereo+input:...`.
fn profile_output(profile: &str, card: &Card) -> Option<Kind> {
    let output = profile
        .split('+')
        .find_map(|part| part.strip_prefix("output:"))?;
    Some(if card.bus == "usb" {
        Kind::Usb
    } else if output.contains("hdmi") {
        Kind::Hdmi
    } else {
        Kind::Jack
    })
}

/// A node's volume and mute from its `Props`, the volume on the cubic scale.
fn volume_of(info: &Value) -> (Option<f64>, Option<bool>) {
    let Some(props) = info
        .pointer("/params/Props")
        .and_then(Value::as_array)
        .and_then(|all| all.iter().find(|p| p.get("channelVolumes").is_some()))
    else {
        return (None, None);
    };
    let volumes: Vec<f64> = props
        .get("channelVolumes")
        .and_then(Value::as_array)
        .map(|all| all.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();
    let volume = (!volumes.is_empty()).then(|| {
        (volumes.iter().sum::<f64>() / volumes.len() as f64)
            .max(0.0)
            .cbrt()
    });
    (volume, props.get("mute").and_then(Value::as_bool))
}

/// Whether the port behind a node has something plugged in, from its card's
/// routes: the route whose `devices` include the node's profile device.
fn node_available(props: &Value, card: Option<&Card>, direction: Direction) -> Option<bool> {
    let card = card?;
    let device = number(props, "card.profile.device")?;
    let wanted = match direction {
        Direction::Output => "Output",
        Direction::Input => "Input",
    };
    let routes: Vec<&Value> = card
        .routes
        .iter()
        .filter(|route| route.get("direction").and_then(Value::as_str) == Some(wanted))
        .filter(|route| {
            route
                .get("devices")
                .and_then(Value::as_array)
                .is_some_and(|all| all.iter().any(|d| d.as_u64() == Some(device)))
        })
        .collect();
    // Any port of it with something plugged in counts; all unplugged is no.
    let states: Vec<Option<bool>> = routes.iter().copied().map(availability).collect();
    if states.contains(&Some(true)) {
        Some(true)
    } else if !states.is_empty() && states.iter().all(|s| *s == Some(false)) {
        Some(false)
    } else {
        None
    }
}

/// The graph from `pw-dump`'s output.
pub fn parse(dump: &str, screen: bool) -> Result<Graph, String> {
    let objects: Vec<Value> =
        serde_json::from_str(dump).map_err(|err| format!("pw-dump: {err}"))?;

    let mut cards: BTreeMap<u64, Card> = BTreeMap::new();
    for object in &objects {
        if object.get("type").and_then(Value::as_str) != Some("PipeWire:Interface:Device") {
            continue;
        }
        let Some(id) = number(object, "id") else {
            continue;
        };
        let info = object.get("info").cloned().unwrap_or(Value::Null);
        let props = info.get("props").cloned().unwrap_or(Value::Null);
        let params = info.get("params").cloned().unwrap_or(Value::Null);
        let list = |name: &str| {
            params
                .get(name)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        };
        cards.insert(
            id,
            Card {
                name: text(&props, "device.name"),
                description: text(&props, "device.description"),
                api: text(&props, "device.api"),
                bus: text(&props, "device.bus"),
                alsa_name: text(&props, "api.alsa.card.name"),
                profiles: list("EnumProfile"),
                current_profile: list("Profile").first().and_then(|p| number(p, "index")),
                routes: list("EnumRoute"),
            },
        );
    }

    let mut graph = Graph {
        screen,
        ..Graph::default()
    };
    // Which card each output node belongs to.
    let mut card_of: BTreeMap<u32, u64> = BTreeMap::new();
    for object in &objects {
        match object.get("type").and_then(Value::as_str) {
            Some("PipeWire:Interface:Node") => {}
            Some("PipeWire:Interface:Metadata") => {
                let props = object
                    .get("props")
                    .or_else(|| object.pointer("/info/props"))
                    .cloned()
                    .unwrap_or(Value::Null);
                if text(&props, "metadata.name") != "default" {
                    continue;
                }
                for entry in object
                    .get("metadata")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let name = entry
                        .pointer("/value/name")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    // What WirePlumber settled on; the configured one is what
                    // was asked for and may not exist.
                    match entry.get("key").and_then(Value::as_str) {
                        Some("default.audio.sink") => graph.default_output = name,
                        Some("default.audio.source") => graph.default_input = name,
                        _ => {}
                    }
                }
                continue;
            }
            _ => continue,
        }
        let Some(id) = number(object, "id") else {
            continue;
        };
        let info = object.get("info").cloned().unwrap_or(Value::Null);
        let props = info.get("props").cloned().unwrap_or(Value::Null);
        let direction = match text(&props, "media.class").as_str() {
            "Audio/Sink" => Direction::Output,
            "Audio/Source" => Direction::Input,
            _ => continue,
        };
        let name = text(&props, "node.name");
        // WirePlumber's placeholder when there is no sink at all.
        if name.is_empty() || name == "auto_null" || name.ends_with(".monitor") {
            continue;
        }
        let card_id = number(&props, "device.id");
        if let Some(card_id) = card_id {
            card_of.insert(id as u32, card_id);
        }
        let card = card_id.and_then(|id| cards.get(&id));
        let (volume, muted) = volume_of(&info);
        let description = [
            text(&props, "node.description"),
            text(&props, "node.nick"),
            name.clone(),
        ]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or_default();
        let candidate = Candidate {
            kind: classify(&props, card),
            available: node_available(&props, card, direction),
            serial: number(&props, "object.serial").unwrap_or(id),
            via: Via::Node(id as u32),
            name,
            description,
            volume,
            muted,
        };
        match direction {
            Direction::Output => graph.outputs.push(candidate),
            Direction::Input => graph.inputs.push(candidate),
        }
    }

    // Outputs a card only has in another profile: its HDMI while it plays
    // analog, or the other way round. One per kind per card - the profile
    // with the highest priority that is not known to be unplugged, keeping
    // an input when the current profile has one.
    for (id, card) in &cards {
        if card.api != "alsa" {
            continue;
        }
        let current_has_input = card
            .current_profile
            .and_then(|index| {
                card.profiles
                    .iter()
                    .find(|p| number(p, "index") == Some(index))
            })
            .is_some_and(|p| text(p, "name").contains("input:"));
        let mut best_of: BTreeMap<Kind, (&Value, (bool, u64))> = BTreeMap::new();
        for profile in &card.profiles {
            let name = text(profile, "name");
            let Some(kind) = profile_output(&name, card) else {
                continue;
            };
            if availability(profile) == Some(false) || name.starts_with("pro-audio") {
                continue;
            }
            let score = (
                current_has_input && name.contains("input:"),
                number(profile, "priority").unwrap_or(0),
            );
            if best_of.get(&kind).is_none_or(|(_, known)| score > *known) {
                best_of.insert(kind, (profile, score));
            }
        }
        for (kind, (profile, _)) in best_of {
            // The card already plays this kind of output as it is.
            let already = graph.outputs.iter().any(|o| {
                o.kind == kind && matches!(o.via, Via::Node(node) if card_of.get(&node) == Some(id))
            });
            let Some(index) = number(profile, "index") else {
                continue;
            };
            if already || card.current_profile == Some(index) {
                continue;
            }
            let profile_name = text(profile, "name");
            graph.outputs.push(Candidate {
                name: format!("{}:{profile_name}", card.name),
                description: format!("{} ({})", card.description, text(profile, "description")),
                kind,
                available: availability(profile),
                serial: 0,
                via: Via::Profile {
                    device: *id as u32,
                    index: index as u32,
                },
                volume: None,
                muted: None,
            });
        }
    }
    Ok(graph)
}

/// A screen on a connector that can carry sound.
pub fn screen_connected(drm: &Path) -> bool {
    crate::display::connectors(drm)
        .iter()
        .any(|c| c.name.starts_with("HDMI") || c.name.starts_with("DP"))
}

/// What the watcher compares to notice new hardware without asking PipeWire:
/// the sound cards the kernel has, the connectors' status, and the PipeWire
/// socket itself, which is new whenever the sound server restarted.
/// Blocking.
pub fn hardware(paths: &Paths) -> String {
    use std::os::unix::fs::MetadataExt;
    let cards = std::fs::read_to_string(&paths.asound_cards).unwrap_or_default();
    let socket = std::fs::metadata(paths.audio_runtime.join("pipewire-0"))
        .map(|meta| format!("{}:{}", meta.ino(), meta.mtime()))
        .unwrap_or_else(|_| "down".to_string());
    let mut connectors: Vec<String> = std::fs::read_dir(&paths.drm)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let status = std::fs::read_to_string(entry.path().join("status")).ok()?;
            Some(format!(
                "{}={}",
                entry.file_name().to_string_lossy(),
                status.trim()
            ))
        })
        .collect();
    connectors.sort();
    format!("{cards}\n{}\n{socket}", connectors.join(" "))
}

// --- the test tone and the level meter ---------------------------------------

const RATE: u32 = 48_000;

/// One second of 440 Hz, mono, 16-bit, at a quarter of full scale, faded in
/// and out so it does not click.
pub fn tone() -> Vec<u8> {
    let samples = RATE as usize;
    let fade = RATE as usize / 50;
    let mut pcm = Vec::with_capacity(samples * 2);
    for at in 0..samples {
        let envelope = (at.min(samples - 1 - at) as f64 / fade as f64).min(1.0);
        let phase = 2.0 * std::f64::consts::PI * 440.0 * at as f64 / RATE as f64;
        let sample = (phase.sin() * envelope * 0.25 * i16::MAX as f64) as i16;
        pcm.extend_from_slice(&sample.to_le_bytes());
    }
    wav(&pcm)
}

fn wav(pcm: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(pcm);
    out
}

/// Peak and RMS of a 16-bit WAV recording, in dBFS. The data chunk is read
/// to the end of the file whatever its header says: a recorder stopped by a
/// signal may never have written the final size.
pub fn level(recording: &[u8]) -> Option<(f64, f64)> {
    if recording.len() < 12 || &recording[..4] != b"RIFF" || &recording[8..12] != b"WAVE" {
        return None;
    }
    let mut at = 12;
    let data = loop {
        let header = recording.get(at..at + 8)?;
        let size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        if &header[..4] == b"data" {
            break &recording[at + 8..];
        }
        at += 8 + size + size % 2;
    };
    let samples: Vec<f64> = data
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]) as f64 / 32768.0)
        .collect();
    if samples.is_empty() {
        return None;
    }
    let peak = samples.iter().fold(0.0f64, |max, s| max.max(s.abs()));
    let rms = (samples.iter().map(|s| s * s).sum::<f64>() / samples.len() as f64).sqrt();
    let db = |value: f64| 20.0 * value.max(1e-5).log10();
    Some((db(peak), db(rms)))
}

// --- the adapter ---------------------------------------------------------------

/// What one apply did, in words, for the reply and the journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub summary: String,
    /// Nothing to play on or record from yet, or PipeWire not up: worth
    /// trying again soon rather than at the next slow tick.
    pub incomplete: bool,
}

pub struct Audio {
    log: Arc<Log>,
    runtime: PathBuf,
    drm: PathBuf,
    run_dir: PathBuf,
    /// One apply at a time: a `set` and the watcher must not interleave
    /// their `wpctl` calls.
    busy: tokio::sync::Mutex<()>,
}

impl Audio {
    pub fn new(log: Arc<Log>, paths: &Paths) -> Arc<Self> {
        Arc::new(Self {
            log,
            runtime: paths.audio_runtime.clone(),
            drm: paths.drm.clone(),
            run_dir: paths.run_dir.clone(),
            busy: tokio::sync::Mutex::new(()),
        })
    }

    fn command(&self, program: &str) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(program);
        command
            .env("PIPEWIRE_RUNTIME_DIR", &self.runtime)
            .env("XDG_RUNTIME_DIR", &self.runtime)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command
    }

    /// Run a PipeWire client to the end, under `limit`.
    async fn run(
        &self,
        what: &'static str,
        limit: Duration,
        program: &str,
        args: &[String],
    ) -> Result<Vec<u8>, String> {
        let output =
            crate::proc::run_async(self.command(program).args(args), None, what, limit).await?;
        if output.status.success() {
            return Ok(output.stdout);
        }
        let stderr = crate::proc::said(&output);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let said = [stderr.as_str(), stdout.trim()]
            .into_iter()
            .find(|s| !s.is_empty())
            .unwrap_or("failed")
            .to_string();
        Err(format!("{program} {}: {said}", args.join(" ")))
    }

    async fn wpctl(&self, args: &[String]) -> Result<(), String> {
        self.run("wpctl", CALL, "wpctl", args).await.map(|_| ())
    }

    /// The graph now, or why it cannot be read.
    pub async fn graph(&self) -> Result<Graph, String> {
        let socket = self.runtime.join("pipewire-0");
        let drm = self.drm.clone();
        let (up, screen) = blocking("looking for PipeWire", move || {
            Ok((socket.exists(), screen_connected(&drm)))
        })
        .await?;
        if !up {
            return Err(format!(
                "PipeWire is not running (no {})",
                self.runtime.join("pipewire-0").display()
            ));
        }
        let dump = self.run("pw-dump", CALL, "pw-dump", &[]).await?;
        parse(&String::from_utf8_lossy(&dump), screen)
    }

    /// Make the settings true on the running sound server. Idempotent: what
    /// already holds is left alone, and only a real change is logged.
    pub async fn apply(&self, wanted: &Wanted) -> Result<Outcome, String> {
        // naked: the in-process apply lock; every holder waits only on
        // bounded calls, so it is released in bounded time
        let _busy = self.busy.lock().await;
        let mut graph = self.graph().await?;

        // A profile switch first: the output it brings up does not exist
        // until then.
        // Never for `off`: switching a card only to mute it is pointless.
        let choice = resolve(&Want::parse(&wanted.output), &graph, Direction::Output);
        if let (
            false,
            Some(Candidate {
                via: Via::Profile { device, index },
                description,
                ..
            }),
        ) = (choice.off, &choice.target)
        {
            self.wpctl(&[
                "set-profile".to_string(),
                device.to_string(),
                index.to_string(),
            ])
            .await?;
            self.log
                .info(format!("audio: switched a sound card to {description}"));
            for _ in 0..6 {
                // naked: a timer, while the card brings its new node up
                tokio::time::sleep(PROFILE_SETTLE).await;
                graph = self.graph().await?;
                let now = resolve(&Want::parse(&wanted.output), &graph, Direction::Output);
                if matches!(
                    now.target,
                    Some(Candidate {
                        via: Via::Node(_),
                        ..
                    })
                ) {
                    break;
                }
            }
        }

        let output = self
            .apply_side(
                &graph,
                Direction::Output,
                &wanted.output,
                wanted.volume,
                wanted.mute,
            )
            .await?;
        let input = self
            .apply_side(
                &graph,
                Direction::Input,
                &wanted.input,
                wanted.input_volume,
                false,
            )
            .await?;
        Ok(Outcome {
            incomplete: output.1 || input.1,
            summary: format!("{}; {}", output.0, input.0),
        })
    }

    /// One side: default, volume, mute. Returns what it now is, in words,
    /// and whether there was nothing to use.
    async fn apply_side(
        &self,
        graph: &Graph,
        direction: Direction,
        setting: &str,
        volume: u8,
        mute: bool,
    ) -> Result<(String, bool), String> {
        let word = direction.word();
        let choice = resolve(&Want::parse(setting), graph, direction);
        let Some(target) = choice.target.clone() else {
            return Ok((format!("{word}: none"), setting != "off"));
        };
        let Via::Node(id) = target.via else {
            // The profile switch did not bring the node up in time.
            return Ok((format!("{word}: waiting for {}", target.description), true));
        };

        let mut changes = Vec::new();
        if graph.current(direction) != Some(target.name.as_str()) {
            self.wpctl(&["set-default".to_string(), id.to_string()])
                .await?;
            changes.push(format!(
                "{word} is now {} ({})",
                target.description,
                target.kind.as_str()
            ));
        }
        let level = f64::from(volume) / 100.0;
        if target.volume.is_none_or(|now| (now - level).abs() > 0.005) {
            self.wpctl(&[
                "set-volume".to_string(),
                id.to_string(),
                format!("{level:.2}"),
            ])
            .await?;
            changes.push(format!("{word} volume {volume}%"));
        }
        let muted = mute || choice.off;
        if target.muted != Some(muted) {
            self.wpctl(&[
                "set-mute".to_string(),
                id.to_string(),
                if muted { "1" } else { "0" }.to_string(),
            ])
            .await?;
            changes.push(format!(
                "{word} {}",
                if muted { "muted" } else { "unmuted" }
            ));
        }
        for change in &changes {
            self.log.info(format!("audio: {change}"));
        }
        if let Some(why) = &choice.fallback {
            if !changes.is_empty() {
                self.log.info(format!(
                    "audio: {word} {setting}: {why}, using {}",
                    target.name
                ));
            }
        }

        let state = if choice.off {
            "off".to_string()
        } else if muted {
            "muted".to_string()
        } else {
            format!("{volume}%")
        };
        let fallback = choice
            .fallback
            .map(|why| format!(" ({why})"))
            .unwrap_or_default();
        Ok((
            format!("{word}: {} {state}{fallback}", target.description),
            false,
        ))
    }

    /// Everything `tessaro-ctl audio show` prints. Never fails: without
    /// PipeWire it says why and shows the settings alone.
    pub async fn status(&self, wanted: &Wanted) -> AudioStatus {
        let side = |graph: &Graph, direction: Direction, setting: &str, volume: u8, mute: bool| {
            let choice = resolve(&Want::parse(setting), graph, direction);
            let in_use = choice.target.as_ref().map(|t| t.name.clone());
            let device = |c: &Candidate| AudioDevice {
                name: c.name.clone(),
                description: c.description.clone(),
                kind: c.kind.as_str().to_string(),
                available: c.available.or_else(|| {
                    (c.kind == Kind::Hdmi && direction == Direction::Output).then_some(graph.screen)
                }),
                in_use: in_use.as_deref() == Some(c.name.as_str()),
                needs_profile: matches!(c.via, Via::Profile { .. }),
            };
            AudioSide {
                setting: setting.to_string(),
                using: choice.target.as_ref().map(&device),
                fallback: choice.fallback.clone(),
                volume,
                muted: mute || choice.off,
                devices: graph.candidates(direction).iter().map(device).collect(),
            }
        };
        let (graph, running, error) = match self.graph().await {
            Ok(graph) => (graph, true, None),
            Err(err) => (Graph::default(), false, Some(err)),
        };
        let mut output = side(
            &graph,
            Direction::Output,
            &wanted.output,
            wanted.volume,
            wanted.mute,
        );
        let mut input = side(
            &graph,
            Direction::Input,
            &wanted.input,
            wanted.input_volume,
            false,
        );
        if !running {
            output.fallback = None;
            input.fallback = None;
        }
        AudioStatus {
            running,
            error,
            output,
            input,
        }
    }

    /// Whether `name` is an output (or input) the device has right now, for
    /// `set` to refuse one that is not.
    pub async fn check_name(&self, direction: Direction, name: &str) -> Result<(), String> {
        let graph = self
            .graph()
            .await
            .map_err(|err| format!("{name} cannot be checked: {err}"))?;
        if graph.candidates(direction).iter().any(|c| c.name == name) {
            return Ok(());
        }
        Err(format!(
            "this device has no {} called {name}; see `tessaro-ctl audio {}s`",
            direction.word(),
            direction.word()
        ))
    }

    /// Play the test tone on whatever the settings put in use.
    pub async fn test_output(&self, wanted: &Wanted) -> Result<AudioTested, String> {
        let graph = self.graph().await?;
        let choice = resolve(&Want::parse(&wanted.output), &graph, Direction::Output);
        let target = choice
            .target
            .ok_or_else(|| "there is no output to play on".to_string())?;
        let file = self.run_dir.join("audio-test.wav");
        let path = file.clone();
        blocking("writing the test tone", move || {
            std::fs::write(&path, tone()).map_err(|err| format!("{}: {err}", path.display()))
        })
        .await?;
        self.run("pw-play", PLAY, "pw-play", &[file.display().to_string()])
            .await?;
        let note = if choice.off {
            " - but audio.output is off, so it was muted"
        } else if wanted.mute {
            " - but audio.mute is on, so it was muted"
        } else {
            ""
        };
        Ok(AudioTested {
            message: format!(
                "played a 1s tone on {} at {}%{note}",
                target.description, wanted.volume
            ),
            peak_dbfs: None,
            rms_dbfs: None,
        })
    }

    /// Record a few seconds from the input in use and say how loud it was.
    pub async fn test_input(&self, wanted: &Wanted) -> Result<AudioTested, String> {
        let graph = self.graph().await?;
        let choice = resolve(&Want::parse(&wanted.input), &graph, Direction::Input);
        let target = choice
            .target
            .ok_or_else(|| "there is no input to record from".to_string())?;
        let file = self.run_dir.join("audio-record.wav");
        let rate = RATE.to_string();
        let mut child = self
            .command("pw-record")
            .args([
                "--rate",
                rate.as_str(),
                "--channels",
                "1",
                "--format",
                "s16",
            ])
            .arg(&file)
            .spawn()
            .map_err(|err| format!("pw-record: {err}"))?;
        // naked: a timer - the recording is exactly this long
        tokio::time::sleep(RECORD).await;
        // SIGINT, so it closes the file the way it would on Ctrl-C.
        if let Some(pid) = child.id() {
            // SAFETY: a signal to the child this function spawned and still
            // holds, so the pid cannot have been reused.
            unsafe {
                libc::kill(pid as i32, libc::SIGINT);
            }
        }
        if within("pw-record", CALL, child.wait()).await.is_err() {
            let _ = child.start_kill();
        }
        let path = file.clone();
        let recording = blocking("reading the recording", move || {
            std::fs::read(&path).map_err(|err| format!("{}: {err}", path.display()))
        })
        .await?;
        let (peak, rms) = level(&recording)
            .ok_or_else(|| "the recording is empty; is anything plugged in?".to_string())?;
        let verdict = if choice.off {
            "silence, as audio.input is off".to_string()
        } else if peak < -60.0 {
            "silence - is the microphone plugged in and unmuted?".to_string()
        } else if peak > -1.0 {
            "clipping - lower audio.input_volume".to_string()
        } else {
            "sound".to_string()
        };
        Ok(AudioTested {
            message: format!(
                "recorded {}s from {}: {verdict}",
                RECORD.as_secs(),
                target.description
            ),
            peak_dbfs: Some(peak),
            rms_dbfs: Some(rms),
        })
    }
}

/// The name a `set` has to check exists, and on which side.
pub fn named_device(key: &keys::Key, value: &str) -> Option<Direction> {
    if !keys::is_audio_device(value) {
        return None;
    }
    match key.kind {
        keys::Kind::AudioOutput => Some(Direction::Output),
        keys::Kind::AudioInput => Some(Direction::Input),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    fn node(id: u64, serial: u64, class: &str, name: &str, props: Value) -> Value {
        let mut all = json!({
            "media.class": class,
            "node.name": name,
            "node.description": name,
            "object.serial": serial,
        });
        for (key, value) in props.as_object().cloned().unwrap_or_default() {
            all[key] = value;
        }
        json!({
            "id": id,
            "type": "PipeWire:Interface:Node",
            "info": {
                "props": all,
                "params": { "Props": [ { "volume": 1.0, "mute": false, "channelVolumes": [0.512, 0.512] } ] }
            }
        })
    }

    fn card(id: u64, props: Value, profiles: Value, current: u64, routes: Value) -> Value {
        json!({
            "id": id,
            "type": "PipeWire:Interface:Device",
            "info": {
                "props": props,
                "params": {
                    "EnumProfile": profiles,
                    "Profile": [ { "index": current } ],
                    "EnumRoute": routes
                }
            }
        })
    }

    fn defaults(sink: &str, source: &str) -> Value {
        json!({
            "id": 30,
            "type": "PipeWire:Interface:Metadata",
            "props": { "metadata.name": "default" },
            "metadata": [
                { "subject": 0, "key": "default.audio.sink", "type": "Spa:String:JSON", "value": { "name": sink } },
                { "subject": 0, "key": "default.audio.source", "type": "Spa:String:JSON", "value": { "name": source } }
            ]
        })
    }

    /// A Raspberry Pi 3: HDMI from vc4, which cannot tell whether a screen is
    /// there, and the headphone jack from bcm2835.
    fn pi() -> String {
        json!([
            card(40, json!({ "device.api": "alsa", "device.bus": "", "device.name": "alsa_card.platform-3f902000.hdmi", "api.alsa.card.name": "vc4-hdmi" }), json!([]), 1, json!([])),
            card(41, json!({ "device.api": "alsa", "device.name": "alsa_card.platform-bcm2835_audio", "api.alsa.card.name": "bcm2835 Headphones" }), json!([]), 1, json!([])),
            node(50, 100, "Audio/Sink", "alsa_output.platform-3f902000.hdmi.hdmi-stereo", json!({ "device.id": 40, "device.api": "alsa" })),
            node(51, 101, "Audio/Sink", "alsa_output.platform-bcm2835_audio.stereo-fallback", json!({ "device.id": 41, "device.api": "alsa" })),
            defaults("alsa_output.platform-bcm2835_audio.stereo-fallback", ""),
        ])
        .to_string()
    }

    fn with_usb(dump: &str) -> String {
        let mut all: Vec<Value> = serde_json::from_str(dump).unwrap();
        all.push(card(42, json!({ "device.api": "alsa", "device.bus": "usb", "device.name": "alsa_card.usb-Generic_USB_Audio-00" }), json!([]), 1, json!([])));
        all.push(node(
            52,
            140,
            "Audio/Sink",
            "alsa_output.usb-Generic_USB_Audio-00.analog-stereo",
            json!({ "device.id": 42 }),
        ));
        all.push(node(
            53,
            141,
            "Audio/Source",
            "alsa_input.usb-Generic_USB_Audio-00.mono-fallback",
            json!({ "device.id": 42 }),
        ));
        Value::Array(all).to_string()
    }

    /// An Intel HDA card playing analog, with HDMI only in another profile,
    /// and a screen on HDMI.
    fn hda() -> String {
        json!([
            card(
                40,
                json!({ "device.api": "alsa", "device.bus": "pci", "device.name": "alsa_card.pci-0000_00_1f.3", "device.description": "Built-in Audio" }),
                json!([
                    { "index": 0, "name": "off", "priority": 0, "available": "yes" },
                    { "index": 1, "name": "output:analog-stereo+input:analog-stereo", "priority": 6565, "available": "yes", "description": "Analog Stereo Duplex" },
                    { "index": 2, "name": "output:hdmi-stereo", "priority": 5900, "available": "yes", "description": "Digital Stereo (HDMI) Output" },
                    { "index": 3, "name": "output:hdmi-stereo+input:analog-stereo", "priority": 5965, "available": "yes", "description": "Digital Stereo (HDMI) Output + Analog Stereo Input" },
                    { "index": 4, "name": "output:hdmi-stereo-extra1", "priority": 5700, "available": "no", "description": "Digital Stereo (HDMI 2) Output" }
                ]),
                1,
                json!([
                    { "index": 0, "direction": "Output", "name": "analog-output-headphones", "available": "no", "devices": [0] },
                    { "index": 1, "direction": "Output", "name": "analog-output-speaker", "available": "unknown", "devices": [0] },
                    { "index": 2, "direction": "Input", "name": "analog-input-mic", "available": "no", "devices": [1] }
                ])
            ),
            node(50, 100, "Audio/Sink", "alsa_output.pci-0000_00_1f.3.analog-stereo", json!({ "device.id": 40, "card.profile.device": 0 })),
            node(51, 101, "Audio/Source", "alsa_input.pci-0000_00_1f.3.analog-stereo", json!({ "device.id": 40, "card.profile.device": 1 })),
            defaults("alsa_output.pci-0000_00_1f.3.analog-stereo", "alsa_input.pci-0000_00_1f.3.analog-stereo"),
        ])
        .to_string()
    }

    fn names(choice: &Choice) -> &str {
        choice
            .target
            .as_ref()
            .map(|t| t.name.as_str())
            .unwrap_or("-")
    }

    #[test]
    fn a_pi_is_hdmi_and_a_jack() {
        let graph = parse(&pi(), true).unwrap();
        let kinds: Vec<(&str, Kind)> = graph
            .outputs
            .iter()
            .map(|o| (o.name.as_str(), o.kind))
            .collect();
        assert_eq!(
            kinds,
            [
                ("alsa_output.platform-3f902000.hdmi.hdmi-stereo", Kind::Hdmi),
                (
                    "alsa_output.platform-bcm2835_audio.stereo-fallback",
                    Kind::Jack
                ),
            ]
        );
        assert_eq!(
            graph.default_output.as_deref(),
            Some("alsa_output.platform-bcm2835_audio.stereo-fallback")
        );
        // 0.512 linear is 0.8 on the cubic scale wpctl uses.
        assert!((graph.outputs[0].volume.unwrap() - 0.8).abs() < 1e-9);
        assert_eq!(graph.outputs[0].muted, Some(false));
    }

    #[test]
    fn auto_plays_on_hdmi_with_a_screen_and_on_the_jack_without() {
        let screen = parse(&pi(), true).unwrap();
        assert_eq!(
            names(&resolve(&Want::Auto, &screen, Direction::Output)),
            "alsa_output.platform-3f902000.hdmi.hdmi-stereo"
        );
        let dark = parse(&pi(), false).unwrap();
        assert_eq!(
            names(&resolve(&Want::Auto, &dark, Direction::Output)),
            "alsa_output.platform-bcm2835_audio.stereo-fallback"
        );
    }

    #[test]
    fn auto_prefers_what_was_plugged_in_last() {
        let graph = parse(&with_usb(&pi()), true).unwrap();
        assert_eq!(
            names(&resolve(&Want::Auto, &graph, Direction::Output)),
            "alsa_output.usb-Generic_USB_Audio-00.analog-stereo"
        );
        assert_eq!(
            names(&resolve(&Want::Auto, &graph, Direction::Input)),
            "alsa_input.usb-Generic_USB_Audio-00.mono-fallback"
        );
    }

    #[test]
    fn a_kind_that_is_not_plugged_in_falls_back_to_auto_and_says_so() {
        let graph = parse(&pi(), true).unwrap();
        let choice = resolve(&Want::Kind(Kind::Usb), &graph, Direction::Output);
        assert_eq!(
            names(&choice),
            "alsa_output.platform-3f902000.hdmi.hdmi-stereo"
        );
        assert_eq!(choice.fallback.as_deref(), Some("there is no usb output"));

        let jack = resolve(&Want::parse("jack"), &graph, Direction::Output);
        assert_eq!(
            names(&jack),
            "alsa_output.platform-bcm2835_audio.stereo-fallback"
        );
        assert_eq!(jack.fallback, None);

        let gone = resolve(&Want::parse("alsa_output.usb-x"), &graph, Direction::Output);
        assert_eq!(
            gone.fallback.as_deref(),
            Some("alsa_output.usb-x is not connected")
        );
    }

    #[test]
    fn off_mutes_what_auto_would_play_on() {
        let graph = parse(&pi(), false).unwrap();
        let choice = resolve(&Want::Off, &graph, Direction::Output);
        assert!(choice.off);
        assert_eq!(
            names(&choice),
            "alsa_output.platform-bcm2835_audio.stereo-fallback"
        );
    }

    #[test]
    fn hdmi_in_another_profile_is_offered_and_chosen_with_a_screen() {
        let graph = parse(&hda(), true).unwrap();
        let hdmi: Vec<&Candidate> = graph
            .outputs
            .iter()
            .filter(|o| o.kind == Kind::Hdmi)
            .collect();
        assert_eq!(hdmi.len(), 1, "{hdmi:?}");
        // The profile that keeps the analog input, and never the unplugged
        // second HDMI port.
        assert_eq!(
            hdmi[0].name,
            "alsa_card.pci-0000_00_1f.3:output:hdmi-stereo+input:analog-stereo"
        );
        assert_eq!(
            hdmi[0].via,
            Via::Profile {
                device: 40,
                index: 3
            }
        );
        assert_eq!(
            names(&resolve(&Want::Kind(Kind::Hdmi), &graph, Direction::Output)),
            hdmi[0].name
        );
        // The analog output's ports are unplugged or unknown: unknown counts.
        let analog = graph.outputs.iter().find(|o| o.kind == Kind::Jack).unwrap();
        assert_eq!(analog.available, None);
        // The mic port reports nothing plugged in.
        assert_eq!(graph.inputs[0].available, Some(false));
    }

    #[test]
    fn nothing_at_all_resolves_to_nothing() {
        let graph = parse("[]", true).unwrap();
        let choice = resolve(&Want::Auto, &graph, Direction::Output);
        assert_eq!(choice.target, None);
        assert!(parse("not json", true).is_err());
    }

    #[test]
    fn settings_default_to_auto_at_eighty_percent() {
        let empty: HashMap<String, String> = HashMap::new();
        assert_eq!(
            Wanted::from_env(&empty),
            Wanted {
                output: "auto".into(),
                volume: 80,
                mute: false,
                input: "auto".into(),
                input_volume: 100,
            }
        );
        let set: HashMap<String, String> = [
            ("KIOSK_AUDIO_OUTPUT".to_string(), "hdmi".to_string()),
            ("KIOSK_AUDIO_VOLUME".to_string(), "35".to_string()),
            ("KIOSK_AUDIO_MUTE".to_string(), "1".to_string()),
        ]
        .into();
        let wanted = Wanted::from_env(&set);
        assert_eq!(
            (wanted.output.as_str(), wanted.volume, wanted.mute),
            ("hdmi", 35, true)
        );
    }

    #[test]
    fn the_tone_is_a_second_of_audio_and_its_level_reads_back() {
        let tone = tone();
        assert_eq!(tone.len(), 44 + RATE as usize * 2);
        let (peak, rms) = level(&tone).unwrap();
        // A quarter of full scale is -12 dBFS; a sine's RMS is 3 dB under.
        assert!((peak + 12.0).abs() < 0.2, "{peak}");
        assert!((rms + 15.0).abs() < 0.5, "{rms}");
    }

    #[test]
    fn a_recording_cut_short_still_has_a_level() {
        let mut wav = wav(&[0u8; 400]);
        // A recorder killed before it wrote the sizes.
        wav[40..44].copy_from_slice(&0u32.to_le_bytes());
        let (peak, _) = level(&wav).unwrap();
        assert!(peak < -90.0);
        assert_eq!(level(b"RIFF"), None);
        assert_eq!(level(&super::wav(&[])), None);
    }

    #[test]
    fn only_a_named_device_is_checked_at_set() {
        let output = keys::find("audio.output").unwrap();
        let volume = keys::find("audio.volume").unwrap();
        assert_eq!(
            named_device(output, "alsa_output.x"),
            Some(Direction::Output)
        );
        assert_eq!(named_device(output, "usb"), None);
        assert_eq!(named_device(volume, "alsa_output.x"), None);
    }
}
