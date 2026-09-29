//! A device's window: one nav entry per subject, each showing its settings
//! beside its tools, all drawn the same way (`section.rs`), with the
//! device's state along the bottom and a log of what the changes did above
//! it.
//!
//! A page shows the keys of its prefix (`Page::scope`). A prefix the device
//! reports that no page shows gets an entry of its own, in the order the
//! device reports it, so a group a newer image adds shows up without a
//! change here. A row is edited in a dialog, the
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
use tessaro_client::text::{Line, Tone};

use crate::dialog::{self, field};
use crate::grid::{bold, cell, col, grid, Col};
use crate::messages::Messages;
use crate::{logs, vnc};

mod pages;
use crate::section::{self, action};
use crate::theme;
use crate::worker::{Event, Request};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    Connecting,
    Online,
    Lost(String),
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

/// The sections no page shows beside its tools, in the device's order. Data
/// is always among them, so the first custom value can be added.
pub fn own_sections(settings: &Settings) -> Vec<String> {
    let claimed: Vec<Scope> = std::iter::once(Page::Overview)
        .chain(Page::TOOLS.iter().map(|(page, _)| *page))
        .filter_map(Page::scope)
        .collect();
    let mut own: Vec<String> = sections(settings)
        .into_iter()
        .filter(|section| !claimed.iter().any(|scope| &scope.prefix == section))
        .collect();
    if !own.iter().any(|section| section == data_section()) {
        own.push(data_section().to_string());
    }
    own
}

/// The section of the custom `data.*` values.
fn data_section() -> &'static str {
    keys::DATA_PREFIX.trim_end_matches('.')
}

fn group(key: &str) -> &str {
    key.split_once('.').map_or(key, |(group, _)| group)
}

/// The keys a nav entry shows: those under `prefix`, less those under
/// `except`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub prefix: String,
    pub except: Option<&'static str>,
}

impl Scope {
    fn of(prefix: &str) -> Self {
        Self {
            prefix: prefix.to_string(),
            except: None,
        }
    }

    fn holds(&self, key: &str) -> bool {
        let under = |prefix: &str| {
            key.strip_prefix(prefix)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
        };
        under(&self.prefix) && !self.except.is_some_and(under)
    }

