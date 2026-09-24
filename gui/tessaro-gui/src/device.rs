//! A device's window: its settings, one section per group of keys, all
//! drawn the same way (`section.rs`), with the device's state along the
//! bottom and a log of what the changes did above it.
//!
//! The sections are the key prefixes the device reports (`browser`,
//! `screen`, ...), in the order it reports them, so a group a newer image
//! adds shows up without a change here. A row is edited in a dialog, the
//! way `tessaro-ctl config set` would set it: validated with the registry
//! the agent itself validates with, and sent with the revision it was read
//! at, so an edit made against stale values is refused instead of applied.

use std::collections::BTreeMap;
use std::sync::mpsc;
use std::time::Instant;

use iced::widget::image;
use iced::widget::{
    button, checkbox, column, container, pick_list, row, rule, scrollable, space, text, text_input,
    Column,
};
use iced::{Element, Length, Task};
use protocol::keys::{self, Consumer, Kind};
use protocol::{Applied, KeyInfo, NodeInfo, RestartTarget, Setting, Settings, Source, Status};
use tessaro_client::journal::Entry;
use tessaro_client::nodes::Node;

use crate::dialog::{self, field};
use crate::grid::{bold, cell, col, grid, Col};
use crate::{logs, vnc};

mod pages;
use crate::section::{self, action};
use crate::theme;
use crate::worker::{Event, Request};

/// The log keeps this many lines.
const LOG_LINES: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    Connecting,
    Online,
    Lost(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tone {
    Info,
    Ok,
    Warn,
    Bad,
}

struct Line {
    tone: Tone,
    text: String,
}

/// A line of a section's table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingRow {
    pub key: String,
    /// The key without its section's prefix.
    pub short: String,
    pub value: String,
    pub default: String,
    pub source: Source,
    pub applies: String,
    pub guarded: bool,
}

/// The sections, in the order the device lists its settings.
pub fn sections(settings: &Settings) -> Vec<String> {
    let mut sections: Vec<String> = Vec::new();
    for setting in &settings.settings {
        let group = group(&setting.key);
        if !sections.iter().any(|known| known == group) {
            sections.push(group.to_string());
        }
    }
    sections
}

fn group(key: &str) -> &str {
    key.split_once('.').map_or(key, |(group, _)| group)
}

/// What the device's registry says about `key`; a `data.*` key is
/// described by the template entry.
fn info<'a>(keys: &'a BTreeMap<String, KeyInfo>, key: &str) -> Option<&'a KeyInfo> {
    keys.get(key).or_else(|| {
        keys::param_name(key).and_then(|_| keys.get(&format!("{}<name>", keys::DATA_PREFIX)))
    })
}

pub fn rows(
    settings: &Settings,
    keys: &BTreeMap<String, KeyInfo>,
    section: &str,
) -> Vec<SettingRow> {
    settings
        .settings
        .iter()
        .filter(|setting| group(&setting.key) == section)
        .map(|setting| {
            let info = info(keys, &setting.key);
            SettingRow {
                key: setting.key.clone(),
                short: setting
                    .key
                    .strip_prefix(section)
                    .and_then(|rest| rest.strip_prefix('.'))
                    .unwrap_or(&setting.key)
                    .to_string(),
                value: setting.value.clone().unwrap_or_default(),
                default: info
                    .and_then(|info| info.default.clone())
                    .unwrap_or_default(),
                source: setting.source,
                applies: info.map(|info| applies(&info.applies)).unwrap_or_default(),
                guarded: info.is_some_and(|info| info.guarded),
            }
        })
        .collect()
}