    fn is_data(&self) -> bool {
        self.prefix == data_section()
    }
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
    scope: &Scope,
) -> Vec<SettingRow> {
    settings
        .settings
        .iter()
        .filter(|setting| scope.holds(&setting.key))
        .map(|setting| {
            let info = info(keys, &setting.key);
            SettingRow {
                key: setting.key.clone(),
                short: setting
                    .key
                    .strip_prefix(scope.prefix.as_str())
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
        .map(|consumer| tessaro_client::describe::device::restarts_short(*consumer))
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
    /// The prefix of the settings window it was opened from, and is drawn
    /// in.
    window: String,
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
    /// Shown once: a new token, a password. Each value with its label, an
    /// empty one for a lone value.
    Secret {
        title: String,
        intro: String,
        values: Vec<(String, String)>,
    },
    Text {
        title: String,
        body: String,
    },
}

#[derive(Debug, Clone)]
pub enum Message {
    Table(String, crate::grid::Event),
    /// Open the settings window of a group, or raise it (`main.rs`).
    Configure(Scope),
    /// The settings window with this prefix was closed.
    Unconfigure(String),
    /// Something in the settings window with this prefix.
    Cfg(String, Cfg),
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
    CopyLog,
    /// A click, selection or scroll in Messages.
    LogAction(iced::widget::text_editor::Action),
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
    /// Something on one of the pages.
    P(pages::Msg),
}

/// What a settings window does with its table.
#[derive(Debug, Clone)]
pub enum Cfg {
    Select(String),
    Activate(String),
    Filter(String),
    Edit,
    Add,
    Reset,
    Copy,
    /// Up (-1) or Down (1) in the table.
    Step(i32),
    Enter,
}

/// An open settings window: the keys it shows, and its own selection and
/// filter.
struct Config {
    scope: Scope,
    selected: Option<String>,
    filter: String,
}

/// What the right side of a device window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Overview,
    Screen,
    Browser,
    Policies,
    Network,
    Wifi,
    Certs,
    Storage,
    Audio,
    Time,
    Schedules,
    Printer,
    Access,
    Ssh,
    Files,
    Update,
    Log,
}

impl Page {
    /// The pages after Overview in the nav, in its order.
    const TOOLS: &'static [(Page, &'static str)] = &[
        (Page::Screen, "Screen"),
        (Page::Browser, "Browser"),
        (Page::Policies, "Policies"),
        (Page::Network, "Network"),
        (Page::Wifi, "WiFi"),
        (Page::Certs, "Certificates"),
        (Page::Storage, "Storage"),
        (Page::Audio, "Audio"),
        (Page::Time, "Time"),
        (Page::Schedules, "Schedules"),
        (Page::Printer, "Printer"),
        (Page::Access, "Access"),
        (Page::Ssh, "SSH"),
        (Page::Files, "Files"),
        (Page::Update, "Update"),
        (Page::Log, "Log"),
    ];

    /// The settings its Configure opens. The WiFi keys are on WiFi, not on
    /// Network.
    fn scope(self) -> Option<Scope> {
        let (prefix, except) = match self {
            Page::Overview => ("device", None),
            Page::Screen => ("screen", None),
            Page::Browser => ("browser", None),
            Page::Network => ("network", Some("network.wifi")),
            Page::Wifi => ("network.wifi", None),
            Page::Storage => ("storage", None),
            Page::Audio => ("audio", None),
            Page::Time => ("time", None),
            Page::Printer => ("printer", None),
            Page::Access => ("access", None),
            Page::Policies
            | Page::Certs
            | Page::Schedules
            | Page::Ssh
            | Page::Files
            | Page::Update
            | Page::Log => return None,
        };
        Some(Scope {
            prefix: prefix.to_string(),
            except,
        })
    }
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
    pub(super) tables: crate::grid::Tables,
    pub node: Node,
    pub link: Link,
    requests: Option<mpsc::Sender<Request>>,
    info: Option<NodeInfo>,
    status: Option<(Status, Instant)>,
    keys: BTreeMap<String, KeyInfo>,
    settings: Option<Settings>,
    /// The open settings windows, by their prefix.
    configs: BTreeMap<String, Config>,
    log: Messages,
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
    /// The picture shown, with its hold on the GPU texture. A frame is
    /// swapped in only once it is uploaded: iced uploads a full screen's
    /// worth off-thread and draws nothing until it is done, so showing a
    /// handle straight away blanks the panel on every frame.
    frame: Option<(image::Handle, Option<image::Allocation>)>,
    /// The newest frame, held while another one uploads.
    next: Option<image::Handle>,
    /// The upload under way. A result with any other number is from before
    /// the panel was closed, and is dropped.
    uploading: Option<u64>,
    uploads: u64,
    state: Option<Result<String, String>>,
}

/// A VNC frame iced has finished uploading, for the window it was meant for.
#[derive(Debug, Clone)]
pub struct VncUploaded {
    upload: u64,
    image: image::Handle,
    allocation: Result<image::Allocation, image::Error>,
}