fn applies(consumers: &[Consumer]) -> String {
    consumers
        .iter()
        .map(|consumer| match consumer {
            Consumer::Agent => "agent",
            Consumer::Browser => "browser",
            Consumer::Weston => "weston",
            Consumer::Network => "network",
            Consumer::Audio => "audio",
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The dialog's input, by the key's kind.
fn input(key: &str) -> Input {
    match keys::find(key).map(|key| key.kind) {
        Some(Kind::Flag) => Input::Flag,
        Some(Kind::Choice(choices)) => Input::Choice(choices),
        _ => Input::Text,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Input {
    Flag,
    Choice(&'static [&'static str]),
    Text,
}

/// `value` checked as the agent will check it at `set`. `Ok` carries the
/// value as it will be stored.
pub fn check(key: &str, value: &str) -> Result<String, String> {
    match keys::find(key) {
        Some(found) => keys::validate(found, value),
        // A key this build does not know, from a newer device: the device
        // has the last word.
        None => Ok(value.to_string()),
    }
}

struct Edit {
    key: String,
    /// A new `data.*` key: its name is typed in the dialog.
    adding: bool,
    name: String,
    value: String,
    error: Option<String>,
    busy: bool,
    close_after: bool,
}

impl Edit {
    /// The key being edited: for a new one, `data.` and the typed name.
    fn key(&self) -> String {
        if self.adding {
            format!("{}{}", keys::DATA_PREFIX, self.name.trim())
        } else {
            self.key.clone()
        }
    }

    /// Why it cannot be applied as it stands.
    fn problem(&self) -> Option<String> {
        if self.adding && !keys::is_param(self.name.trim()) {
            return Some("a name of lower-case letters, digits and _, up to 32".to_string());
        }
        check(&self.key(), &self.value).err()
    }
}

#[derive(Debug, Clone)]
enum Confirmable {
    Restart(RestartTarget),
    Reboot,
}

enum Dialog {
    Edit(Edit),
    Confirm(Confirmable),
    Form(pages::Form),
    /// Shown once: a new token, a password.
    Secret {
        title: String,
        intro: String,
        value: String,
    },
    Text {
        title: String,
        body: String,
    },
}

#[derive(Debug, Clone)]
pub enum Message {
    Section(String),
    Select(String),
    Activate(String),
    Filter(String),
    Edit,
    Add,
    Reset,
    Copy,
    Refresh,
    EditName(String),
    EditValue(String),
    EditFlag(bool),
    EditChoice(&'static str),
    EditOk,
    EditApply,
    EditDefault,
    ConfirmPending,
    Ask(Restart),
    Accept,
    Cancel,
    ToggleLog,
    ClearLog,
    Page(Page),
    /// Up (-1) or Down (1) in the table.
    Step(i32),
    Enter,
    TakeShot,
    LiveShot,
    SaveShot,
    JournalLive,
    JournalPause,
    JournalClear,
    JournalUnit(String),
    JournalUnitApply,
    JournalFilter(String),
    ToggleVnc,
    VncReconnect,
    /// Something on one of the pages beyond the settings.
    P(pages::Msg),
}

/// What the right side of a device window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Overview,
    /// The settings section in `Device::section`.
    Settings,
    Screen,
    Network,
    Wifi,
    Storage,
    Audio,
    Access,
    Ssh,
    Files,
    Update,
    Log,
}

impl Page {
    /// The pages below the settings sections in the nav, in its order.
    const TOOLS: &'static [(Page, &'static str)] = &[
        (Page::Screen, "Screen"),
        (Page::Network, "Network"),
        (Page::Wifi, "WiFi"),
        (Page::Storage, "Storage"),
        (Page::Audio, "Audio"),
        (Page::Access, "Access"),
        (Page::Ssh, "SSH"),
        (Page::Files, "Files"),
        (Page::Update, "Update"),
        (Page::Log, "Log"),
    ];
}

/// The journal kept for the Log page.
const JOURNAL_ENTRIES: usize = 5_000;
/// How many of them the table draws: the newest that pass the filter.
const JOURNAL_SHOWN: usize = 500;

struct Journal {
    live: bool,
    /// While paused, the table stays as it was; entries still arrive.
    paused: Option<usize>,
    unit: String,
    /// The unit the stream is filtered to; a change is a new stream.
    streaming_unit: Option<String>,
    generation: u64,
    entries: std::collections::VecDeque<Entry>,
    filter: String,
    state: Option<Result<(), String>>,
}

struct Shot {
    image: image::Handle,
    bytes: Vec<u8>,
    at: Instant,
}

/// The device actions on the toolbar, confirmed in a dialog first.
#[derive(Debug, Clone, Copy)]
pub enum Restart {
    Browser,
    Weston,
    Agent,
    Reboot,
}

pub struct Device {
    pub node: Node,
    pub link: Link,
    requests: Option<mpsc::Sender<Request>>,
    info: Option<NodeInfo>,
    status: Option<(Status, Instant)>,
    keys: BTreeMap<String, KeyInfo>,
    settings: Option<Settings>,
    section: Option<String>,
    selected: Option<String>,
    filter: String,
    log: Vec<Line>,
    log_open: bool,
    dialog: Option<Dialog>,
    page: Page,
    shot: Option<Shot>,
    shot_error: Option<String>,
    /// The Live toggle on the Screenshot page; shots are only taken while
    /// that page is shown.
    live_shots: bool,
    journal: Journal,
    vnc: Vnc,
    pages: pages::State,
    jobs: Vec<pages::Job>,
    next_job: u64,
}

/// The live VNC panel on the right of the window.
#[derive(Default)]
struct Vnc {
    open: bool,
    /// A new one reconnects (Reconnect).
    generation: u64,
    frame: Option<(image::Handle, u32, u32)>,
    state: Option<Result<String, String>>,
}

impl Device {
    pub fn new(node: Node) -> Self {
        Self {
            node,
            link: Link::Connecting,
            requests: None,
            info: None,
            status: None,
            keys: BTreeMap::new(),
            settings: None,
            section: None,
            selected: None,
            filter: String::new(),
            log: Vec::new(),
            log_open: true,
            dialog: None,
            page: Page::Overview,
            shot: None,
            shot_error: None,
            live_shots: false,
            journal: Journal {
                live: true,
                paused: None,
                unit: String::new(),
                streaming_unit: None,
                generation: 0,
                entries: std::collections::VecDeque::new(),
                filter: String::new(),
                state: None,
            },
            vnc: Vnc::default(),
            pages: pages::State::default(),
            jobs: Vec::new(),
            next_job: 0,
        }
    }

    /// The VNC stream to watch: while the panel is open.
    pub fn vnc_stream(&self) -> Option<u64> {
        self.vnc.open.then_some(self.vnc.generation)
    }

    /// What the VNC viewer said.
    pub fn vnc_event(&mut self, event: vnc::Event) {
        match event {
            vnc::Event::State(state) => self.vnc.state = Some(Ok(state)),
            vnc::Event::Frame {
                image,
                width,
                height,
            } => self.vnc.frame = Some((image, width, height)),
            vnc::Event::Lost(why) => self.vnc.state = Some(Err(why)),
        }
    }

    fn vnc_view(&self) -> Element<'_, Message> {
        let state: Element<'_, Message> = match &self.vnc.state {
            Some(Ok(state)) => text(state.clone())
                .size(theme::SMALL)
                .style(theme::muted)
                .into(),
            Some(Err(why)) => text(why.clone())
                .size(theme::SMALL)
                .style(text::danger)
                .into(),
            None => text("starting")
                .size(theme::SMALL)
                .style(theme::muted)
                .into(),
        };
        let off = self
            .setting("screen.vnc")
            .and_then(|setting| setting.value.as_deref())
            == Some("off");
        let picture: Element<'_, Message> = match (&self.vnc.frame, off) {
            (_, true) => text("VNC is off on this device (screen.vnc=off)")
                .size(theme::SMALL)
                .style(text::warning)
                .into(),
            (Some((frame, _, _)), false) => iced::widget::image(frame.clone())
                .content_fit(iced::ContentFit::Contain)
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
            (None, false) => text("no picture yet")
                .size(theme::SMALL)
                .style(theme::muted)
                .into(),
        };
        column![
            row![
                text("VNC").size(theme::SMALL).font(bold()),
                text("view only").size(theme::SMALL).style(theme::muted),
                space::horizontal(),
                theme::tool("Reconnect", Some(Message::VncReconnect)),
            ]
            .spacing(6)
            .height(24)
            .align_y(iced::alignment::Vertical::Center),
            container(picture)
                .width(Length::Fill)
                .height(Length::Fill)
                .center(Length::Fill)
                .style(theme::panel),
            state,
        ]
        .spacing(4)
        .into()
    }

    /// The device's name as it calls itself, else as nodes.json has it.
    pub fn name(&self) -> &str {
        self.info
            .as_ref()
            .map_or(&self.node.name, |info| &info.name)
    }

    pub fn title(&self) -> String {
        match &self.node.address {
            address if !address.is_empty() => format!("{} - {address}", self.name()),
            _ => self.name().to_string(),
        }
    }

    pub fn has_dialog(&self) -> bool {
        self.dialog.is_some()
    }

    /// The journal stream to follow: while the Log page is shown and Live.
    pub fn journal_stream(&self) -> Option<(Option<String>, u64)> {
        (self.page == Page::Log && self.journal.live)
            .then(|| (self.journal.streaming_unit.clone(), self.journal.generation))
    }

    /// What the journal stream said.
    pub fn journal_event(&mut self, event: logs::Event) {
        match event {
            logs::Event::Connected => self.journal.state = Some(Ok(())),
            logs::Event::Lost(why) => self.journal.state = Some(Err(why)),
            logs::Event::Entry(entry) => {
                self.journal.entries.push_back(*entry);
                if self.journal.entries.len() > JOURNAL_ENTRIES {
                    self.journal.entries.pop_front();
                    if let Some(paused) = &mut self.journal.paused {
                        *paused = paused.saturating_sub(1);
                    }
                }
            }
        }
    }

    /// Show `page`: live screenshots run only while theirs is shown.
    fn show(&mut self, page: Page) {
        if page == self.page {
            return;
        }
        if self.page == Page::Screen && self.live_shots {
            self.request(Request::Live(false));
        }
        self.page = page;
        if page == Page::Screen {
            if self.live_shots {
                self.request(Request::Live(true));
            } else if self.shot.is_none() {
                self.request(Request::Screenshot);
            }
        }
        self.refresh_page(page);
    }

    /// Write the last screenshot to the Downloads folder (else home).
    fn save_shot(&mut self) {
        let Some(shot) = &self.shot else {
            return;
        };
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(std::path::PathBuf::from)
            .unwrap_or_default();
        let downloads = home.join("Downloads");
        let dir = if downloads.is_dir() { downloads } else { home };
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        let path = dir.join(format!("{}-{stamp}.jpg", self.name()));
        match std::fs::write(&path, &shot.bytes) {
            Ok(()) => self.log(Tone::Ok, format!("saved {}", path.display())),
            Err(err) => self.log(Tone::Bad, format!("{}: {err}", path.display())),
        }
    }

    /// The rows of the settings section shown, as the table shows them.
    fn visible_rows(&self) -> Vec<SettingRow> {
        let (Some(settings), Some(section)) = (&self.settings, self.current_section()) else {
            return Vec::new();
        };
        rows(settings, &self.keys, &section)
            .into_iter()
            .filter(|row| section::matches(&self.filter, &[&row.short, &row.value, &row.default]))
            .collect()
    }

    fn log(&mut self, tone: Tone, text: impl Into<String>) {
        self.log.push(Line {
            tone,
            text: text.into(),
        });
        if self.log.len() > LOG_LINES {
            self.log.remove(0);
        }
    }

    /// Hand `request` to the worker; whether it took it.
    fn request(&mut self, request: Request) -> bool {
        let sent = self
            .requests
            .as_ref()
            .is_some_and(|requests| requests.send(request).is_ok());
        if !sent {
            self.log(Tone::Bad, "not connected");
        }
        sent
    }

    fn revision(&self) -> u64 {
        self.settings
            .as_ref()
            .map_or(0, |settings| settings.revision)
    }

    fn current_section(&self) -> Option<String> {
        let settings = self.settings.as_ref()?;
        let sections = sections(settings);
        self.section
            .clone()
            .filter(|section| sections.contains(section))
            .or_else(|| sections.first().cloned())
    }

    fn setting(&self, key: &str) -> Option<&Setting> {
        self.settings
            .as_ref()?
            .settings
            .iter()
            .find(|setting| setting.key == key)
    }

    /// What the worker said.
    pub fn event(&mut self, event: Event) {
        match event {
            Event::Ready(requests) => self.requests = Some(requests),
            Event::Connecting => self.link = Link::Connecting,
            Event::Connected(info) => {
                let was_online = self.link == Link::Online;
                if !was_online {
                    self.log(
                        Tone::Info,
                        format!("connected to {} ({})", info.name, info.id),
                    );
                }
                self.link = Link::Online;
                self.info = Some(info);
                if !was_online {
                    self.refresh_page(self.page);
                }
            }
            Event::Lost(why) => {
                if self.link != Link::Lost(why.clone()) {
                    self.log(Tone::Bad, format!("connection: {why}"));
                }
                self.link = Link::Lost(why);
                self.forget_in_flight();
            }
            Event::Status(status) => self.status = Some((*status, Instant::now())),
            Event::Settings(settings) => self.settings = Some(settings),
            Event::Keys(keys) => {
                self.keys = keys
                    .into_iter()
                    .map(|key| (key.name.clone(), key))
                    .collect();
            }
            Event::Applied(result) => self.applied(result.map(|applied| *applied)),
            Event::Done(Ok(message)) => self.log(Tone::Ok, message),
            Event::Done(Err(error)) => self.log(Tone::Bad, error),
            Event::Screenshot(Ok(bytes)) => {
                self.shot = Some(Shot {
                    image: image::Handle::from_bytes(bytes.clone()),
                    bytes,
                    at: Instant::now(),
                });
                self.shot_error = None;
            }
            Event::Screenshot(Err(error)) => self.shot_error = Some(error),
            Event::Answer(tag, result) => self.answer(tag, result),
            Event::Note(note) => self.log(Tone::Warn, note),
        }
    }

    fn applied(&mut self, result: Result<Applied, String>) {
        let editing = match &mut self.dialog {
            Some(Dialog::Edit(edit)) if edit.busy => Some(edit),
            _ => None,
        };
        match result {
            Ok(applied) => {
                let close = editing.is_some_and(|edit| {
                    edit.busy = false;
                    edit.close_after
                });
                if close {
                    self.dialog = None;
                }
                self.log_applied(&applied);
            }
            Err(error) => {
                if let Some(edit) = editing {
                    edit.busy = false;
                    edit.error = Some(error.clone());
                }
                if error.contains("someone else changed them") {
                    self.request(Request::Refresh);
                }
                self.log(Tone::Bad, error);
            }
        }
    }

    /// What `tessaro-ctl config set` prints about a change, as log lines.
    fn log_applied(&mut self, applied: &Applied) {
        if applied.changed.is_empty() {
            self.log(
                Tone::Info,
                format!("nothing changed (revision {})", applied.revision),
            );
            return;
        }
        self.log(
            Tone::Ok,
            format!(
                "revision {}: {}",
                applied.revision,
                applied.changed.join(", ")
            ),
        );
        if let Some(network) = &applied.network {
            for check in &network.checks {
                let tone = if check.passed { Tone::Ok } else { Tone::Bad };
                self.log(tone, format!("  {}: {}", check.name, check.detail));
            }
        }
        if let Some(audio) = &applied.audio {
            self.log(Tone::Info, audio.clone());
        }
        if !applied.restarted.is_empty() {
            self.log(
                Tone::Warn,
                format!("restarting {}", applied.restarted.join(", ")),
            );
        }
        if let Some(pending) = &applied.pending {
            self.log(
                Tone::Warn,
                format!(
                    "{}={} goes back to {} in {}s unless confirmed: check the screen, then Confirm",
                    pending.key,
                    pending.value,
                    pending.previous_or_default(),
                    pending.seconds_left
                ),
            );
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Section(section) => {
                self.show(Page::Settings);
                self.section = Some(section);
                self.selected = None;
            }
            Message::Page(page) => self.show(page),
            Message::Step(by) => {
                if self.dialog.is_none() && self.page == Page::Settings {
                    let keys: Vec<String> =
                        self.visible_rows().into_iter().map(|row| row.key).collect();
                    self.selected = section::step(&keys, self.selected.as_ref(), by);
                } else if self.dialog.is_none() {
                    self.primary_step(by);
                }
            }
            Message::Enter => {
                let next = match &self.dialog {
                    Some(Dialog::Edit(_)) => Some(Message::EditOk),
                    Some(Dialog::Confirm(_)) => Some(Message::Accept),
                    Some(Dialog::Form(_)) => Some(Message::P(pages::Msg::FormOk)),
                    Some(Dialog::Secret { .. } | Dialog::Text { .. }) => Some(Message::Cancel),
                    None if self.page == Page::Settings => {
                        self.selected.clone().map(Message::Activate)
                    }
                    None => self.primary_activate(),
                };
                if let Some(next) = next {
                    return self.update(next);
                }
            }
            Message::TakeShot => {
                self.request(Request::Screenshot);
            }
            Message::LiveShot => {
                self.live_shots = !self.live_shots;
                let on = self.live_shots;
                self.request(Request::Live(on));
            }
            Message::SaveShot => self.save_shot(),
            Message::JournalLive => self.journal.live = !self.journal.live,
            Message::JournalPause => {
                self.journal.paused = match self.journal.paused {
                    Some(_) => None,
                    None => Some(self.journal.entries.len()),
                };
            }
            Message::JournalClear => {
                self.journal.entries.clear();
                self.journal.paused = self.journal.paused.map(|_| 0);
            }
            Message::JournalUnit(unit) => self.journal.unit = unit,
            Message::JournalUnitApply => {
                let unit = self.journal.unit.trim();
                self.journal.streaming_unit = (!unit.is_empty()).then(|| unit.to_string());
                self.journal.generation += 1;
                self.journal.entries.clear();
                self.journal.state = None;
            }
            Message::JournalFilter(filter) => self.journal.filter = filter,
            Message::ToggleVnc => {
                self.vnc.open = !self.vnc.open;
                if !self.vnc.open {
                    self.vnc.frame = None;
                    self.vnc.state = None;
                }
            }
            Message::VncReconnect => {
                self.vnc.generation += 1;
                self.vnc.state = None;
            }
            Message::P(message) => return self.page_update(message),
            Message::Select(key) => self.selected = Some(key),
            Message::Activate(key) => {
                self.selected = Some(key);
                self.open_edit();
            }
            Message::Filter(filter) => self.filter = filter,
            Message::Edit => self.open_edit(),
            Message::Add => {
                self.dialog = Some(Dialog::Edit(Edit {
                    key: String::new(),
                    adding: true,
                    name: String::new(),
                    value: String::new(),
                    error: None,
                    busy: false,
                    close_after: false,
                }));
            }
            Message::Reset => {
                if let Some(key) = self.selected.clone() {
                    let if_revision = self.revision();
                    self.request(Request::Unset {
                        keys: vec![key],
                        if_revision,
                    });
                }
            }
            Message::Copy => {
                if let Some(setting) = self.selected.as_deref().and_then(|key| self.setting(key)) {
                    return iced::clipboard::write(setting.value.clone().unwrap_or_default());
                }
            }
            Message::Refresh => {
                self.request(Request::Refresh);
            }
            Message::EditName(name) => self.edit(|edit| edit.name = name),
            Message::EditValue(value) => self.edit(|edit| edit.value = value),
            Message::EditFlag(on) => {
                self.edit(|edit| edit.value = if on { "1" } else { "0" }.into())
            }
            Message::EditChoice(choice) => self.edit(|edit| edit.value = choice.to_string()),
            Message::EditOk => self.apply_edit(true),
            Message::EditApply => self.apply_edit(false),
            Message::EditDefault => {
                if let Some(Dialog::Edit(edit)) = &mut self.dialog {
                    if edit.adding || edit.busy {
                        return Task::none();
                    }
                    edit.busy = true;
                    edit.close_after = true;
                    let keys = vec![edit.key.clone()];
                    let if_revision = self.revision();
                    self.request(Request::Unset { keys, if_revision });
                }
            }
            Message::ConfirmPending => {
                self.request(Request::Confirm);
            }
            Message::Ask(what) => {
                self.dialog = Some(Dialog::Confirm(match what {
                    Restart::Browser => Confirmable::Restart(RestartTarget::Browser),
                    Restart::Weston => Confirmable::Restart(RestartTarget::Weston),
                    Restart::Agent => Confirmable::Restart(RestartTarget::Agent),
                    Restart::Reboot => Confirmable::Reboot,
                }));
            }
            Message::Accept => {
                if let Some(Dialog::Confirm(what)) = self.dialog.take() {
                    self.request(match what {
                        Confirmable::Restart(target) => Request::Restart(target),
                        Confirmable::Reboot => Request::Reboot,
                    });
                }
            }
            Message::Cancel => {
                // A change on its way stays in the dialog until it is answered.
                if !matches!(&self.dialog, Some(Dialog::Edit(edit)) if edit.busy) {
                    self.dialog = None;
                }
            }
            Message::ToggleLog => self.log_open = !self.log_open,
            Message::ClearLog => self.log.clear(),
        }
        Task::none()
    }

    fn edit(&mut self, change: impl FnOnce(&mut Edit)) {
        if let Some(Dialog::Edit(edit)) = &mut self.dialog {
            if !edit.busy {
                change(edit);
                edit.error = None;
            }
        }
    }

    /// The edit dialog for the selected row, unless it is read-only.
    fn open_edit(&mut self) {
        let Some(setting) = self.selected.as_deref().and_then(|key| self.setting(key)) else {
            return;
        };
        if setting.source == Source::Live {
            return;
        }
        // A default shows as the value it is, so editing starts from it.
        let value = setting.value.clone().unwrap_or_default();
        self.dialog = Some(Dialog::Edit(Edit {
            key: setting.key.clone(),
            adding: false,
            name: String::new(),
            value,
            error: None,
            busy: false,
            close_after: false,
        }));
    }

    fn apply_edit(&mut self, close_after: bool) {
        let if_revision = self.revision();
        let Some(Dialog::Edit(edit)) = &mut self.dialog else {
            return;
        };
        if edit.busy {
            return;
        }
        if let Some(problem) = edit.problem() {
            edit.error = Some(problem);
            return;
        }
        edit.busy = true;
        edit.close_after = close_after;
        let values = BTreeMap::from([(edit.key(), edit.value.clone())]);
        if values.keys().any(|key| crate::worker::is_network_key(key)) {
            self.log(
                Tone::Info,
                format!(
                    "changing the network; kept only if {} - this can take a minute",
                    protocol::Verify::default().describe()
                ),
            );
        }
        self.request(Request::Set {
            values,
            if_revision,
        });
    }

    pub fn view(&self) -> Element<'_, Message> {
        let body: Element<'_, Message> = match &self.settings {
            None => container(
                text(match &self.link {
                    Link::Lost(why) => format!("not connected: {why}"),
                    _ => "connecting ...".to_string(),
                })
                .size(theme::SMALL),
            )
            .padding(12)
            .into(),
            Some(settings) => {
                let content = match self.page {
                    Page::Settings => self.section_view(settings),
                    Page::Screen => column![self.modes_view(), self.screenshot_view()]
                        .spacing(8)
                        .into(),
                    Page::Log => self.journal_view(),
                    _ => self.page_view(),
                };
                let mut body = row![
                    self.nav(settings),
                    container(content).padding(6).width(Length::FillPortion(3))
                ]
                .height(Length::Fill);
                if self.vnc.open {
                    body = body.push(
                        container(self.vnc_view())
                            .padding(6)
                            .width(Length::FillPortion(2)),
                    );
                }
                body.into()
            }
        };

        let mut page = column![self.toolbar(), body];
        if self.log_open {
            page = page.push(self.log_view());
        }
        let page = page.push(self.status_bar());

        match &self.dialog {
            None => page.into(),
            Some(dialog) => dialog::modal(page.into(), self.dialog_view(dialog)),
        }
    }

    fn toolbar(&self) -> Element<'_, Message> {
        let online = self.link == Link::Online;
        let when = |message: Message| online.then_some(message);
        container(
            row![
                text(self.name()).size(theme::TEXT).font(bold()),
                text(&self.node.address)
                    .size(theme::SMALL)
                    .style(theme::muted),
                space::horizontal(),
                theme::tool("Refresh", when(Message::Refresh)),
                rule::vertical(1),
                theme::tool("Restart browser", when(Message::Ask(Restart::Browser))),
                theme::tool("Restart weston", when(Message::Ask(Restart::Weston))),
                theme::tool("Restart agent", when(Message::Ask(Restart::Agent))),
                theme::tool("Reboot", when(Message::Ask(Restart::Reboot))),
                rule::vertical(1),
                theme::toggle("VNC", self.vnc.open, Message::ToggleVnc),
                theme::toggle("Messages", self.log_open, Message::ToggleLog),
            ]
            .spacing(6)
            .height(24)
            .align_y(iced::alignment::Vertical::Center),
        )
        .padding([4, 8])
        .style(theme::status_bar)
        .into()
    }

    fn nav(&self, settings: &Settings) -> Element<'_, Message> {
        let current = self.current_section();
        let entry = |label: String, selected: bool, message: Message| -> Element<'_, Message> {
            button(text(label).size(theme::TEXT))
                .width(Length::Fill)
                .padding([4, 10])
                .style(theme::nav(selected))
                .on_press(message)
                .into()
        };
        let heading = |label: &'static str| -> Element<'_, Message> {
            container(text(label).size(theme::SMALL).style(theme::muted))
                .padding([6, 10])
                .into()
        };
        let mut entries: Vec<Element<'_, Message>> = vec![entry(
            "Overview".to_string(),
            self.page == Page::Overview,
            Message::Page(Page::Overview),
        )];
        entries.push(heading("Settings"));
        entries.extend(sections(settings).into_iter().map(|section| {
            let selected =
                self.page == Page::Settings && current.as_deref() == Some(section.as_str());
            entry(title_case(&section), selected, Message::Section(section))
        }));
        entries.push(heading("Tools"));
        for (page, label) in Page::TOOLS {
            entries.push(entry(
                label.to_string(),
                self.page == *page,
                Message::Page(*page),
            ));
        }
        container(scrollable(Column::with_children(entries).spacing(1)))
            .width(140)
            .height(Length::Fill)
            .padding([6, 0])
            .into()
    }

    fn screenshot_view(&self) -> Element<'_, Message> {
        let online = self.link == Link::Online;
        let mut toolbar = row![
            theme::tool("Take", online.then_some(Message::TakeShot)),
            theme::toggle(
                if self.live_shots {
                    "Live (3s): on"
                } else {
                    "Live (3s)"
                },
                self.live_shots,
                Message::LiveShot
            ),
            theme::tool("Save", self.shot.as_ref().map(|_| Message::SaveShot)),
            space::horizontal(),
        ]
        .spacing(4)
        .height(24)
        .align_y(iced::alignment::Vertical::Center);
        if let Some(error) = &self.shot_error {
            toolbar = toolbar.push(text(error).size(theme::SMALL).style(text::danger));
        } else if let Some(shot) = &self.shot {
            toolbar = toolbar.push(
                text(format!("taken {}s ago", shot.at.elapsed().as_secs()))
                    .size(theme::SMALL)
                    .style(theme::muted),
            );
        }
        let picture: Element<'_, Message> = match &self.shot {
            Some(shot) => iced::widget::image(shot.image.clone())
                .content_fit(iced::ContentFit::Contain)
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
            None => text("no screenshot yet")
                .size(theme::SMALL)
                .style(theme::muted)
                .into(),
        };
        column![
            toolbar,
            container(picture)
                .width(Length::Fill)
                .height(Length::Fill)
                .center(Length::Fill)
                .style(theme::panel)
        ]
        .spacing(4)
        .into()
    }

    fn journal_view(&self) -> Element<'_, Message> {
        let journal = &self.journal;
        let end = journal
            .paused
            .unwrap_or(journal.entries.len())
            .min(journal.entries.len());
        let shown: Vec<&Entry> = journal
            .entries
            .range(..end)
            .rev()
            .filter(|entry| section::matches(&journal.filter, &[&entry.source, &entry.message]))
            .take(JOURNAL_SHOWN)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        const COLUMNS: &[Col] = &[
            col("Time (UTC)", Length::Fixed(80.0)),
            col("Source", Length::Fixed(150.0)),
            col("Message", Length::Fill),
        ];
        let cells = shown.iter().map(|entry| {
            let message = cell(entry.message.clone());
            let message: Element<'_, Message> = match entry.priority {
                Some(0..=3) => message.style(text::danger).into(),
                Some(4) => message.style(text::warning).into(),
                Some(7) => message.style(theme::muted).into(),
                _ => message.into(),
            };
            vec![
                cell(entry.clock()).style(theme::muted).into(),
                cell(entry.source.clone()).into(),
                message,
            ]
        });
        let table = crate::grid::grid_following(COLUMNS, cells.collect(), journal.paused.is_none());

        let state: Element<'_, Message> = match (&journal.state, journal.live) {
            (_, false) => text("stopped")
                .size(theme::SMALL)
                .style(theme::muted)
                .into(),
            (Some(Err(why)), true) => text(why.clone())
                .size(theme::SMALL)
                .style(text::danger)
                .into(),
            (Some(Ok(())), true) => text("following")
                .size(theme::SMALL)
                .style(text::success)
                .into(),
            (None, true) => text("connecting")
                .size(theme::SMALL)
                .style(theme::muted)
                .into(),
        };
        let toolbar = row![
            theme::toggle("Live", journal.live, Message::JournalLive),
            theme::toggle("Pause", journal.paused.is_some(), Message::JournalPause),
            theme::tool("Clear", Some(Message::JournalClear)),
            rule::vertical(1),
            text_input("unit, e.g. tessaro-agent", &journal.unit)
                .on_input(Message::JournalUnit)
                .on_submit(Message::JournalUnitApply)
                .size(theme::SMALL)
                .padding([2, 6])
                .width(200),
            state,
            space::horizontal(),
            text_input("Find", &journal.filter)
                .on_input(Message::JournalFilter)
                .size(theme::SMALL)
                .padding([2, 6])
                .width(180),
        ]
        .spacing(6)
        .height(24)
        .align_y(iced::alignment::Vertical::Center);
        column![toolbar, table].spacing(4).into()
    }

    fn section_view<'a>(&'a self, settings: &'a Settings) -> Element<'a, Message> {
        let Some(section) = self.current_section() else {
            return text("this device reports no settings")
                .size(theme::SMALL)
                .into();
        };
        let rows: Vec<SettingRow> = rows(settings, &self.keys, &section)
            .into_iter()
            .filter(|row| section::matches(&self.filter, &[&row.short, &row.value, &row.default]))
            .collect();
        let selected_at = self
            .selected
            .as_ref()
            .and_then(|key| rows.iter().position(|row| &row.key == key));
        let selected = selected_at.map(|at| &rows[at]);

        const COLUMNS: &[Col] = &[
            col("", Length::Fixed(18.0)),
            col("Key", Length::Fixed(190.0)),
            col("Value", Length::FillPortion(3)),
            col("Default", Length::FillPortion(2)),
            col("Source", Length::Fixed(70.0)),
            col("Applies", Length::Fixed(110.0)),
        ];
        let cells = rows.iter().map(|row| {
            let muted = row.source != Source::Set;
            let value = cell(row.value.clone());
            vec![
                if row.guarded {
                    iced::widget::tooltip(
                        cell("!").style(text::warning),
                        text("guarded: reverts on its own unless confirmed").size(theme::SMALL),
                        iced::widget::tooltip::Position::Right,
                    )
                    .into()
                } else {
                    cell("").into()
                },
                self.with_doc(&row.key, cell(row.short.clone()).into()),
                if muted {
                    value.style(theme::muted).into()
                } else {
                    value.into()
                },
                cell(row.default.clone()).style(theme::muted).into(),
                match row.source {
                    Source::Set => cell("set").into(),
                    Source::Default => cell("default").style(theme::muted).into(),
                    Source::Live => cell("read-only").style(theme::muted).into(),
                },
                cell(row.applies.clone()).style(theme::muted).into(),
            ]
        });
        let keys: Vec<String> = rows.iter().map(|row| row.key.clone()).collect();
        let keys_too = keys.clone();
        let table = grid(
            COLUMNS,
            cells.collect(),
            selected_at,
            move |at| Message::Select(keys[at].clone()),
            move |at| Message::Activate(keys_too[at].clone()),
        );

        let online = self.link == Link::Online;
        let editable = selected.filter(|row| online && row.source != Source::Live);
        let mut list = vec![action("Refresh", online.then_some(Message::Refresh))];
        if section == keys::DATA_PREFIX.trim_end_matches('.') {
            list.push(action("Add", online.then_some(Message::Add)));
        }
        section::view(
            list,
            vec![
                action("Edit", editable.map(|_| Message::Edit)),
                action(
                    "Reset to default",
                    editable
                        .filter(|row| row.source == Source::Set)
                        .map(|_| Message::Reset),
                ),
                action("Copy value", selected.map(|_| Message::Copy)),
            ],
            &self.filter,
            Message::Filter,
            table,
        )
    }

    /// `content` with the key's documentation as its tooltip.
    fn with_doc<'a>(&'a self, key: &str, content: Element<'a, Message>) -> Element<'a, Message> {
        match info(&self.keys, key) {
            Some(info) if !info.doc.is_empty() => iced::widget::tooltip(
                content,
                container(text(&info.doc).size(theme::SMALL))
                    .max_width(420)
                    .padding(6)
                    .style(theme::panel),
                iced::widget::tooltip::Position::FollowCursor,
            )
            .into(),
            _ => content,
        }
    }

    fn log_view(&self) -> Element<'_, Message> {
        let lines = Column::with_children(self.log.iter().map(|line| {
            let text = text(&line.text)
                .size(theme::SMALL)
                .font(iced::Font::MONOSPACE);
            match line.tone {
                Tone::Info => text.into(),
                Tone::Ok => text.style(iced::widget::text::success).into(),
                Tone::Warn => text.style(iced::widget::text::warning).into(),
                Tone::Bad => text.style(iced::widget::text::danger).into(),
            }
        }));
        container(
            column![
                row![
                    text("Messages").size(theme::SMALL).font(bold()),
                    space::horizontal(),
                    theme::tool("Clear", Some(Message::ClearLog)),
                ]
                .align_y(iced::alignment::Vertical::Center),
                scrollable(lines)
                    .anchor_bottom()
                    .width(Length::Fill)
                    .height(110),
            ]
            .spacing(2),
        )
        .padding([4, 6])
        .style(theme::panel)
        .into()
    }

    fn status_bar(&self) -> Element<'_, Message> {
        let small = |value: String| text(value).size(theme::SMALL);
        let link = match &self.link {
            Link::Online => small("online".into()).style(iced::widget::text::success),
            Link::Connecting => small("connecting".into()).style(iced::widget::text::warning),
            Link::Lost(_) => small("offline".into()).style(iced::widget::text::danger),
        };
        let mut bar = row![link]
            .spacing(12)
            .align_y(iced::alignment::Vertical::Center);
        if let Some(info) = &self.info {
            bar = bar.push(small(format!("{} ({})", info.name, info.id)));
        }
        if let Some((status, at)) = &self.status {
            if let Some(image) = status.image_version.as_ref().or(status.os.as_ref()) {
                bar = bar.push(small(image.clone()).style(theme::muted));
            }
            bar = bar.push(small(format!("revision {}", status.revision)).style(theme::muted));
            bar = bar.push(if status.browser_answering {
                small("browser answering".into())
            } else {
                small("browser not answering".into()).style(iced::widget::text::danger)
            });
            if status.maintenance {
                bar = bar.push(small("maintenance".into()).style(iced::widget::text::warning));
            }
            if status.debug_screen {
                bar = bar.push(small("debug screen".into()).style(iced::widget::text::warning));
            }
            if let Some(data) = &status.data {
                if let Some(percent) = (data.used * 100).checked_div(data.size) {
                    bar = bar.push(small(format!("/data {percent}% used")).style(theme::muted));
                }
            }
            bar = bar.push(space::horizontal());
            if let Some(pending) = &status.pending {
                let left = pending.seconds_left.saturating_sub(at.elapsed().as_secs());
                bar = bar
                    .push(
                        small(format!(
                            "{}={} reverts to {} in {left}s",
                            pending.key,
                            pending.value,
                            pending.previous_or_default()
                        ))
                        .style(iced::widget::text::warning),
                    )
                    .push(theme::default_button(
                        "Confirm",
                        Some(Message::ConfirmPending),
                    ));
            }
        }
        container(bar)
            .width(Length::Fill)
            .padding([3, 8])
            .style(theme::status_bar)
            .into()
    }

    fn dialog_view<'a>(&'a self, dialog: &'a Dialog) -> Element<'a, Message> {
        match dialog {
            Dialog::Confirm(what) => {
                let name = self
                    .info
                    .as_ref()
                    .map_or(&self.node.name, |info| &info.name);
                let (title, body, label) = match what {
                    Confirmable::Restart(RestartTarget::Browser) => (
                        "Restart the browser",
                        format!("Restart the browser on {name}? The screen goes blank for a moment."),
                        "Restart",
                    ),
                    Confirmable::Restart(RestartTarget::Weston) => (
                        "Restart weston",
                        format!("Restart the compositor on {name}? The browser and the agent restart with it."),
                        "Restart",
                    ),
                    Confirmable::Restart(RestartTarget::Agent) => (
                        "Restart the agent",
                        format!("Restart tessaro-agent on {name}? This window reconnects by itself."),
                        "Restart",
                    ),
                    Confirmable::Reboot => (
                        "Reboot",
                        format!("Reboot {name}? It is gone for as long as it takes to boot."),
                        "Reboot",
                    ),
                };
                dialog::frame(
                    title.to_string(),
                    text(body).size(theme::SMALL).into(),
                    vec![
                        theme::default_button(label, Some(Message::Accept)),
                        theme::dialog_button("Cancel", Some(Message::Cancel)),
                    ],
                )
            }
            Dialog::Edit(edit) => self.edit_view(edit),
            Dialog::Form(form) => self.form_view(form),
            Dialog::Secret {
                title,
                intro,
                value,
            } => dialog::frame(
                title.clone(),
                column![
                    text(intro).size(theme::SMALL).style(text::warning),
                    row![
                        text(value).size(theme::TEXT).font(iced::Font::MONOSPACE),
                        theme::tool("Copy", Some(Message::P(pages::Msg::Copy(value.clone())))),
                    ]
                    .spacing(8)
                    .align_y(iced::alignment::Vertical::Center),
                ]
                .spacing(10)
                .into(),
                vec![theme::default_button("Done", Some(Message::Cancel))],
            ),
            Dialog::Text { title, body } => dialog::frame(
                title.clone(),
                scrollable(text(body).size(theme::SMALL).font(iced::Font::MONOSPACE))
                    .height(Length::Shrink)
                    .into(),
                vec![
                    theme::dialog_button("Copy", Some(Message::P(pages::Msg::Copy(body.clone())))),
                    theme::default_button("Close", Some(Message::Cancel)),
                ],
            ),
        }
    }

    fn edit_view<'a>(&'a self, edit: &'a Edit) -> Element<'a, Message> {
        let key = edit.key();
        let info = info(&self.keys, &key);
        let mut body = column![].spacing(8);
        if edit.adding {
            body = body.push(field(
                "Name",
                row![
                    text(keys::DATA_PREFIX).size(theme::SMALL),
                    text_input("table", &edit.name)
                        .on_input(Message::EditName)
                        .size(theme::SMALL),
                ]
                .align_y(iced::alignment::Vertical::Center),
            ));
        } else {
            body = body.push(field(
                "Key",
                text(&edit.key).size(theme::SMALL).font(bold()),
            ));
        }

        let value: Element<'a, Message> = match input(&key) {
            Input::Flag => checkbox(matches!(edit.value.trim(), "1" | "on" | "true" | "yes"))
                .label("on")
                .on_toggle(Message::EditFlag)
                .size(14)
                .text_size(theme::SMALL)
                .into(),
            Input::Choice(choices) => pick_list(
                choices,
                choices.iter().find(|choice| **choice == edit.value.trim()),
                |choice: &'static str| Message::EditChoice(choice),
            )
            .text_size(theme::SMALL)
            .padding([2, 6])
            .into(),
            Input::Text => text_input("", &edit.value)
                .on_input(Message::EditValue)
                .on_submit(Message::EditOk)
                .size(theme::SMALL)
                .into(),
        };
        body = body.push(field("Value", value));
        if let Some(info) = info {
            if let Some(default) = &info.default {
                body = body.push(field(
                    "Default",
                    text(default).size(theme::SMALL).style(theme::muted),
                ));
            }
            if !info.values.is_empty() {
                body = body.push(field(
                    "Values",
                    text(&info.values).size(theme::SMALL).style(theme::muted),
                ));
            }
            if !info.doc.is_empty() {
                body = body.push(text(&info.doc).size(theme::SMALL));
            }
            if info.guarded {
                body = body.push(
                    text("Guarded: the change reverts on its own unless it is confirmed from the status bar.")
                        .size(theme::SMALL)
                        .style(iced::widget::text::warning),
                );
            }
        }
        let problem = edit.problem();
        body = body.push(dialog::error(edit.error.clone().or(problem.clone())));
        if edit.busy {
            body = body.push(text("applying ...").size(theme::SMALL).style(theme::muted));
        }

        let ready = !edit.busy && problem.is_none() && self.link == Link::Online;
        let set = self
            .setting(&edit.key)
            .is_some_and(|setting| setting.source == Source::Set);
        let mut buttons = vec![
            theme::default_button("OK", ready.then_some(Message::EditOk)),
            theme::dialog_button("Apply", ready.then_some(Message::EditApply)),
        ];
        if !edit.adding {
            buttons.push(theme::dialog_button(
                "Default",
                (ready && set).then_some(Message::EditDefault),
            ));
        }
        buttons.push(theme::dialog_button(
            "Cancel",
            (!edit.busy).then_some(Message::Cancel),
        ));
        let title = if edit.adding {
            "New data value".to_string()
        } else {
            format!("Edit {}", edit.key)
        };
        dialog::frame(title, body.into(), buttons)
    }
}

fn title_case(section: &str) -> String {
    let mut chars = section.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setting(key: &str, value: &str, source: Source) -> Setting {
        Setting {
            key: key.to_string(),
            env: String::new(),
            value: Some(value.to_string()),
            source,
        }
    }

    fn settings() -> Settings {
        Settings {
            revision: 3,
            settings: vec![
                setting("browser.url", "https://a.test/", Source::Set),
                setting("screen.scale", "auto", Source::Default),
                setting("browser.touch", "auto", Source::Default),
                setting("data.table", "12", Source::Set),
            ],
        }
    }

    #[test]
    fn sections_follow_the_order_the_device_lists_them_in() {
        assert_eq!(sections(&settings()), ["browser", "screen", "data"]);
    }

    #[test]
    fn a_section_holds_its_keys_without_the_prefix() {
        let rows = rows(&settings(), &BTreeMap::new(), "browser");
        let short: Vec<&str> = rows.iter().map(|row| row.short.as_str()).collect();
        assert_eq!(short, ["url", "touch"]);
        assert_eq!(rows[0].source, Source::Set);
    }

    #[test]
    fn a_data_key_is_described_by_the_template() {
        let template = KeyInfo {
            name: "data.<name>".to_string(),
            env: String::new(),
            applies: vec![Consumer::Agent],
            guarded: false,
            doc: "custom".to_string(),
            values: String::new(),
            default: None,
            value: None,
        };
        let keys = BTreeMap::from([(template.name.clone(), template)]);
        let rows = rows(&settings(), &keys, "data");
        assert_eq!(rows[0].applies, "agent");
    }

    #[test]
    fn values_are_checked_as_the_agent_checks_them() {
        assert!(check("browser.url", "https://a.test/").is_ok());
        assert!(check("browser.url", "not a url").is_err());
        assert!(check("browser.url", "https://a.test/$x").is_err());
    }

    #[test]
    fn a_new_data_key_needs_a_placeholder_name() {
        let mut edit = Edit {
            key: String::new(),
            adding: true,
            name: "Table".to_string(),
            value: "12".to_string(),
            error: None,
            busy: false,
            close_after: false,
        };
        assert!(edit.problem().is_some());
        edit.name = "table".to_string();
        assert_eq!(edit.key(), "data.table");
        assert!(edit.problem().is_none());
    }

    #[test]
    fn the_input_follows_the_kind() {
        assert_eq!(input("browser.url"), Input::Text);
        assert!(matches!(input("browser.touch"), Input::Choice(_)));
    }
}