impl Device {
    /// A window for `node`, with the message log shown or not as the last
    /// window left it.
    pub fn new(node: Node, log_open: bool) -> Self {
        Self {
            tables: crate::grid::Tables::default(),
            node,
            link: Link::Connecting,
            requests: None,
            info: None,
            status: None,
            keys: BTreeMap::new(),
            settings: None,
            configs: BTreeMap::new(),
            log: Messages::default(),
            log_open,
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
    pub fn vnc_event(&mut self, event: vnc::Event) -> Task<VncUploaded> {
        match event {
            vnc::Event::State(state) => self.vnc.state = Some(Ok(state)),
            vnc::Event::Frame(image) if self.vnc.open => {
                if self.vnc.uploading.is_none() {
                    return self.vnc_upload(image);
                }
                self.vnc.next = Some(image);
            }
            vnc::Event::Frame(_) => {}
            vnc::Event::Lost(why) => self.vnc.state = Some(Err(why)),
        }
        Task::none()
    }

    /// A frame is uploaded: show it, and start on the newest one waiting.
    pub fn vnc_uploaded(&mut self, uploaded: VncUploaded) -> Task<VncUploaded> {
        if self.vnc.uploading != Some(uploaded.upload) {
            return Task::none();
        }
        self.vnc.uploading = None;
        // Without an allocation the picture may blink, which beats none.
        self.vnc.frame = Some((uploaded.image, uploaded.allocation.ok()));
        match self.vnc.next.take() {
            Some(image) => self.vnc_upload(image),
            None => Task::none(),
        }
    }

    fn vnc_upload(&mut self, image: image::Handle) -> Task<VncUploaded> {
        self.vnc.uploads += 1;
        let upload = self.vnc.uploads;
        self.vnc.uploading = Some(upload);
        image::allocate(image.clone()).map(move |allocation| VncUploaded {
            upload,
            image: image.clone(),
            allocation,
        })
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
            (Some((frame, _)), false) => iced::widget::image(frame.clone())
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

    /// The device's name as it calls itself, else as the known nodes have it.
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

    /// Whether the message log is shown (Messages).
    pub fn log_open(&self) -> bool {
        self.log_open
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

    /// The Log page, live, on the units `unit` names (a pattern is fine).
    fn journal_of(&mut self, unit: String) {
        self.journal.unit = unit.clone();
        self.journal.streaming_unit = Some(unit);
        self.journal.generation += 1;
        self.journal.entries.clear();
        self.journal.state = None;
        self.journal.live = true;
        self.show(Page::Log);
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

    /// The rows of a settings window, as its table shows them.
    fn visible_rows(&self, config: &Config) -> Vec<SettingRow> {
        let Some(settings) = &self.settings else {
            return Vec::new();
        };
        rows(settings, &self.keys, &config.scope)
            .into_iter()
            .filter(|row| section::matches(&config.filter, &[&row.short, &row.value, &row.default]))
            .collect()
    }

    fn log(&mut self, tone: Tone, text: impl Into<String>) {
        self.log_line(Line::of(tone, text));
    }

    /// A line of the shared text, in its own tones.
    fn log_line(&mut self, line: Line) {
        self.log.push(line);
    }

    /// Show Messages, for a command whose output goes there. Only the
    /// Messages toggle changes what new windows start with (`main.rs`).
    fn open_log(&mut self) {
        self.log_open = true;
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

    /// Whether the device has any key in `scope`.
    fn has_keys(&self, scope: &Scope) -> bool {
        self.settings.as_ref().is_some_and(|settings| {
            settings
                .settings
                .iter()
                .any(|setting| scope.holds(&setting.key))
        })
    }

    /// The page's Configure: its settings window, when it has keys.
    fn configure(&self) -> Option<Message> {
        self.page
            .scope()
            .filter(|scope| self.has_keys(scope))
            .map(Message::Configure)
    }

    /// A settings window's title: the device, and the page its keys belong
    /// to.
    pub fn config_title(&self, prefix: &str) -> String {
        let label = std::iter::once((Page::Overview, "Device"))
            .chain(Page::TOOLS.iter().copied())
            .find(|(page, _)| page.scope().is_some_and(|scope| scope.prefix == prefix))
            .map_or_else(|| title_case(prefix), |(_, label)| label.to_string());
        format!("{} - {label} settings", self.name())
    }

    fn selected_in(&self, prefix: &str) -> Option<String> {
        self.configs.get(prefix)?.selected.clone()
    }

    fn setting(&self, key: &str) -> Option<&Setting> {
        self.settings
            .as_ref()?
            .settings
            .iter()
            .find(|setting| setting.key == key)
    }

    /// `browser.zoom` as set, else the image's 100.
    fn zoom(&self) -> String {
        self.setting(keys::ZOOM)
            .and_then(|setting| setting.value.clone())
            .unwrap_or_else(|| "100".to_string())
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
                        Tone::Plain,
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
            Event::Pinned(note) => self.log(Tone::Ok, note),
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
    /// What a change did, in the words `tessaro-ctl config set` uses: the
    /// network change first, when there was one.
    fn log_applied(&mut self, applied: &Applied) {
        let network = applied
            .network
            .as_ref()
            .map(tessaro_client::describe::net::change)
            .unwrap_or_default();
        let lines = tessaro_client::describe::device::applied(applied, false);
        for line in network.into_iter().chain(lines) {
            if !line.is_empty() {
                self.log_line(line);
            }
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Table(name, event) => return self.tables.update(&name, event),
            Message::Configure(scope) => {
                self.configs.entry(scope.prefix.clone()).or_insert(Config {
                    scope,
                    selected: None,
                    filter: String::new(),
                });
            }
            Message::Unconfigure(prefix) => {
                self.configs.remove(&prefix);
                // Its dialog goes with it; an answer still on its way is
                // logged all the same.
                if matches!(&self.dialog, Some(Dialog::Edit(edit)) if edit.window == prefix) {
                    self.dialog = None;
                }
            }
            Message::Cfg(prefix, cfg) => return self.config_update(prefix, cfg),
            Message::Page(page) => self.show(page),
            Message::Step(by) => {
                if self.dialog.is_none() {
                    self.primary_step(by);
                }
            }
            Message::Enter => {
                let next = match &self.dialog {
                    Some(Dialog::Edit(_)) => Some(Message::EditOk),
                    Some(Dialog::Confirm(_)) => Some(Message::Accept),
                    Some(Dialog::Form(_)) => Some(Message::P(pages::Msg::FormOk)),
                    Some(Dialog::Secret { .. } | Dialog::Text { .. }) => Some(Message::Cancel),
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
                    self.vnc.next = None;
                    self.vnc.uploading = None;
                    self.vnc.state = None;
                }
            }
            Message::VncReconnect => {
                self.vnc.generation += 1;
                self.vnc.state = None;
            }
            Message::P(message) => return self.page_update(message),
            Message::Refresh => {
                self.request(Request::Refresh);
                self.refresh_page(self.page);
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
            Message::CopyLog => return iced::clipboard::write(self.log.text()),
            Message::LogAction(action) => self.log.perform(action),
        }
        Task::none()
    }

    fn config_update(&mut self, prefix: String, cfg: Cfg) -> Task<Message> {
        let Some(config) = self.configs.get_mut(&prefix) else {
            return Task::none();
        };
        match cfg {
            Cfg::Select(key) => config.selected = Some(key),
            Cfg::Activate(key) => {
                config.selected = Some(key);
                self.open_edit(&prefix);
            }
            Cfg::Filter(filter) => config.filter = filter,
            Cfg::Edit => self.open_edit(&prefix),
            Cfg::Add => {
                self.dialog = Some(Dialog::Edit(Edit {
                    window: prefix,
                    key: String::new(),
                    adding: true,
                    name: String::new(),
                    value: String::new(),
                    error: None,
                    busy: false,
                    close_after: false,
                }));
            }
            Cfg::Reset => {
                if let Some(key) = config.selected.clone() {
                    let if_revision = self.revision();
                    self.request(Request::Unset {
                        keys: vec![key],
                        if_revision,
                    });
                }
            }
            Cfg::Copy => {
                let selected = config.selected.clone();
                if let Some(setting) = selected.as_deref().and_then(|key| self.setting(key)) {
                    return iced::clipboard::write(setting.value.clone().unwrap_or_default());
                }
            }
            Cfg::Step(by) => {
                if self.dialog.is_some() {
                    return Task::none();
                }
                let Some(config) = self.configs.get(&prefix) else {
                    return Task::none();
                };
                let keys: Vec<String> = self
                    .visible_rows(config)
                    .into_iter()
                    .map(|row| row.key)
                    .collect();
                let keys = self.tables.ordered(&format!("config:{prefix}"), &keys);
                let next = section::step(&keys, config.selected.as_ref(), by);
                if let Some(config) = self.configs.get_mut(&prefix) {
                    config.selected = next;
                }
            }
            Cfg::Enter => {
                if self.dialog.is_some() {
                    return self.update(Message::Enter);
                }
                if let Some(key) = self.selected_in(&prefix) {
                    return self.config_update(prefix, Cfg::Activate(key));
                }
            }
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

    /// The edit dialog for the row selected in the settings window of
    /// `prefix`, unless it is read-only.
    fn open_edit(&mut self, prefix: &str) {
        let selected = self.selected_in(prefix);
        let Some(setting) = selected.as_deref().and_then(|key| self.setting(key)) else {
            return;
        };
        if setting.source == Source::Live {
            return;
        }
        // A default shows as the value it is, so editing starts from it.
        let value = setting.value.clone().unwrap_or_default();
        self.dialog = Some(Dialog::Edit(Edit {
            window: prefix.to_string(),
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
                Tone::Plain,
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
                let mut body = row![
                    self.nav(settings),
                    container(self.tools_view())
                        .padding(6)
                        .width(Length::FillPortion(3))
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

        let mut page = column![body];
        if self.log_open {
            page = page.push(self.log_view());
        }
        let page = page.push(self.status_bar());

        // An edit is drawn in the settings window it came from.
        match &self.dialog {
            None | Some(Dialog::Edit(_)) => page.into(),
            Some(dialog) => dialog::modal(page.into(), self.dialog_view(dialog)),
        }
    }

    /// The settings window with `prefix`: its table, and the edit opened
    /// from it.
    pub fn config_view(&self, prefix: &str) -> Element<'_, Message> {
        let (Some(settings), Some(config)) = (&self.settings, self.configs.get(prefix)) else {
            return container(text("connecting ...").size(theme::SMALL))
                .padding(12)
                .into();
        };
        let table = container(self.section_view(settings, config)).padding(6);
        match &self.dialog {
            Some(dialog @ Dialog::Edit(edit)) if edit.window == prefix => {
                dialog::modal(table.into(), self.dialog_view(dialog))
            }
            _ => table.into(),
        }
    }

    /// The device window's actions, in its title bar next to its name.
    pub fn title_tools(&self) -> Element<'_, Message> {
        let online = self.link == Link::Online;
        let when = |message: Message| online.then_some(message);
        row![
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
        .height(20)
        .align_y(iced::alignment::Vertical::Center)
        .into()
    }

    /// What the page shown draws: its tools.
    fn tools_view(&self) -> Element<'_, Message> {
        match self.page {
            Page::Screen => column![self.modes_view(), self.screenshot_view()]
                .spacing(8)
                .into(),
            Page::Log => self.journal_view(),
            _ => self.page_view(),
        }
    }

    fn nav(&self, settings: &Settings) -> Element<'_, Message> {
        let entry = |label: String, selected: bool, message: Message| -> Element<'_, Message> {
            button(text(label).size(theme::TEXT))
                .width(Length::Fill)
                .padding([4, 10])
                .style(theme::nav(selected))
                .on_press(message)
                .into()
        };
        let mut entries: Vec<Element<'_, Message>> = vec![entry(
            "Overview".to_string(),
            self.page == Page::Overview,
            Message::Page(Page::Overview),
        )];
        for (page, label) in Page::TOOLS {
            entries.push(entry(
                label.to_string(),
                self.page == *page,
                Message::Page(*page),
            ));
        }
        // The sections without a page open their settings window.
        entries.extend(own_sections(settings).into_iter().map(|section| {
            let configure = Message::Configure(Scope::of(&section));
            entry(title_case(&section), false, configure)
        }));
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
            let message: crate::grid::Cell<'_, Message> = match entry.priority {
                Some(0..=3) => message.style(text::danger).into(),
                Some(4) => message.style(text::warning).into(),
                Some(7) => message.style(theme::muted).into(),
                _ => message.into(),
            };
            vec![
                cell(entry.clock())
                    .sort_number(entry.time.unwrap_or_default() as f64)
                    .style(theme::muted)
                    .into(),
                cell(entry.source.clone()).into(),
                message,
            ]
        });
        let table = crate::grid::grid_following(
            self.tables.state("journal"),
            |event| Message::Table("journal".into(), event),
            COLUMNS,
            cells.collect(),
            journal.paused.is_none(),
        );

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

    /// The table of a settings window.
    fn section_view<'a>(
        &'a self,
        settings: &'a Settings,
        config: &'a Config,
    ) -> Element<'a, Message> {
        let scope = &config.scope;
        let all = rows(settings, &self.keys, scope);
        let empty = all.is_empty();
        let rows: Vec<SettingRow> = all
            .into_iter()
            .filter(|row| section::matches(&config.filter, &[&row.short, &row.value, &row.default]))
            .collect();
        let selected_at = config
            .selected
            .as_ref()
            .and_then(|key| rows.iter().position(|row| &row.key == key));
        let selected = selected_at.map(|at| &rows[at]);
        let prefix = scope.prefix.clone();
        let cfg = move |cfg: Cfg| Message::Cfg(prefix.clone(), cfg);

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
                    crate::grid::Cell::from(cell("!").style(text::warning)).map(|content| {
                        iced::widget::tooltip(
                            content,
                            text("guarded: reverts on its own unless confirmed").size(theme::SMALL),
                            iced::widget::tooltip::Position::Right,
                        )
                        .into()
                    })
                } else {
                    cell("").into()
                },
                crate::grid::Cell::from(cell(row.short.clone()))
                    .map(|content| self.with_doc(&row.key, content)),
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
        let (on_select, on_activate) = (cfg.clone(), cfg.clone());
        let table = if empty && scope.is_data() {
            container(
                text("no custom values yet")
                    .size(theme::SMALL)
                    .style(theme::muted),
            )
            .padding(6)
            .into()
        } else {
            grid(
                self.tables.state(&format!("config:{}", scope.prefix)),
                move |event| Message::Table(format!("config:{}", scope.prefix), event),
                COLUMNS,
                cells.collect(),
                selected_at,
                move |at| on_select(Cfg::Select(keys[at].clone())),
                move |at| on_activate(Cfg::Activate(keys_too[at].clone())),
            )
        };

        let online = self.link == Link::Online;
        let editable = selected.filter(|row| online && row.source != Source::Live);
        let mut list = Vec::new();
        if scope.is_data() {
            list.push(action("Add", online.then(|| cfg(Cfg::Add))));
        }
        let on_filter = cfg.clone();
        section::view(
            list,
            vec![
                action("Edit", editable.map(|_| cfg(Cfg::Edit))),
                action(
                    // Unsetting a custom value removes it.
                    if scope.is_data() {
                        "Delete"
                    } else {
                        "Reset to default"
                    },
                    editable
                        .filter(|row| row.source == Source::Set)
                        .map(|_| cfg(Cfg::Reset)),
                ),
                action("Copy value", selected.map(|_| cfg(Cfg::Copy))),
            ],
            &config.filter,
            move |filter| on_filter(Cfg::Filter(filter)),
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
        container(
            column![
                row![
                    text("Messages").size(theme::SMALL).font(bold()),
                    space::horizontal(),
                    theme::tool("Copy", Some(Message::CopyLog)),
                    theme::tool("Clear", Some(Message::ClearLog)),
                ]
                .spacing(4)
                .align_y(iced::alignment::Vertical::Center),
                self.log.view(Message::LogAction),
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
            if let Some(percent) = status.cpu_percent {
                bar = bar.push(small(format!("CPU {percent}%")).style(theme::muted));
            }
            if let Some(memory) = &status.memory {
                bar =
                    bar.push(small(format!("RAM {}%", memory.used_percent())).style(theme::muted));
            }
            if let Some(data) = &status.data {
                if let Some(percent) = (data.used * 100).checked_div(data.size) {
                    bar = bar.push(small(format!("/data {percent}% used")).style(theme::muted));
                }
            }
            bar = bar.push(space::horizontal());
            if let Some(pending) = &status.pending {
                let left = pending.seconds_left.saturating_sub(at.elapsed().as_secs());
                bar = bar.push(
                    small(format!(
                        "{}={} reverts to {} in {left}s",
                        pending.key,
                        pending.value,
                        pending.previous_or_default()
                    ))
                    .style(iced::widget::text::warning),
                );
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
                values,
            } => {
                let mut body =
                    column![text(intro).size(theme::SMALL).style(text::warning)].spacing(10);
                for (label, value) in values {
                    let line = row![
                        // A token is one long word: break it anywhere so
                        // it wraps inside the dialog and Copy stays in view.
                        text(value)
                            .size(theme::TEXT)
                            .font(iced::Font::MONOSPACE)
                            .wrapping(text::Wrapping::WordOrGlyph)
                            .width(Length::Fill),
                        theme::tool("Copy", Some(Message::P(pages::Msg::Copy(value.clone())))),
                    ]
                    .spacing(8)
                    .align_y(iced::alignment::Vertical::Center);
                    body = if label.is_empty() {
                        body.push(line)
                    } else {
                        body.push(field(label, line))
                    };
                }
                dialog::frame(
                    title.clone(),
                    body.into(),
                    vec![theme::default_button("Done", Some(Message::Cancel))],
                )
            }
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
                    text("Guarded: the change reverts on its own unless it is confirmed on the Screen page.")
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
        let rows = rows(&settings(), &BTreeMap::new(), &Scope::of("browser"));
        let short: Vec<&str> = rows.iter().map(|row| row.short.as_str()).collect();
        assert_eq!(short, ["url", "touch"]);
        assert_eq!(rows[0].source, Source::Set);
    }

    #[test]
    fn the_wifi_keys_are_on_wifi_not_on_network() {
        let settings = Settings {
            revision: 1,
            settings: vec![
                setting("network.ethernet.method", "auto", Source::Default),
                setting("network.wifi.ssid", "cafe", Source::Set),
                setting("network.wifiless", "x", Source::Set),
            ],
        };
        let short = |page: Page| -> Vec<String> {
            rows(&settings, &BTreeMap::new(), &page.scope().unwrap())
                .into_iter()
                .map(|row| row.short)
                .collect()
        };
        assert_eq!(short(Page::Network), ["ethernet.method", "wifiless"]);
        assert_eq!(short(Page::Wifi), ["ssid"]);
    }

    #[test]
    fn the_sections_no_page_shows_get_entries_with_data_always_among_them() {
        let mut settings = settings();
        settings
            .settings
            .push(setting("agent.watchdog", "on", Source::Default));
        assert_eq!(own_sections(&settings), ["data", "agent"]);
        settings
            .settings
            .retain(|setting| setting.key != "data.table");
        assert_eq!(own_sections(&settings), ["agent", "data"]);
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
            input: protocol::KeyInput::Text,
        };
        let keys = BTreeMap::from([(template.name.clone(), template)]);
        let rows = rows(&settings(), &keys, &Scope::of("data"));
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
            window: "data".to_string(),
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
