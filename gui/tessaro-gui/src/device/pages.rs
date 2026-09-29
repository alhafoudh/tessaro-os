//! The device window's pages beyond the settings: one for each group of
//! `tessaro-ctl` commands, so everything the command line can do to a
//! device, the window can too. Each is drawn like every other list
//! (`section.rs`): a toolbar, a table, actions on the list and on the
//! selected row.
//!
//! A page asks through the worker's generic call (`Request::Call`) and gets
//! the answer back by its tag (`answer`); settings go through `Request::Set`
//! like an edit, so they are validated and revision-checked the same way.
//! Long work - streams and transfers - runs as a job on a connection of its
//! own (`jobs.rs`). Dialogs are one generic form (`Form`), confirmed with
//! Enter; a destructive one wants the device's name typed, as
//! `tessaro-ctl` does.

use std::collections::BTreeMap;
use std::path::PathBuf;

use iced::widget::{
    checkbox, column, container, pick_list, progress_bar, row, rule, scrollable, slider, space,
    text, text_editor, text_input, Column,
};
use iced::{Element, Length, Task};
use protocol::api::{
    self, AudioTestBody, CalendarBody, CertBody, CertQuery, DeleteBody, EvalBody, FilesQuery,
    GrowBody, KeyboardBody, MoveBody, NameBody, NavigateBody, PasswordBody, PathBody, PingBody,
    PolicyBody, PolicyRef, ProfileQuery, ScheduleChange, ScheduleRef, ScreenPowerBody,
    SpeedtestBody, SshKeyQuery, TokenRef, WifiJoinBody, WifiScanQuery,
};
use protocol::files::{self as store, FileEntry, FileKind, FilesListing};
use protocol::keys;
use protocol::policy::{self, EffectiveEntry, PolicyDoc, PolicyInfo, PolicyRemoved, PolicySaved};
use protocol::{
    size_label, Applied, AudioDevice, AudioStatus, AudioTested, CalendarCheck, CertInfo,
    CertsAdded, Claimed, Connector, Done, HotspotCredentials, Net, NetChange, NetProfile,
    NetProfileDetail, OnError, Password, PingEvent, ProxyTested, ScheduleInfo, ScheduleSpec,
    SpeedtestEvent, SshKeyInfo, SshKeyRevoked, Storage, StorageGrowEvent, TimeStatus, TokenCreated,
    TokenInfo, UpdatePhase, UpdateStatus, Verify, WifiNetwork, WifiSecurity, WifiStatus,
};
use serde_json::Value;
use tessaro_client::connect::Answer;
use tessaro_client::describe;
use tessaro_client::text::{Fact, Line};
use tessaro_client::transfer::date;
use tessaro_client::webconfig;

use super::{Device, Dialog, Link, Message, Page, Tone};
use crate::dialog::{self, field};
use crate::grid::{bold, cell, col, grid, Cell, Col};
use crate::section::{self, action};
use crate::worker::{call, fetch, send, Call, Request};
use crate::{blocking, jobs, theme};

/// What the pages keep between answers.
#[derive(Default)]
pub struct State {
    net: Option<Net>,
    profiles: Vec<NetProfile>,
    /// The extra certificate authorities the device trusts.
    certs: Vec<CertInfo>,
    /// The stored browser policies.
    policies: Vec<PolicyInfo>,
    wifi: Option<WifiStatus>,
    networks: Vec<WifiNetwork>,
    storage: Option<Storage>,
    audio: Option<AudioStatus>,
    /// A volume slider being dragged: shown, not yet sent.
    volume: Option<u8>,
    input_volume: Option<u8>,
    time: Option<TimeStatus>,
    schedules: Vec<ScheduleInfo>,
    /// Counts edits of a schedule form's calendar, so only the check of the
    /// last one is sent.
    calendar_edits: u64,
    tokens: Vec<TokenInfo>,
    ssh_keys: Vec<SshKeyInfo>,
    files_dir: String,
    files: Vec<FileEntry>,
    update: Option<UpdateStatus>,
    modes: Vec<Connector>,
    /// The selected row of each table, by table.
    selected: BTreeMap<&'static str, String>,
    /// Output of the page's streams (ping, speed test, grow), by page.
    output: BTreeMap<&'static str, Vec<Line>>,
    /// The last error a page's refresh got, by page.
    errors: BTreeMap<&'static str, String>,
    /// Calls sent and not answered yet, by tag, so a page can say what it
    /// is waiting for.
    in_flight: BTreeMap<&'static str, u32>,
}

/// A job, while it runs and after.
pub struct Job {
    pub id: u64,
    owner: &'static str,
    label: String,
    pub kind: jobs::Kind,
    progress: Option<(Line, u64, u64)>,
    pub running: bool,
}

/// A dialog with fields, answered with Enter.
pub struct Form {
    title: String,
    intro: Option<String>,
    fields: Vec<Field>,
    ok: &'static str,
    action: Action,
    error: Option<String>,
    /// What the device says about the fields so far, under them: a
    /// schedule's next run times.
    note: Option<String>,
    /// Destructive: the device's name must be typed first.
    typed: bool,
    /// Wide enough for a document.
    wide: bool,
}

struct Field {
    label: &'static str,
    value: String,
    kind: FieldKind,
    /// A multi-line field's text as the editor holds it; `value` follows it.
    editor: Option<text_editor::Content>,
    /// Typed in `Font::MONOSPACE`: code, where the characters matter.
    mono: bool,
    /// A multi-line field tall enough for a document.
    tall: bool,
}

#[derive(Clone, Copy)]
enum FieldKind {
    Text(&'static str),
    /// One item per line.
    Multiline(&'static str),
    Secret,
    Check,
    Choice(&'static [&'static str]),
}

#[derive(Debug, Clone)]
enum Action {
    Navigate,
    Maintenance,
    DebugScreen,
    Zoom,
    Inject,
    Bridge,
    Eval,
    ControlPing,
    FactoryReset,
    NetPing,
    Speedtest,
    Proxy,
    CertRevoke(String),
    /// A new browser policy (`name` None) or a change to one, saved only
    /// while the device's copy is still at `revision` (`""`: none yet).
    PolicySave {
        name: Option<String>,
        revision: String,
    },
    PolicyRemove(String),
    /// A new schedule, or a change to the one with this id.
    ScheduleSave(Option<String>),
    ScheduleRemove(String),
    WifiJoin,
    Hotspot,
    Grow,
    TokenCreate,
    TokenRevoke(String),
    Password,
    Claim,
    Unclaim,
    KeyRevoke(String),
    Mkdir,
    Move(String),
    Delete(Vec<String>),
    UpdateSend(PathBuf),
    UpdateCancel,
    Timezone,
    Ntp,
    SetClock,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Select(&'static str, String),
    Activate(&'static str, String),
    // device and browser
    Navigate,
    Maintenance(bool),
    DebugScreen(bool),
    Zoom,
    DevTools,
    Reload,
    ClearCache,
    Inject,
    Bridge,
    Eval,
    ControlPing,
    FactoryReset,
    // browser policies
    PolicyNew,
    PolicyPick,
    PolicyPicked(Option<PathBuf>),
    PolicyEdit,
    PolicyEffective,
    PolicyRemove,
    // screen
    UseMode,
    ScreenPower(bool),
    Keyboard(bool),
    // network
    NetLast,
    NetPing,
    Speedtest,
    Proxy,
    ProxyOff,
    ProxyTest,
    ProfileDetail,
    CertPick,
    CertPicked(Option<PathBuf>),
    CertRevoke,
    // wifi
    WifiScan,
    WifiJoin,
    Hotspot,
    // storage
    GrowCheck,
    Grow,
    // audio
    UseAudio(&'static str),
    Volume(u8),
    VolumeDone,
    InputVolume(u8),
    InputVolumeDone,
    Mute(bool),
    InputMute(bool),
    Test(bool),
    // time
    Timezone,
    Ntp,
    TimeSync,
    SetClock,
    // schedules
    ScheduleNew,
    ScheduleEdit,
    ScheduleToggle,
    ScheduleRun,
    ScheduleLogs,
    ScheduleRemove,
    /// The calendar field has not changed for a moment since this edit.
    ScheduleCheckDue(u64),
    // access
    TokenNew,
    TokenRevoke,
    Password,
    Claim,
    Unclaim,
    Webconfig,
    // ssh
    KeyRevoke,
    Authorize(bool),
    /// Whether to open a terminal; the command, and whether a key was sent
    /// (not for an unclaimed device).
    /// Whether to open a terminal, and the ssh command with what
    /// authorizing did.
    Authorized(bool, Result<(String, Vec<Line>), String>),
    // files
    FilesUp,
    Upload(bool),
    Uploads(Option<Vec<PathBuf>>),
    Download,
    DownloadTo(Option<PathBuf>),
    Mkdir,
    Move,
    Delete,
    // update
    UpdatePick,
    UpdatePicked(Option<PathBuf>),
    UpdateCancel,
    // jobs and forms
    CancelJob(u64),
    ClearOutput,
    FormText(usize, String),
    FormEdit(usize, text_editor::Action),
    FormCheck(usize, bool),
    FormChoice(usize, &'static str),
    FormOk,
    Copy(String),
}

impl Field {
    fn text(label: &'static str, value: impl Into<String>, placeholder: &'static str) -> Self {
        Self {
            label,
            value: value.into(),
            kind: FieldKind::Text(placeholder),
            editor: None,
            mono: false,
            tall: false,
        }
    }

    /// Several lines, each one item.
    fn multiline(label: &'static str, lines: &[String], placeholder: &'static str) -> Self {
        let value = lines.join("\n");
        Self {
            label,
            editor: Some(text_editor::Content::with_text(&value)),
            value,
            kind: FieldKind::Multiline(placeholder),
            mono: false,
            tall: false,
        }
    }

    fn secret(label: &'static str) -> Self {
        Self {
            label,
            value: String::new(),
            kind: FieldKind::Secret,
            editor: None,
            mono: false,
            tall: false,
        }
    }

    fn check(label: &'static str, on: bool) -> Self {
        Self {
            label,
            value: flag(on).to_string(),
            kind: FieldKind::Check,
            editor: None,
            mono: false,
            tall: false,
        }
    }

    fn choice(label: &'static str, value: &'static str, choices: &'static [&'static str]) -> Self {
        Self {
            label,
            value: value.to_string(),
            kind: FieldKind::Choice(choices),
            editor: None,
            mono: false,
            tall: false,
        }
    }

    /// Typed in the monospace font.
    fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    /// A multi-line field with room for a document.
    fn tall(mut self) -> Self {
        self.tall = true;
        self
    }

    fn on(&self) -> bool {
        self.value == "1"
    }
}

fn flag(on: bool) -> &'static str {
    if on {
        "1"
    } else {
        "0"
    }
}

impl Form {
    fn new(title: impl Into<String>, ok: &'static str, action: Action) -> Self {
        Self {
            title: title.into(),
            intro: None,
            fields: Vec::new(),
            ok,
            action,
            error: None,
            note: None,
            typed: false,
            wide: false,
        }
    }

    fn intro(mut self, intro: impl Into<String>) -> Self {
        self.intro = Some(intro.into());
        self
    }

    fn wide(mut self) -> Self {
        self.wide = true;
        self
    }

    fn field(mut self, field: Field) -> Self {
        self.fields.push(field);
        self
    }

    /// Destructive: the device's name goes in a last field.
    fn typed(mut self) -> Self {
        self.typed = true;
        self.fields
            .push(Field::text("Device name", "", "type it to confirm"));
        self
    }

    fn value(&self, label: &str) -> &str {
        self.fields
            .iter()
            .find(|field| field.label == label)
            .map_or("", |field| field.value.as_str())
    }

    fn checked(&self, label: &str) -> bool {
        self.fields
            .iter()
            .find(|field| field.label == label)
            .is_some_and(Field::on)
    }

    /// A multi-line field's non-blank lines, trimmed.
    fn lines(&self, label: &str) -> Vec<String> {
        self.value(label)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect()
    }
}

const ON_ERROR: &[&str] = OnError::NAMES;

/// The Join form's security choice that leaves it to the last scan, as
/// `tessaro-ctl network wifi join` without `--security` does.
const AUTO: &str = "auto";

/// `AUTO`, then every security the device takes.
fn wifi_securities() -> &'static [&'static str] {
    const CHOICES: [&str; WifiSecurity::NAMES.len() + 1] = {
        let mut choices = [AUTO; WifiSecurity::NAMES.len() + 1];
        let mut at = 0;
        while at < WifiSecurity::NAMES.len() {
            choices[at + 1] = WifiSecurity::NAMES[at];
            at += 1;
        }
        choices
    };
    &CHOICES
}

/// The schedule dialog: new, or `existing` to change.
fn schedule_form(existing: Option<&ScheduleInfo>) -> Form {
    let spec = existing
        .map(|info| info.spec.clone())
        .unwrap_or(ScheduleSpec {
            name: String::new(),
            enabled: true,
            calendar: Vec::new(),
            lines: Vec::new(),
            on_error: OnError::Stop,
            timeout_s: None,
        });
    let title = existing.map_or_else(
        || "New schedule".to_string(),
        |info| format!("Schedule {}", info.spec.name),
    );
    let on_error = ON_ERROR
        .iter()
        .find(|name| **name == spec.on_error.name())
        .copied()
        .unwrap_or("stop");
    let timeout = spec
        .timeout_s
        .map(tessaro_client::schedule::format_timeout)
        .unwrap_or_default();
    Form::new(
        title,
        "Save",
        Action::ScheduleSave(existing.map(|info| info.id.clone())),
    )
    .intro(
        "Each command line runs with /bin/sh -c as root, in order; write tessaro-ctl commands out in full. \
         Calendar lines are systemd OnCalendar expressions in the device's timezone; any of them fires.",
    )
    .field(Field::text("Name", spec.name, "lower-case letters, digits and -"))
    .field(Field::multiline(
        "Calendar",
        &spec.calendar,
        "one per line: Mon..Fri 07:00, Sat,Sun *:0/15, daily",
    ).mono())
    .field(Field::multiline(
        "Commands",
        &spec.lines,
        "one per line: tessaro-ctl screen power off",
    ).mono())
    .field(Field::choice("On error", on_error, ON_ERROR))
    .field(Field::text("Timeout", timeout, "none, or 90s, 10m, 2h"))
    .field(Field::check("Enabled", spec.enabled))
}

/// The tz database's names, as the first device asked listed them. Kept
/// for the life of the program, since a choice field holds `&'static str`s;
/// it is one list, fetched once.
static ZONES: std::sync::OnceLock<&'static [&'static str]> = std::sync::OnceLock::new();

/// `ZONES`, filled from a `TimeZones` answer the first time.
fn zones(list: Vec<String>) -> &'static [&'static str] {
    ZONES.get_or_init(|| {
        let names: Vec<&'static str> = list
            .into_iter()
            .map(|zone| &*Box::leak(zone.into_boxed_str()))
            .collect();
        Box::leak(names.into_boxed_slice())
    })
}

/// Under a schedule form's calendar: how systemd reads it and when it
/// fires, as `tessaro-ctl schedule check` prints it.
fn calendar_note(check: &CalendarCheck) -> String {
    let mut lines: Vec<String> = check
        .normalized
        .iter()
        .map(|form| format!("reads as: {form}"))
        .collect();
    for fact in tessaro_client::schedule::upcoming(check, "fires") {
        let label = if fact.label.is_empty() {
            String::new()
        } else {
            format!("{}: ", fact.label)
        };
        lines.push(format!("{label}{}", fact.value));
    }
    lines.join("\n")
}

/// The page a tag's answer belongs to, for its error line.
fn page_of(tag: &str) -> &'static str {
    match tag.split('.').next().unwrap_or("") {
        "net" => "net",
        "wifi" => "wifi",
        "certs" | "cert" => "certs",
        "policies" | "policy" => "policies",
        "storage" => "storage",
        "audio" => "audio",
        "time" => "time",
        "schedules" | "schedule" => "schedules",
        "tokens" | "token" | "password" | "claim" | "unclaim" => "access",
        "ssh" => "ssh",
        "files" => "files",
        "update" => "update",
        "modes" | "screen" => "screen",
        "browser" => "browser",
        _ => "overview",
    }
}

fn parse<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|err| format!("unexpected answer: {err}"))
}

/// `command` in a new terminal window of the platform's own.
fn open_terminal(command: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let result = {
        let script = format!(
            "tell application \"Terminal\" to do script \"{}\"",
            command.replace('\\', "\\\\").replace('"', "\\\"")
        );
        std::process::Command::new("osascript")
            .args([
                "-e",
                &script,
                "-e",
                "tell application \"Terminal\" to activate",
            ])
            .spawn()
    };
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd")
        .args(["/c", "start", "cmd", "/k", command])
        .spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let result = std::process::Command::new("x-terminal-emulator")
        .args(["-e", "sh", "-c", command])
        .spawn()
        .or_else(|_| {
            std::process::Command::new("gnome-terminal")
                .args(["--", "sh", "-c", command])
                .spawn()
        });
    result
        .map(|_| ())
        .map_err(|err| format!("no terminal to open: {err}"))
}

const TABLE_HEIGHT: f32 = 170.0;

impl Device {
    fn call(&mut self, tag: &'static str, call: Call) {
        self.send_call(tag, call, false);
    }

    fn call_long(&mut self, tag: &'static str, call: Call) {
        self.send_call(tag, call, true);
    }

    fn send_call(&mut self, tag: &'static str, call: Call, long: bool) {
        if self.request(Request::Call { tag, call, long }) {
            *self.pages.in_flight.entry(tag).or_default() += 1;
        }
    }

    /// A call with this tag is on its way.
    fn waiting(&self, tag: &str) -> bool {
        self.pages
            .in_flight
            .get(tag)
            .is_some_and(|count| *count > 0)
    }

    /// The connection went: nothing sent on it will be answered.
    pub(super) fn forget_in_flight(&mut self) {
        self.pages.in_flight.clear();
    }

    fn set(&mut self, values: &[(&str, &str)]) {
        let values = values
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        let if_revision = self.revision();
        self.request(Request::Set {
            values,
            if_revision,
        });
    }

    fn start_job(&mut self, owner: &'static str, label: impl Into<String>, kind: jobs::Kind) {
        self.next_job += 1;
        let label = label.into();
        self.log(Tone::Plain, format!("started: {label}"));
        self.jobs.push(Job {
            id: self.next_job,
            owner,
            label,
            kind,
            progress: None,
            running: true,
        });
    }

    fn selected(&self, table: &'static str) -> Option<&String> {
        self.pages.selected.get(table)
    }

    fn output(&mut self, page: &'static str, line: impl Into<Line>) {
        let lines = self.pages.output.entry(page).or_default();
        lines.push(line.into());
        if lines.len() > 300 {
            lines.remove(0);
        }
    }

    fn form(&mut self, form: Form) {
        self.dialog = Some(Dialog::Form(form));
    }

    /// The browser policy editor: `name` for one the device has, at
    /// `revision`; a name field and `""` for a new one.
    fn policy_form(&mut self, name: Option<String>, text: &str, revision: String) {
        let title = match &name {
            Some(name) => format!("Policy {name}"),
            None => "New policy".to_string(),
        };
        let mut form = Form::new(
            title,
            "Save",
            Action::PolicySave {
                name: name.clone(),
                revision,
            },
        )
        .intro("One JSON object of Chromium policies (chromeenterprise.google/policies), merged over the image's; comments and trailing commas are fine. A later name wins a policy two of them set. The browser restarts when the result changes.")
        .wide();
        if name.is_none() {
            form = form.field(Field::text("Name", "", "lockdown"));
        }
        form = form.field(
            Field::multiline("Policy", &[text.to_string()], "{ }")
                .mono()
                .tall(),
        );
        form.note = Some(policy_note(text));
        self.form(form);
    }

    /// The timezone dialog: a choice of every zone the device knows, on the
    /// one it has now.
    fn timezone_form(&mut self, zones: &'static [&'static str]) {
        let current = self
            .pages
            .time
            .as_ref()
            .map(|time| time.setting_timezone.as_str())
            .unwrap_or(keys::DEFAULT_TIMEZONE);
        let current = zones
            .iter()
            .find(|zone| **zone == current)
            .copied()
            .unwrap_or(keys::DEFAULT_TIMEZONE);
        self.form(
            Form::new("Timezone", "Set", Action::Timezone)
                .intro("Pages and the journal show local time in it. The browser follows without a restart.")
                .field(Field::choice("Timezone", current, zones)),
        );
    }

    fn secret(&mut self, title: impl Into<String>, intro: impl Into<String>, value: String) {
        self.dialog = Some(Dialog::Secret {
            title: title.into(),
            intro: intro.into(),
            values: vec![(String::new(), value)],
        });
    }

    fn show_text(&mut self, title: impl Into<String>, body: String) {
        self.dialog = Some(Dialog::Text {
            title: title.into(),
            body,
        });
    }

    /// Ask for what `page` shows.
    pub(super) fn refresh_page(&mut self, page: Page) {
        if self.link != Link::Online {
            return;
        }
        match page {
            Page::Screen => self.call("modes", fetch::<api::screen::Modes>()),
            Page::Network => {
                self.call("net", fetch::<api::network::Show>());
                self.call("net.profiles", fetch::<api::network::Profiles>());
            }
            Page::Certs => self.call("certs", fetch::<api::network::Certs>()),
            Page::Policies => self.call("policies", fetch::<api::browser::Policies>()),
            Page::Wifi => {
                self.call("wifi", fetch::<api::network::Wifi>());
                self.call(
                    "wifi.scan",
                    call::<api::network::WifiScan>(
                        WifiScanQuery {
                            interface: None,
                            rescan: false,
                        },
                        (),
                    ),
                );
            }
            Page::Storage => self.call("storage", fetch::<api::storage::Show>()),
            Page::Audio => self.call("audio", fetch::<api::audio::Show>()),
            Page::Time => self.call("time", fetch::<api::time::Show>()),
            Page::Schedules => self.call("schedules", fetch::<api::schedule::List>()),
            Page::Access => self.call("tokens", fetch::<api::access::Tokens>()),
            Page::Ssh => self.call("ssh.keys", fetch::<api::ssh::Keys>()),
            Page::Files => {
                let path = self.pages.files_dir.clone();
                self.call(
                    "files",
                    call::<api::files::List>(
                        FilesQuery {
                            path,
                            recursive: false,
                        },
                        (),
                    ),
                );
            }
            Page::Update => self.call("update", fetch::<api::update::Status>()),
            Page::Overview | Page::Browser | Page::Log => {}
        }
    }

    /// What a page's call got back.
    pub(super) fn answer(&mut self, tag: &'static str, result: Result<Value, String>) {
        if let Some(count) = self.pages.in_flight.get_mut(tag) {
            *count = count.saturating_sub(1);
        }
        let result = match tag {
            "schedule.save" | "schedule.check" | "policy.save" => {
                match self.form_answer(tag, result) {
                    Some(result) => result,
                    None => return,
                }
            }
            _ => result,
        };
        let value = match result {
            Ok(value) => {
                self.pages.errors.remove(page_of(tag));
                value
            }
            Err(error) => {
                self.log(Tone::Bad, format!("{tag}: {error}"));
                self.pages.errors.insert(page_of(tag), error);
                return;
            }
        };
        let handled = self.take_answer(tag, value);
        if let Err(error) = handled {
            self.log(Tone::Bad, format!("{tag}: {error}"));
        }
    }

    /// A schedule or browser policy form's answers belong in the form while
    /// it is open: a refused save keeps it open with the device's reason, a
    /// check says when the calendar fires. What is left for the usual path,
    /// if anything.
    fn form_answer(
        &mut self,
        tag: &'static str,
        result: Result<Value, String>,
    ) -> Option<Result<Value, String>> {
        let form = match &mut self.dialog {
            Some(Dialog::Form(form))
                if match form.action {
                    Action::ScheduleSave(_) => tag.starts_with("schedule."),
                    Action::PolicySave { .. } => tag == "policy.save",
                    _ => false,
                } =>
            {
                form
            }
            // The form is gone: a check means nothing now, a save is logged.
            _ => return (tag != "schedule.check").then_some(result),
        };
        match (tag, result) {
            ("schedule.check", Ok(value)) => {
                form.note = Some(match parse::<CalendarCheck>(value) {
                    Ok(check) => calendar_note(&check),
                    Err(error) => error,
                });
                None
            }
            ("schedule.check", Err(error)) => {
                form.note = Some(error);
                None
            }
            (_, Ok(value)) => {
                self.dialog = None;
                Some(Ok(value))
            }
            (_, Err(error)) => {
                form.error = Some(error);
                None
            }
        }
    }

    fn take_answer(&mut self, tag: &'static str, value: Value) -> Result<(), String> {
        match tag {
            "modes" => self.pages.modes = parse(value)?,
            "schedules" => self.pages.schedules = parse(value)?,
            "schedule.save" | "schedule.set" => {
                let info: ScheduleInfo = parse(value)?;
                let state = if info.spec.enabled { "on" } else { "off" };
                self.log(
                    Tone::Ok,
                    format!("schedule {} saved, {state}", info.spec.name),
                );
                self.pages.selected.insert("schedules", info.id);
                self.call("schedules", fetch::<api::schedule::List>());
            }
            "schedule.run" | "schedule.remove" => {
                let done: Done = parse(value)?;
                self.log(Tone::Ok, done.message);
                if tag == "schedule.remove" {
                    self.pages.selected.remove("schedules");
                }
                self.call("schedules", fetch::<api::schedule::List>());
            }
            "net" => self.pages.net = Some(parse(value)?),
            "net.profiles" => self.pages.profiles = parse(value)?,
            "certs" => self.pages.certs = parse(value)?,
            "cert.add" => {
                let added: CertsAdded = parse(value)?;
                for cert in &added.added {
                    self.log(Tone::Ok, format!("trusted {}", cert.subject));
                }
                for cert in &added.present {
                    self.log(Tone::Plain, format!("already trusted: {}", cert.subject));
                }
                self.call("certs", fetch::<api::network::Certs>());
            }
            "cert.revoke" => {
                let revoked: CertInfo = parse(value)?;
                self.log(Tone::Ok, format!("revoked {}", revoked.subject));
                self.pages.selected.remove("certs");
                self.call("certs", fetch::<api::network::Certs>());
            }
            "policies" => self.pages.policies = parse(value)?,
            "policy.open" => {
                let doc: PolicyDoc = parse(value)?;
                self.policy_form(Some(doc.name), &doc.text, doc.revision);
            }
            "policy.save" => {
                let saved: PolicySaved = parse(value)?;
                for line in describe::browser::policy_saved(&saved) {
                    self.log_line(line);
                }
                self.pages.selected.insert("policies", saved.name);
                self.call("policies", fetch::<api::browser::Policies>());
            }
            "policy.remove" => {
                let removed: PolicyRemoved = parse(value)?;
                for line in describe::browser::policy_removed(&removed) {
                    self.log_line(line);
                }
                self.pages.selected.remove("policies");
                self.call("policies", fetch::<api::browser::Policies>());
            }
            "policies.effective" => {
                let entries: Vec<EffectiveEntry> = parse(value)?;
                self.show_text(
                    "Effective policy",
                    joined(&describe::browser::effective(&entries)),
                );
            }
            "net.profile" => {
                let detail: NetProfileDetail = parse(value)?;
                let title = format!("Profile {}", detail.profile.name);
                self.show_text(title, joined(&describe::net::profile(&detail)));
            }
            "proxy.test" => {
                let tested: ProxyTested = parse(value)?;
                self.log_line(describe::net::proxy_test(&tested));
            }
            "net.last" => {
                let last: Option<NetChange> = parse(value)?;
                let body = match last {
                    Some(change) => joined(&describe::net::change(&change)),
                    None => "no network change yet".to_string(),
                };
                self.show_text("Last network change", body);
            }
            "wifi" => self.pages.wifi = Some(parse(value)?),
            "wifi.scan" => self.pages.networks = parse(value)?,
            "wifi.join" => {
                let applied: Applied = parse(value)?;
                self.log_applied(&applied);
                self.refresh_page(Page::Wifi);
            }
            "wifi.hotspot" => {
                let hotspot: HotspotCredentials = parse(value)?;
                self.secret(
                    format!("Hotspot {}", hotspot.ssid),
                    "The hotspot's new password - shown this once. Anyone on the hotspot now is dropped.",
                    hotspot.password,
                );
            }
            "storage" => self.pages.storage = Some(parse(value)?),
            "audio" => {
                self.pages.audio = Some(parse(value)?);
                self.pages.volume = None;
                self.pages.input_volume = None;
            }
            "audio.test" => {
                let tested: AudioTested = parse(value)?;
                for line in describe::audio::test(&tested) {
                    self.log_line(line);
                }
            }
            "time" => self.pages.time = Some(parse(value)?),
            "time.zones" => {
                let zones = zones(parse(value)?);
                self.timezone_form(zones);
            }
            "tokens" => self.pages.tokens = parse(value)?,
            "webconfig" => {
                let shown: String = parse(value)?;
                self.log(Tone::Ok, format!("opened {shown}"));
            }
            "token.new" => {
                let created: TokenCreated = parse(value)?;
                self.secret(
                    format!("Token {}", created.id),
                    "The new token - shown this once. Log in with it on another machine.",
                    created.token,
                );
                self.refresh_page(Page::Access);
            }
            "password" => {
                let set: Password = parse(value)?;
                match set.password {
                    Some(password) => self.secret(
                        "Root password",
                        "The new root password - shown this once, store it now.",
                        password,
                    ),
                    None => self.log(Tone::Ok, "root password changed"),
                }
            }
            "claim" => {
                let claimed: Claimed = parse(value)?;
                let mut secrets = vec![("Root password".to_string(), claimed.root_password)];
                let mut intro =
                    "The new root password - shown this once, store it now.".to_string();
                if let Some(hotspot) = claimed.hotspot {
                    secrets.push((hotspot.ssid, hotspot.password));
                    intro = "The new root and hotspot passwords - shown this once, store them now. Anyone on the hotspot now is dropped.".to_string();
                }
                self.log(Tone::Ok, format!("claimed {}", self.name()));
                self.dialog = Some(Dialog::Secret {
                    title: format!("Claimed {}", self.name()),
                    intro,
                    values: secrets,
                });
                self.refresh_page(Page::Access);
            }
            "unclaim" | "factory" => {
                let done: Done = parse(value)?;
                self.log(Tone::Warn, done.message);
                self.forget_here();
                // The device stays up after an unclaim, and answers the
                // stale token as it answers none.
                if tag == "unclaim" {
                    self.refresh_page(Page::Access);
                }
            }
            "ssh.keys" => self.pages.ssh_keys = parse(value)?,
            "ssh.revoke" => {
                let revoked: SshKeyRevoked = parse(value)?;
                self.log(Tone::Ok, revoked.message);
                self.refresh_page(Page::Ssh);
            }
            "files" => {
                let listing: FilesListing = parse(value)?;
                self.pages.files = listing.entries;
            }
            "update" => self.pages.update = Some(parse(value)?),
            "browser.eval" => {
                let result: protocol::EvalResult = parse(value)?;
                let line = describe::device::eval(&result).unwrap_or_else(|thrown| thrown);
                // A pretty-printed value is one span over several lines.
                if line.0.len() == 1 && line.0[0].text.contains('\n') {
                    let tone = line.0[0].tone;
                    for part in line.0[0].text.lines() {
                        self.output("browser", Line::of(tone, part));
                    }
                } else {
                    self.output("browser", line);
                }
            }
            "screen.power" => {
                let power: protocol::ScreenPower = parse(value)?;
                self.log(Tone::Ok, if power.on { "screen on" } else { "screen off" });
                self.refresh_page(Page::Overview);
            }
            _ => {
                // Everything else answers with a message and changes what
                // its page shows.
                let done: Done = parse(value)?;
                self.log(Tone::Ok, done.message);
                let page = match page_of(tag) {
                    "files" => Page::Files,
                    "access" => Page::Access,
                    "update" => Page::Update,
                    "net" => Page::Network,
                    "time" => Page::Time,
                    "browser" => Page::Browser,
                    _ => self.page,
                };
                self.refresh_page(page);
            }
        }
        Ok(())
    }

    /// The device dropped every token: this machine's pin and token for it
    /// are worth nothing now.
    fn forget_here(&mut self) {
        if let Ok(mut nodes) = tessaro_client::nodes::Nodes::load() {
            if nodes.forget(&self.node.id) == Ok(true) {
                self.log(
                    Tone::Warn,
                    format!("forgot {} on this machine", self.name()),
                );
            }
        }
    }

    /// The jobs to run, for the app's subscriptions.
    pub fn active_jobs(&self) -> impl Iterator<Item = &Job> {
        self.jobs.iter().filter(|job| job.running)
    }

    /// What a job said.
    pub fn job_event(&mut self, id: u64, event: jobs::Event) {
        let Some(at) = self.jobs.iter().position(|job| job.id == id) else {
            return;
        };
        let owner = self.jobs[at].owner;
        match event {
            jobs::Event::Progress { label, done, total } => {
                self.jobs[at].progress = Some((label, done, total));
            }
            jobs::Event::Line(line) => self.output(owner, line),
            jobs::Event::Value(value) => {
                if let Some(line) = stream_line(owner, &value) {
                    self.output(owner, line);
                }
            }
            jobs::Event::Wiped => {
                self.log(
                    Tone::Warn,
                    "the device comes back unclaimed, with a new name and certificate: find it in the device list and claim it again",
                );
                self.forget_here();
            }
            jobs::Event::Finished(result) => {
                let label = self.jobs[at].label.clone();
                self.jobs.remove(at);
                match result {
                    Ok(message) => {
                        self.output(owner, format!("{label}: {message}"));
                        self.log(Tone::Ok, format!("{label}: {message}"));
                    }
                    Err(error) => {
                        self.output(owner, format!("{label}: {error}"));
                        self.log(Tone::Bad, format!("{label}: {error}"));
                    }
                }
                let page = match owner {
                    "files" => Some(Page::Files),
                    "update" => Some(Page::Update),
                    "storage" => Some(Page::Storage),
                    _ => None,
                };
                if let Some(page) = page {
                    self.refresh_page(page);
                }
            }
        }
    }

    pub(super) fn page_update(&mut self, message: Msg) -> Task<Message> {
        let online = self.link == Link::Online;
        match message {
            Msg::Select(table, key) => {
                self.pages.selected.insert(table, key);
            }
            Msg::Activate(table, key) => {
                self.pages.selected.insert(table, key.clone());
                return self.activate(table, key);
            }
            Msg::Navigate => {
                let url = self
                    .status
                    .as_ref()
                    .and_then(|(status, _)| status.current_url.clone())
                    .unwrap_or_default();
                self.form(
                    Form::new("Navigate", "Go", Action::Navigate)
                        .intro("Open a page now. The kiosk page is back after a restart; set browser.url to change it for good.")
                        .field(Field::text("URL", url, "https://...")),
                );
            }
            Msg::Maintenance(true) => {
                let url = self
                    .setting(keys::MAINTENANCE_URL)
                    .and_then(|setting| setting.value.clone())
                    .unwrap_or_default();
                self.form(
                    Form::new("Maintenance mode", "Turn on", Action::Maintenance)
                        .intro("The screen shows the maintenance page until it is turned off.")
                        .field(Field::text("Page", url, "the image's maintenance page")),
                );
            }
            Msg::Maintenance(false) => self.set(&[(keys::MAINTENANCE_ENABLE, "0")]),
            Msg::DebugScreen(true) => {
                let template = self
                    .setting(keys::DEBUG_TEMPLATE)
                    .and_then(|setting| setting.value.clone())
                    .unwrap_or_default();
                self.form(
                    Form::new("Debug screen", "Turn on", Action::DebugScreen)
                        .intro("The screen shows the device's details until it is turned off. {key} placeholders work as in browser.url.")
                        .field(Field::text("Template", template, "the image's template")),
                );
            }
            Msg::DebugScreen(false) => self.set(&[(keys::DEBUG_ENABLE, "0")]),
            Msg::Zoom => {
                let zoom = self.zoom();
                self.form(
                    Form::new("Page zoom", "Zoom", Action::Zoom)
                        .intro("Percent, 25 to 500: Chrome's Ctrl+/- zoom for every site. 100 is no zoom. The browser restarts.")
                        .field(Field::text("Percent", zoom, "100")),
                );
            }
            Msg::DevTools => {
                if !self.devtools_open() {
                    self.start_job("browser", "DevTools tunnel", jobs::Kind::DevTools);
                }
            }
            Msg::Reload => self.call("browser", send::<api::browser::Reload>(())),
            Msg::ClearCache => self.call("browser", send::<api::browser::ClearCache>(())),
            Msg::Inject => {
                let script = self
                    .setting(keys::INJECT_SCRIPT)
                    .and_then(|setting| setting.value.clone())
                    .unwrap_or_default();
                self.form(
                    Form::new("Inject a script", "Save", Action::Inject)
                        .intro("A file from the file store, run in every page before the page's own scripts. Uploading a new copy reloads the page with it. Empty for none.")
                        .field(Field::text("Script", script, "inject.js")),
                );
            }
            Msg::Bridge => {
                let current = self
                    .setting(keys::BRIDGE_MODE)
                    .and_then(|setting| setting.value.clone())
                    .unwrap_or_default();
                let mode = keys::BRIDGE_MODES
                    .iter()
                    .find(|mode| **mode == current)
                    .copied()
                    .unwrap_or("off");
                self.form(
                    Form::new("Page bridge", "Save", Action::Bridge)
                        .intro("What the page gets as window.tessaro: nothing, the settings (config), or the settings and device actions such as reload, volume and screen power (actions).")
                        .field(Field::choice("Mode", mode, keys::BRIDGE_MODES)),
                );
            }
            Msg::Eval => self.form(
                Form::new("Run JavaScript", "Run", Action::Eval)
                    .intro("Runs in the page on screen now, as tessaro-ctl browser eval. What it returns goes to the output below.")
                    .field(Field::text("Code", "", "document.title")),
            ),
            Msg::ControlPing => self.form(
                Form::new("Ping the device", "Ping", Action::ControlPing)
                    .intro("Round trips over the control connection, as tessaro-ctl device ping.")
                    .field(Field::text("Count", "4", "4")),
            ),
            Msg::FactoryReset => {
                let name = self.name().to_string();
                self.form(
                    Form::new(format!("Factory reset {name}"), "Reset", Action::FactoryReset)
                        .intro(format!(
                            "This will {}. The device comes back unclaimed.",
                            tessaro_client::access::FACTORY_RESET_LOSES
                        ))
                        .typed(),
                );
            }
            Msg::UseMode => {
                if let Some(mode) = self.selected("modes").cloned() {
                    let mode = mode.split_once(' ').map_or(mode.clone(), |(_, mode)| mode.to_string());
                    self.set(&[("screen.resolution", &mode)]);
                }
            }
            Msg::ScreenPower(on) => self.call(
                "screen.power",
                send::<api::screen::PowerSet>(ScreenPowerBody { on }),
            ),
            Msg::Keyboard(show) => self.call(
                "screen.keyboard",
                send::<api::screen::Keyboard>(KeyboardBody {
                    show,
                    selector: None,
                }),
            ),
            Msg::NetLast => self.call("net.last", fetch::<api::network::Last>()),
            Msg::NetPing => self.form(
                Form::new("Ping from the device", "Ping", Action::NetPing)
                    .intro("The device pings a host, as tessaro-ctl network ping.")
                    .field(Field::text("Host", "", "gateway, 1.1.1.1, example.com"))
                    .field(Field::text("Count", "4", "4"))
                    .field(Field::text("Interface", "", "any")),
            ),
            Msg::Speedtest => self.form(
                Form::new("Speed test", "Start", Action::Speedtest)
                    .intro("The device measures against Cloudflare, as tessaro-ctl network speedtest. It uses real traffic.")
                    .field(Field::choice(
                        "Largest transfer",
                        "25m",
                        &["100k", "1m", "10m", "25m", "100m"],
                    ))
                    .field(Field::check("Bypass the proxy", false)),
            ),
            Msg::Proxy => {
                // The URL without its password, which goes in its own
                // field: typed there it needs no percent-encoding.
                let current = |key| {
                    self.setting(key)
                        .and_then(|setting| setting.value.clone())
                        .unwrap_or_default()
                };
                let url = match keys::parse_proxy(&current(keys::PROXY_URL)) {
                    Ok(mut proxy) => {
                        proxy.password = None;
                        proxy.to_string()
                    }
                    Err(_) => String::new(),
                };
                self.form(
                    Form::new("Proxy", "Use it", Action::Proxy)
                        .intro("Everything the device fetches from the internet goes through it: the browser, the reachability probe, the public address and the speed test. http://host:port or socks5://host:port. The browser restarts when the proxy is switched on.")
                        .field(Field::text("URL", url, "http://10.0.0.5:3128"))
                        .field(Field::text("User", "", "only if the proxy wants a login"))
                        .field(Field::secret("Password"))
                        .field(Field::text("Bypass", current(keys::PROXY_BYPASS), ".corp.test, 10.0.0.0/8")),
                );
            }
            Msg::ProxyOff => {
                self.set(&[(keys::PROXY_URL, "")]);
                self.call("net", fetch::<api::network::Show>());
            }
            Msg::ProxyTest => {
                self.log(Tone::Plain, "testing the proxy ...");
                self.call_long("proxy.test", send::<api::network::ProxyTest>(()));
            }
            Msg::ProfileDetail => {
                if let Some(profile) = self.selected("profiles").cloned() {
                    self.call(
                        "net.profile",
                        call::<api::network::Profile>(ProfileQuery { profile }, ()),
                    );
                }
            }
            Msg::CertPick => {
                return Task::perform(
                    rfd::AsyncFileDialog::new()
                        .set_title("Certificate authority to trust")
                        .add_filter("Certificate", &["pem", "crt", "cer", "der"])
                        .pick_file(),
                    |picked| Message::P(Msg::CertPicked(picked.map(|handle| handle.path().to_path_buf()))),
                );
            }
            Msg::CertPicked(Some(file)) => match tessaro_client::certs::read_pem(&file) {
                Ok(pem) => self.call("cert.add", send::<api::network::CertAdd>(CertBody { pem })),
                Err(error) => self.log(Tone::Bad, error),
            },
            Msg::CertPicked(None) => {}
            Msg::PolicyNew => self.policy_form(None, policy::TEMPLATE, String::new()),
            Msg::PolicyPick => {
                return Task::perform(
                    rfd::AsyncFileDialog::new()
                        .set_title("Browser policy to open")
                        .add_filter("Policy", &["json", "jsonc"])
                        .pick_file(),
                    |picked| {
                        Message::P(Msg::PolicyPicked(
                            picked.map(|handle| handle.path().to_path_buf()),
                        ))
                    },
                )
            }
            // Opened as it is, even with a mistake in it: the form says
            // where, and it can be fixed there before it is sent.
            Msg::PolicyPicked(Some(file)) => match read_policy_file(&file) {
                Ok(text) => {
                    self.policy_form(None, &text, String::new());
                    if let Some(Dialog::Form(form)) = &mut self.dialog {
                        if let Some(name) = form.fields.iter_mut().find(|f| f.label == "Name") {
                            name.value = policy_name_of(&file);
                        }
                    }
                }
                Err(error) => self.log(Tone::Bad, error),
            },
            Msg::PolicyPicked(None) => {}
            Msg::PolicyEdit => {
                if let Some(name) = self.selected("policies").cloned() {
                    self.call(
                        "policy.open",
                        call::<api::browser::Policy>(PolicyRef { name }, ()),
                    );
                }
            }
            Msg::PolicyEffective => {
                self.call("policies.effective", fetch::<api::browser::Effective>())
            }
            Msg::PolicyRemove => {
                if let Some(name) = self.selected("policies").cloned() {
                    self.form(
                        Form::new(
                            format!("Remove policy {name}"),
                            "Remove",
                            Action::PolicyRemove(name),
                        )
                        .intro("Its Chromium policies leave the browser's policy, and the browser restarts to drop them."),
                    );
                }
            }
            Msg::CertRevoke => {
                let chosen = self
                    .selected("certs")
                    .and_then(|fingerprint| {
                        self.pages
                            .certs
                            .iter()
                            .find(|cert| &cert.fingerprint == fingerprint)
                    })
                    .cloned();
                if let Some(cert) = chosen {
                    self.form(
                        Form::new(
                            "Revoke certificate authority",
                            "Revoke",
                            Action::CertRevoke(cert.fingerprint.clone()),
                        )
                        .intro(format!(
                            "The device stops trusting {}. Sites whose certificates it signed stop loading. The agent restarts; the browser does not.",
                            cert.subject
                        )),
                    );
                }
            }
            Msg::ScheduleNew => self.form(schedule_form(None)),
            Msg::ScheduleEdit => {
                if let Some(info) = self.selected_schedule() {
                    self.form(schedule_form(Some(&info)));
                    return self.check_calendar_soon();
                }
            }
            Msg::ScheduleToggle => {
                if let Some(info) = self.selected_schedule() {
                    self.call(
                        "schedule.set",
                        call::<api::schedule::Change>(
                            ScheduleRef { schedule: info.id },
                            ScheduleChange {
                                enabled: Some(!info.spec.enabled),
                                ..ScheduleChange::default()
                            },
                        ),
                    );
                }
            }
            Msg::ScheduleRun => {
                if let Some(info) = self.selected_schedule() {
                    self.call(
                        "schedule.run",
                        call::<api::schedule::Run>(ScheduleRef { schedule: info.id }, ()),
                    );
                }
            }
            Msg::ScheduleLogs => {
                if let Some(info) = self.selected_schedule() {
                    self.journal_of(info.units);
                }
            }
            Msg::ScheduleRemove => {
                if let Some(info) = self.selected_schedule() {
                    self.form(
                        Form::new(
                            format!("Remove schedule {}", info.spec.name),
                            "Remove",
                            Action::ScheduleRemove(info.id),
                        )
                        .intro("Its timer stops; runs already going finish."),
                    );
                }
            }
            Msg::ScheduleCheckDue(edit) => {
                if edit == self.pages.calendar_edits {
                    self.check_calendar();
                }
            }
            Msg::WifiScan => self.call(
                "wifi.scan",
                call::<api::network::WifiScan>(
                    WifiScanQuery {
                        interface: None,
                        rescan: true,
                    },
                    (),
                ),
            ),
            Msg::WifiJoin => {
                let ssid = self
                    .selected("wifi")
                    .and_then(|bssid| {
                        self.pages
                            .networks
                            .iter()
                            .find(|network| &network.bssid == bssid)
                    })
                    .map(|network| network.ssid.clone())
                    .unwrap_or_default();
                self.form(
                    Form::new("Join a WiFi network", "Join", Action::WifiJoin)
                        .intro("A network change: the device keeps it only if it still reaches its gateway, and rolls it back otherwise.")
                        .field(Field::text("SSID", ssid, "network name"))
                        .field(Field::secret("Password"))
                        .field(Field::choice("Security", AUTO, wifi_securities()))
                        .field(Field::check("Hidden", false)),
                );
            }
            Msg::Hotspot => self.form(
                Form::new("New hotspot password", "Change", Action::Hotspot)
                    .intro("A new random password for the device's hotspot, shown once. Anyone on the hotspot now is dropped."),
            ),
            Msg::GrowCheck => self.start_job(
                "storage",
                "grow check",
                jobs::Kind::Stream(jobs::Stream::Grow(GrowBody { check: true })),
            ),
            Msg::Grow => self.form(
                Form::new("Grow /data", "Grow", Action::Grow)
                    .intro("Grow the /data partition and its filesystem over the free space after it, online.")
                    .typed(),
            ),
            Msg::UseAudio(side) => {
                let table = if side == keys::AUDIO_OUTPUT { "outputs" } else { "inputs" };
                if let Some(name) = self.selected(table).cloned() {
                    self.set(&[(side, &name)]);
                    self.call("audio", fetch::<api::audio::Show>());
                }
            }
            Msg::Volume(volume) => self.pages.volume = Some(volume),
            Msg::VolumeDone => {
                if let Some(volume) = self.pages.volume {
                    self.set(&[(keys::AUDIO_VOLUME, &volume.to_string())]);
                    self.call("audio", fetch::<api::audio::Show>());
                }
            }
            Msg::InputVolume(volume) => self.pages.input_volume = Some(volume),
            Msg::InputVolumeDone => {
                if let Some(volume) = self.pages.input_volume {
                    self.set(&[(keys::AUDIO_INPUT_VOLUME, &volume.to_string())]);
                    self.call("audio", fetch::<api::audio::Show>());
                }
            }
            Msg::Mute(on) => {
                self.set(&[(keys::AUDIO_MUTE, flag(on))]);
                self.call("audio", fetch::<api::audio::Show>());
            }
            Msg::InputMute(on) => {
                let input = if on { "off" } else { "auto" };
                self.set(&[(keys::AUDIO_INPUT, input)]);
                self.call("audio", fetch::<api::audio::Show>());
            }
            Msg::Test(input) => {
                self.log(
                    Tone::Plain,
                    if input {
                        "recording a few seconds from the input ..."
                    } else {
                        "playing a test tone ..."
                    },
                );
                self.call_long(
                    "audio.test",
                    send::<api::audio::Test>(AudioTestBody { input }),
                );
            }
            Msg::Timezone => match ZONES.get() {
                Some(zones) => self.timezone_form(zones),
                None => self.call("time.zones", fetch::<api::time::Zones>()),
            },
            Msg::Ntp => {
                let (on, servers) = self
                    .pages
                    .time
                    .as_ref()
                    .map(|time| (time.ntp.unwrap_or(true), time.setting_servers.join(", ")))
                    .unwrap_or((true, String::new()));
                self.form(
                    Form::new("NTP", "Apply", Action::Ntp)
                        .intro("Keep the clock in sync over NTP. With no servers the device uses the ones the network's DHCP offers, else the image's fallback. Only systemd-timesyncd restarts.")
                        .field(Field::check("Sync over NTP", on))
                        .field(Field::text("Servers", servers, "from DHCP, else the fallback")),
                );
            }
            Msg::TimeSync => self.call("time.sync", send::<api::time::Sync>(())),
            Msg::SetClock => {
                let now = self
                    .pages
                    .time
                    .as_ref()
                    .and_then(|time| time.local_time.clone())
                    .unwrap_or_default();
                self.form(
                    Form::new("Set the clock", "Set", Action::SetClock)
                        .intro("With NTP off only. Either this computer's clock, or a time in the device's timezone as YYYY-MM-DD HH:MM[:SS].")
                        .field(Field::check("Use this computer's clock", true))
                        .field(Field::text("Time", now, "YYYY-MM-DD HH:MM")),
                );
            }
            Msg::TokenNew => self.form(
                Form::new("New token", "Create", Action::TokenCreate)
                    .intro("A token for another machine or person; it is shown once.")
                    .field(Field::text("Name", "", "who it is for")),
            ),
            Msg::TokenRevoke => {
                if let Some(id) = self.selected("tokens").cloned() {
                    self.form(
                        Form::new(format!("Revoke token {id}"), "Revoke", Action::TokenRevoke(id))
                            .intro("Whoever holds it can no longer manage the device. Revoking the last token unclaims it."),
                    );
                }
            }
            // On the worker, which has the session: a claimed device issues
            // the ticket the address carries.
            Msg::Webconfig => self.call(
                "webconfig",
                Box::new(|session| {
                    let opened = webconfig::address(session)
                        .and_then(|address| webconfig::open(&address.url).map(|()| address.shown));
                    match opened {
                        Ok(shown) => Answer::Ok(Value::String(shown)),
                        Err(error) => Answer::Refused(error),
                    }
                }),
            ),
            Msg::Password => self.form(
                Form::new("Root password", "Set", Action::Password)
                    .intro("The root password for the console and SSH. Leave both empty for a random one, shown once.")
                    .field(Field::secret("Password"))
                    .field(Field::secret("Again")),
            ),
            Msg::Claim => {
                let name = self.name().to_string();
                let fingerprint = self
                    .info
                    .as_ref()
                    .map_or_else(String::new, |info| info.fingerprint.clone());
                self.form(
                    Form::new(format!("Claim {name}"), "Claim", Action::Claim)
                        .intro(format!("This machine gets the device's token and pins the certificate this window is connected on ({fingerprint}). The device sets a new root password and hotspot password, shown once."))
                        .field(Field::text("Claim as", tessaro_client::client_name(), "who is claiming it")),
                );
            }
            Msg::Unclaim => {
                let name = self.name().to_string();
                self.form(
                    Form::new(format!("Unclaim {name}"), "Unclaim", Action::Unclaim)
                        .intro(format!(
                            "This will {}. Settings stay.",
                            tessaro_client::access::UNCLAIM_LOSES
                        ))
                        .typed(),
                );
            }
            Msg::KeyRevoke => {
                if let Some(key) = self.selected("ssh").cloned() {
                    self.form(
                        Form::new("Revoke SSH key", "Revoke", Action::KeyRevoke(key.clone()))
                            .intro(format!("{key} can no longer log in as root.")),
                    );
                }
            }
            Msg::Authorize(terminal) => {
                let node = self.node.clone();
                return Task::perform(
                    blocking::run(move || {
                        let (mut session, _) = crate::worker::connect(&node)?;
                        let authorized = tessaro_client::ssh::authorize(&mut session, None)?;
                        let command = tessaro_client::ssh::shell_words(
                            &authorized.argv(tessaro_client::ssh::PORT, &[]),
                        );
                        Ok((command, authorized.lines(&session.node.name, false)))
                    }),
                    move |result| Message::P(Msg::Authorized(terminal, result)),
                );
            }
            Msg::Authorized(terminal, Ok((command, lines))) => {
                for line in lines {
                    self.log_line(line);
                }
                self.log_line(Line::of(Tone::Cmd, command.clone()));
                if terminal {
                    if let Err(error) = open_terminal(&command) {
                        self.log(Tone::Bad, error);
                    }
                }
                self.refresh_page(Page::Ssh);
            }
            Msg::Authorized(_, Err(error)) => self.log(Tone::Bad, error),
            Msg::FilesUp => {
                let dir = &self.pages.files_dir;
                self.pages.files_dir = dir.rsplit_once('/').map_or(String::new(), |(up, _)| up.to_string());
                self.pages.selected.remove("files");
                self.refresh_page(Page::Files);
            }
            Msg::Upload(folder) => {
                let dialog = rfd::AsyncFileDialog::new().set_title("Upload to the device");
                return Task::perform(
                    async move {
                        let picked = if folder {
                            dialog.pick_folders().await
                        } else {
                            dialog.pick_files().await
                        };
                        picked.map(|handles| {
                            handles
                                .into_iter()
                                .map(|handle| handle.path().to_path_buf())
                                .collect()
                        })
                    },
                    |picked| Message::P(Msg::Uploads(picked)),
                );
            }
            Msg::Uploads(Some(local)) if !local.is_empty() => {
                let into = self.pages.files_dir.clone();
                let label = format!("upload into /{into}");
                self.start_job("files", label, jobs::Kind::Upload { local, into });
            }
            Msg::Uploads(_) => {}
            Msg::Download => {
                return Task::perform(
                    rfd::AsyncFileDialog::new()
                        .set_title("Download into")
                        .pick_folder(),
                    |picked| Message::P(Msg::DownloadTo(picked.map(|handle| handle.path().to_path_buf()))),
                );
            }
            Msg::DownloadTo(Some(into)) => {
                if let Some(entry) = self.selected_file() {
                    let label = format!("download {}", entry.path);
                    self.start_job("files", label, jobs::Kind::Download { entry, into });
                }
            }
            Msg::DownloadTo(None) => {}
            Msg::Mkdir => self.form(
                Form::new("New folder", "Create", Action::Mkdir)
                    .field(Field::text("Name", "", "folder name")),
            ),
            Msg::Move => {
                if let Some(entry) = self.selected_file() {
                    self.form(
                        Form::new(format!("Move {}", entry.path), "Move", Action::Move(entry.path.clone()))
                            .intro("A new path from the root of the store: rename it, or move it into another folder.")
                            .field(Field::text("To", entry.path, "path/in/the/store")),
                    );
                }
            }
            Msg::Delete => {
                if let Some(entry) = self.selected_file() {
                    let what = match entry.kind {
                        FileKind::Dir => format!("{}/ and everything in it", entry.path),
                        FileKind::File => entry.path.clone(),
                    };
                    self.form(
                        Form::new("Delete", "Delete", Action::Delete(vec![entry.path]))
                            .intro(format!("Delete {what} from the device.")),
                    );
                }
            }
            Msg::UpdatePick => {
                return Task::perform(
                    rfd::AsyncFileDialog::new()
                        .set_title("Image to send")
                        .add_filter("Tessaro image", &["zst", "bz2", "wic"])
                        .pick_file(),
                    |picked| Message::P(Msg::UpdatePicked(picked.map(|handle| handle.path().to_path_buf()))),
                );
            }
            Msg::UpdatePicked(Some(image)) => {
                let bmap = tessaro_client::transfer::bmap_for(&image);
                let name = image
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.form(
                    Form::new(format!("Send {name}"), "Send", Action::UpdateSend(image))
                        .intro("Upload the image, have the device check and stage it, then commit it. The kiosk keeps running until the reboot writes it.")
                        .field(Field::text("Block map", bmap.display().to_string(), "IMAGE.bmap"))
                        .field(Field::check("Reboot to apply", true))
                        .field(Field::check("Erase /data", false))
                        .field(Field::check("Rewrite the whole disk", false))
                        .field(Field::check("Skip the checksum check", false)),
                );
            }
            Msg::UpdatePicked(None) => {}
            Msg::UpdateCancel => self.form(
                Form::new("Cancel the update", "Cancel update", Action::UpdateCancel)
                    .intro("Drop the staged or pending image; the device keeps running what it runs."),
            ),
            Msg::CancelJob(id) => {
                if let Some(job) = self.jobs.iter_mut().find(|job| job.id == id) {
                    job.running = false;
                    let label = job.label.clone();
                    self.log(Tone::Warn, format!("cancelled: {label}"));
                }
                self.jobs.retain(|job| job.running);
            }
            Msg::ClearOutput => {
                let page = page_key(self.page);
                self.pages.output.remove(page);
            }
            Msg::FormText(at, value) => self.form_field(at, value),
            Msg::FormEdit(at, action) => return self.form_edit(at, action),
            Msg::FormCheck(at, on) => self.form_field(at, flag(on).to_string()),
            Msg::FormChoice(at, value) => self.form_field(at, value.to_string()),
            Msg::FormOk => {
                if online || matches!(self.dialog, Some(Dialog::Form(_))) {
                    self.form_ok();
                }
            }
            Msg::Copy(value) => return iced::clipboard::write(value),
        }
        Task::none()
    }

    fn activate(&mut self, table: &'static str, key: String) -> Task<Message> {
        match table {
            "files" => {
                let dir = self
                    .pages
                    .files
                    .iter()
                    .find(|entry| entry.path == key && entry.kind == FileKind::Dir)
                    .is_some();
                if dir {
                    self.pages.files_dir = key;
                    self.pages.selected.remove("files");
                    self.refresh_page(Page::Files);
                    Task::none()
                } else {
                    self.page_update(Msg::Download)
                }
            }
            "profiles" => self.page_update(Msg::ProfileDetail),
            "wifi" => self.page_update(Msg::WifiJoin),
            "outputs" => self.page_update(Msg::UseAudio(keys::AUDIO_OUTPUT)),
            "inputs" => self.page_update(Msg::UseAudio(keys::AUDIO_INPUT)),
            "modes" => self.page_update(Msg::UseMode),
            "schedules" => self.page_update(Msg::ScheduleEdit),
            "policies" => self.page_update(Msg::PolicyEdit),
            _ => Task::none(),
        }
    }

    /// The main table of the page shown, and its keys, for Up and Down.
    fn primary(&self) -> Option<(&'static str, Vec<String>)> {
        let keys = match self.page {
            Page::Files => (
                "files",
                self.pages
                    .files
                    .iter()
                    .map(|entry| entry.path.clone())
                    .collect(),
            ),
            Page::Wifi => (
                "wifi",
                self.pages
                    .networks
                    .iter()
                    .map(|network| network.bssid.clone())
                    .collect(),
            ),
            Page::Network => (
                "profiles",
                self.pages
                    .profiles
                    .iter()
                    .map(|profile| profile.name.clone())
                    .collect(),
            ),
            Page::Certs => (
                "certs",
                self.pages
                    .certs
                    .iter()
                    .map(|cert| cert.fingerprint.clone())
                    .collect(),
            ),
            Page::Policies => (
                "policies",
                self.pages
                    .policies
                    .iter()
                    .map(|info| info.name.clone())
                    .collect(),
            ),
            Page::Schedules => (
                "schedules",
                self.pages
                    .schedules
                    .iter()
                    .map(|info| info.id.clone())
                    .collect(),
            ),
            Page::Access => (
                "tokens",
                self.pages
                    .tokens
                    .iter()
                    .map(|token| token.id.clone())
                    .collect(),
            ),
            Page::Ssh => (
                "ssh",
                self.pages
                    .ssh_keys
                    .iter()
                    .map(|key| key.fingerprint.clone())
                    .collect(),
            ),
            Page::Audio => (
                "outputs",
                self.pages
                    .audio
                    .iter()
                    .flat_map(|audio| {
                        audio
                            .output
                            .devices
                            .iter()
                            .map(|device| device.name.clone())
                    })
                    .collect(),
            ),
            Page::Screen => (
                "modes",
                self.pages
                    .modes
                    .iter()
                    .flat_map(|connector| {
                        connector
                            .modes
                            .iter()
                            .map(move |mode| format!("{} {mode}", connector.name))
                    })
                    .collect(),
            ),
            _ => return None,
        };
        Some(keys)
    }

    pub(super) fn primary_step(&mut self, by: i32) {
        if let Some((table, keys)) = self.primary() {
            let keys = self.tables.ordered(table, &keys);
            if let Some(key) = section::step(&keys, self.selected(table), by) {
                self.pages.selected.insert(table, key);
            }
        }
    }

    /// Enter on a page: what a double-click on the selected row does.
    pub(super) fn primary_activate(&self) -> Option<Message> {
        let (table, _) = self.primary()?;
        let key = self.selected(table)?.clone();
        Some(Message::P(Msg::Activate(table, key)))
    }

    fn selected_file(&self) -> Option<FileEntry> {
        let path = self.selected("files")?;
        self.pages
            .files
            .iter()
            .find(|entry| &entry.path == path)
            .cloned()
    }

    fn selected_schedule(&self) -> Option<ScheduleInfo> {
        let id = self.selected("schedules")?;
        self.pages
            .schedules
            .iter()
            .find(|info| &info.id == id)
            .cloned()
    }

    /// An edit in a multi-line field. An edit of a schedule's calendar asks
    /// the device when it fires, once the typing pauses.
    fn form_edit(&mut self, at: usize, action: text_editor::Action) -> Task<Message> {
        let Some(Dialog::Form(form)) = &mut self.dialog else {
            return Task::none();
        };
        let schedule = matches!(form.action, Action::ScheduleSave(_));
        let policy = matches!(form.action, Action::PolicySave { .. });
        let Some(field) = form.fields.get_mut(at) else {
            return Task::none();
        };
        let Some(editor) = &mut field.editor else {
            return Task::none();
        };
        let edited = action.is_edit();
        editor.perform(action);
        if !edited {
            return Task::none();
        }
        field.value = editor.text();
        let label = field.label;
        if policy && label == "Policy" {
            // The check is local and quick: every edit gets it.
            form.note = Some(policy_note(&field.value));
        }
        form.error = None;
        if schedule && label == "Calendar" {
            self.check_calendar_soon()
        } else {
            Task::none()
        }
    }

    /// A calendar check in a moment, unless the calendar changes again.
    fn check_calendar_soon(&mut self) -> Task<Message> {
        const PAUSE: std::time::Duration = std::time::Duration::from_millis(400);
        self.pages.calendar_edits += 1;
        let edit = self.pages.calendar_edits;
        Task::perform(
            blocking::run(|| {
                std::thread::sleep(PAUSE);
                Ok(())
            }),
            move |_: Result<(), String>| Message::P(Msg::ScheduleCheckDue(edit)),
        )
    }

    fn check_calendar(&mut self) {
        let Some(Dialog::Form(form)) = &mut self.dialog else {
            return;
        };
        let calendar = form.lines("Calendar");
        if calendar.is_empty() {
            form.note = None;
            return;
        }
        self.call(
            "schedule.check",
            send::<api::schedule::Check>(CalendarBody {
                calendar,
                count: Some(3),
            }),
        );
    }

    fn form_field(&mut self, at: usize, value: String) {
        if let Some(Dialog::Form(form)) = &mut self.dialog {
            if let Some(field) = form.fields.get_mut(at) {
                field.value = value;
                form.error = None;
            }
        }
    }

    /// The form's OK: check it, close it, do what it asked for.
    fn form_ok(&mut self) {
        let name = self.name().to_string();
        let Some(Dialog::Form(form)) = &mut self.dialog else {
            return;
        };
        if form.typed && form.value("Device name").trim() != name {
            form.error = Some(format!("type {name} to confirm"));
            return;
        }
        let Some(Dialog::Form(form)) = self.dialog.take() else {
            return;
        };
        match self.run_action(&form) {
            Err(error) => {
                let mut form = form;
                form.error = Some(error);
                self.dialog = Some(Dialog::Form(form));
            }
            // Open until the device takes it: a refusal is shown in it.
            Ok(())
                if matches!(
                    form.action,
                    Action::ScheduleSave(_) | Action::PolicySave { .. }
                ) =>
            {
                self.dialog = Some(Dialog::Form(form));
            }
            Ok(()) => {}
        }
    }

    fn run_action(&mut self, form: &Form) -> Result<(), String> {
        let count = |text: &str| -> Result<u32, String> {
            match text.trim() {
                "" => Ok(4),
                text => text
                    .parse::<u32>()
                    .ok()
                    .filter(|count| (1..=protocol::PING_MAX_COUNT).contains(count))
                    .ok_or_else(|| format!("a count from 1 to {}", protocol::PING_MAX_COUNT)),
            }
        };
        match &form.action {
            Action::Navigate => {
                let url = form.value("URL").trim().to_string();
                if url.is_empty() {
                    return Err("a URL, please".to_string());
                }
                self.call(
                    "browser",
                    send::<api::browser::Navigate>(NavigateBody { url }),
                );
            }
            Action::Maintenance => {
                let url = form.value("Page").trim();
                let mut values = vec![(keys::MAINTENANCE_ENABLE, "1")];
                if !url.is_empty() {
                    super::check(keys::MAINTENANCE_URL, url)?;
                    values.push((keys::MAINTENANCE_URL, url));
                }
                self.set(&values);
            }
            Action::DebugScreen => {
                let template = form.value("Template").trim();
                let mut values = vec![(keys::DEBUG_ENABLE, "1")];
                if !template.is_empty() {
                    super::check(keys::DEBUG_TEMPLATE, template)?;
                    values.push((keys::DEBUG_TEMPLATE, template));
                }
                self.set(&values);
            }
            Action::Zoom => {
                let zoom = form.value("Percent").trim();
                super::check(keys::ZOOM, zoom)?;
                self.set(&[(keys::ZOOM, zoom)]);
            }
            Action::Inject => {
                let script = form.value("Script").trim();
                let script = super::check(keys::INJECT_SCRIPT, script)?;
                self.set(&[(keys::INJECT_SCRIPT, &script)]);
            }
            Action::Bridge => {
                let mode = form.value("Mode");
                self.set(&[(keys::BRIDGE_MODE, mode)]);
            }
            Action::Eval => {
                let code = form.value("Code").trim().to_string();
                if code.is_empty() {
                    return Err("some code, please".to_string());
                }
                self.output("browser", format!("> {code}"));
                self.call(
                    "browser.eval",
                    send::<api::browser::Eval>(EvalBody {
                        code,
                        timeout_ms: None,
                        await_promise: true,
                        user_gesture: false,
                    }),
                );
            }
            Action::Timezone => {
                let zone = form.value("Timezone").trim();
                super::check(keys::TIMEZONE, zone)?;
                self.set(&[(keys::TIMEZONE, zone)]);
                self.call("time", fetch::<api::time::Show>());
            }
            Action::Ntp => {
                // As `tessaro-ctl time ntp on|off --server ...`; emptying the
                // field goes back to DHCP's servers, as `config unset` would.
                let servers: Vec<String> = form
                    .value("Servers")
                    .split([',', ' '])
                    .map(str::to_string)
                    .collect();
                let mut values =
                    tessaro_client::actions::ntp_change(form.checked("Sync over NTP"), &servers)?;
                let had = self
                    .pages
                    .time
                    .as_ref()
                    .is_some_and(|time| !time.setting_servers.is_empty());
                if had && !values.contains_key(keys::NTP_SERVERS) {
                    values.insert(keys::NTP_SERVERS.to_string(), String::new());
                }
                if let Some(servers) = values.get(keys::NTP_SERVERS) {
                    super::check(keys::NTP_SERVERS, servers)?;
                }
                let values: Vec<(&str, &str)> = values
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.as_str()))
                    .collect();
                self.set(&values);
                self.call("time", fetch::<api::time::Show>());
            }
            Action::SetClock => {
                let local =
                    (!form.checked("Use this computer's clock")).then(|| form.value("Time"));
                let body = tessaro_client::actions::set_clock(local)?;
                self.call("time.set", send::<api::time::Set>(body));
            }
            Action::ControlPing => {
                let count = count(form.value("Count"))?;
                self.start_job("overview", "ping", jobs::Kind::ControlPing { count });
            }
            Action::FactoryReset => {
                self.call_long("factory", send::<api::device::FactoryReset>(()))
            }
            Action::NetPing => {
                let host = form.value("Host").trim().to_string();
                if host.is_empty() {
                    return Err("a host, please".to_string());
                }
                let interface = Some(form.value("Interface").trim().to_string())
                    .filter(|interface| !interface.is_empty());
                let body = PingBody {
                    host: host.clone(),
                    count: Some(count(form.value("Count"))?),
                    interval_ms: None,
                    timeout_ms: None,
                    interface,
                };
                self.start_job(
                    "net",
                    format!("ping {host}"),
                    jobs::Kind::Stream(jobs::Stream::Ping(body)),
                );
            }
            Action::Speedtest => {
                let size = match form.value("Largest transfer") {
                    "100k" => 100_000,
                    "1m" => 1_000_000,
                    "10m" => 10_000_000,
                    "100m" => 100_000_000,
                    _ => 25_000_000,
                };
                let body = SpeedtestBody {
                    max_size: Some(size),
                    tests: None,
                    direct: form.checked("Bypass the proxy"),
                };
                self.start_job(
                    "net",
                    "speed test",
                    jobs::Kind::Stream(jobs::Stream::Speedtest(body)),
                );
            }
            Action::Proxy => {
                let url = tessaro_client::network::proxy_url(
                    form.value("URL"),
                    form.value("User"),
                    form.value("Password"),
                )?;
                let bypass = super::check(keys::PROXY_BYPASS, form.value("Bypass").trim())?;
                self.set(&[(keys::PROXY_URL, &url), (keys::PROXY_BYPASS, &bypass)]);
                self.call("net", fetch::<api::network::Show>());
            }
            Action::WifiJoin => {
                let ssid = form.value("SSID").trim().to_string();
                if ssid.is_empty() {
                    return Err("the network's name, please".to_string());
                }
                // As `tessaro-ctl network wifi join`: the last scan says
                // whether it is open and whether the device knows it.
                let security = match form.value("Security") {
                    AUTO => None,
                    named => Some(named.parse::<WifiSecurity>()?),
                };
                let seen = self
                    .pages
                    .networks
                    .iter()
                    .find(|network| network.ssid == ssid);
                let psk =
                    tessaro_client::network::wifi_psk(form.value("Password"), security, seen)?;
                let verify = Verify::default();
                self.log_line(tessaro_client::network::notice(
                    self.name(),
                    &format!("joining {ssid}"),
                    &verify,
                ));
                let body = WifiJoinBody {
                    ssid,
                    psk,
                    security,
                    hidden: form.checked("Hidden"),
                    verify,
                };
                self.call_long("wifi.join", send::<api::network::WifiJoin>(body));
            }
            Action::Hotspot => self.call("wifi.hotspot", send::<api::network::HotspotPassword>(())),
            Action::Grow => self.start_job(
                "storage",
                "grow /data",
                jobs::Kind::Stream(jobs::Stream::Grow(GrowBody { check: false })),
            ),
            Action::TokenCreate => {
                let name = form.value("Name").trim().to_string();
                if name.is_empty() {
                    return Err("a name, please".to_string());
                }
                self.call(
                    "token.new",
                    send::<api::access::TokenCreate>(NameBody { name }),
                );
            }
            Action::TokenRevoke(id) => self.call(
                "token.revoke",
                call::<api::access::TokenRevoke>(TokenRef { id: id.clone() }, ()),
            ),
            Action::Password => {
                let password = tessaro_client::actions::root_password(
                    form.value("Password"),
                    form.value("Again"),
                )?;
                self.call(
                    "password",
                    send::<api::access::Password>(PasswordBody { password }),
                );
            }
            Action::Claim => {
                let name = form.value("Claim as").to_string();
                if self.request(Request::Claim { name }) {
                    *self.pages.in_flight.entry("claim").or_default() += 1;
                }
            }
            Action::Unclaim => self.call("unclaim", send::<api::access::Unclaim>(())),
            Action::KeyRevoke(key) => self.call(
                "ssh.revoke",
                call::<api::ssh::Revoke>(SshKeyQuery { key: key.clone() }, ()),
            ),
            Action::CertRevoke(cert) => self.call(
                "cert.revoke",
                call::<api::network::CertRevoke>(CertQuery { cert: cert.clone() }, ()),
            ),
            Action::ScheduleSave(id) => {
                let timeout =
                    tessaro_client::schedule::parse_timeout(match form.value("Timeout").trim() {
                        "" => "none",
                        timeout => timeout,
                    })?;
                let on_error = form.value("On error").parse::<OnError>()?;
                let name = form.value("Name").trim().to_string();
                let calendar = form.lines("Calendar");
                let lines = tessaro_client::schedule::command_lines(form.value("Commands"));
                let enabled = form.checked("Enabled");
                let save = match id {
                    None => send::<api::schedule::Create>(ScheduleSpec {
                        name,
                        enabled,
                        calendar,
                        lines,
                        on_error,
                        timeout_s: (timeout > 0).then_some(timeout),
                    }),
                    Some(id) => call::<api::schedule::Change>(
                        ScheduleRef {
                            schedule: id.clone(),
                        },
                        ScheduleChange {
                            name: Some(name),
                            calendar: Some(calendar),
                            lines: Some(lines),
                            on_error: Some(on_error),
                            timeout_s: Some(timeout),
                            enabled: Some(enabled),
                        },
                    ),
                };
                self.call("schedule.save", save);
            }
            Action::PolicySave { name, revision } => {
                let name = match name {
                    Some(name) => name.clone(),
                    None => form.value("Name").trim().to_string(),
                };
                policy::check_name(&name)?;
                let text = form.value("Policy").to_string();
                policy::check(&text).map_err(|err| err.to_string())?;
                self.call(
                    "policy.save",
                    call::<api::browser::PolicySet>(
                        PolicyRef { name },
                        PolicyBody {
                            text,
                            if_revision: Some(revision.clone()),
                        },
                    ),
                );
            }
            Action::PolicyRemove(name) => self.call(
                "policy.remove",
                call::<api::browser::PolicyRemove>(PolicyRef { name: name.clone() }, ()),
            ),
            Action::ScheduleRemove(id) => self.call(
                "schedule.remove",
                call::<api::schedule::Remove>(
                    ScheduleRef {
                        schedule: id.clone(),
                    },
                    (),
                ),
            ),
            Action::Mkdir => {
                let name = form.value("Name").trim();
                if name.is_empty() {
                    return Err("a name, please".to_string());
                }
                let path = store::normalize(&store::join(&self.pages.files_dir, name))?;
                self.call("files.done", send::<api::files::Mkdir>(PathBody { path }));
            }
            Action::Move(from) => {
                let to = store::normalize(form.value("To").trim())?;
                if to.is_empty() {
                    return Err("a path, please".to_string());
                }
                self.call(
                    "files.done",
                    send::<api::files::Move>(MoveBody {
                        from: from.clone(),
                        to,
                    }),
                );
            }
            Action::Delete(paths) => self.call(
                "files.done",
                send::<api::files::Delete>(DeleteBody {
                    paths: paths.clone(),
                    recursive: true,
                }),
            ),
            Action::UpdateSend(image) => {
                let bmap = PathBuf::from(form.value("Block map").trim());
                if !bmap.is_file() {
                    return Err(format!("{}: no such block map", bmap.display()));
                }
                let repartition = form.checked("Rewrite the whole disk");
                let wipe_data = form.checked("Erase /data") || repartition;
                let plan = tessaro_client::update::Plan {
                    image: image.clone(),
                    bmap: bmap.clone(),
                    wipe_data,
                    repartition,
                    verify: !form.checked("Skip the checksum check"),
                    reboot: form.checked("Reboot to apply"),
                };
                let warning = plan.warning(&plan.name()?);
                if let (Some(warning), false) = (warning, form.typed) {
                    // Destructive after all: ask again, with the name.
                    let mut again = Form::new(form.title.clone(), form.ok, form.action.clone())
                        .intro(format!("This will {warning}."));
                    again.fields = form
                        .fields
                        .iter()
                        .map(|field| Field {
                            label: field.label,
                            value: field.value.clone(),
                            kind: field.kind,
                            editor: field
                                .editor
                                .as_ref()
                                .map(|_| text_editor::Content::with_text(&field.value)),
                            mono: field.mono,
                            tall: field.tall,
                        })
                        .collect();
                    self.form(again.typed());
                    return Ok(());
                }
                let label = format!("update {}", plan.name()?);
                self.start_job("update", label, jobs::Kind::Update(plan));
            }
            Action::UpdateCancel => self.call("update.done", send::<api::update::Cancel>(())),
        }
        Ok(())
    }

    /// The form's view.
    pub(super) fn form_view<'a>(&'a self, form: &'a Form) -> Element<'a, Message> {
        let mut body = column![].spacing(8);
        if let Some(intro) = &form.intro {
            let intro = text(intro).size(theme::SMALL);
            body = body.push(if form.typed {
                intro.style(text::warning)
            } else {
                intro
            });
        }
        for (at, item) in form.fields.iter().enumerate() {
            let font = if item.mono {
                iced::Font::MONOSPACE
            } else {
                theme::FONT
            };
            let input: Element<'a, Message> = match item.kind {
                FieldKind::Text(placeholder) => text_input(placeholder, &item.value)
                    .on_input(move |value| Message::P(Msg::FormText(at, value)))
                    .on_submit(Message::P(Msg::FormOk))
                    .size(theme::SMALL)
                    .font(font)
                    .into(),
                FieldKind::Multiline(placeholder) => match &item.editor {
                    Some(editor) => text_editor(editor)
                        .placeholder(placeholder)
                        .on_action(move |action| Message::P(Msg::FormEdit(at, action)))
                        .height(Length::Fixed(if item.tall { 360.0 } else { 96.0 }))
                        .size(theme::SMALL)
                        .font(font)
                        .into(),
                    None => space().into(),
                },
                FieldKind::Secret => text_input("", &item.value)
                    .on_input(move |value| Message::P(Msg::FormText(at, value)))
                    .on_submit(Message::P(Msg::FormOk))
                    .secure(true)
                    .size(theme::SMALL)
                    .into(),
                FieldKind::Check => checkbox(item.on())
                    .on_toggle(move |on| Message::P(Msg::FormCheck(at, on)))
                    .size(14)
                    .into(),
                FieldKind::Choice(choices) => pick_list(
                    choices,
                    choices.iter().find(|choice| **choice == item.value),
                    move |choice: &'static str| Message::P(Msg::FormChoice(at, choice)),
                )
                .text_size(theme::SMALL)
                .padding([2, 6])
                .into(),
            };
            body = body.push(field(item.label, input));
        }
        if let Some(note) = &form.note {
            body = body.push(text(note).size(theme::SMALL).style(theme::muted));
        }
        body = body.push(dialog::error(form.error.clone()));
        dialog::frame_sized(
            form.title.clone(),
            body.into(),
            vec![
                theme::default_button(form.ok, Some(Message::P(Msg::FormOk))),
                theme::dialog_button("Cancel", Some(Message::Cancel)),
            ],
            if form.wide { 720.0 } else { 480.0 },
        )
    }

    /// The page's view.
    pub(super) fn page_view(&self) -> Element<'_, Message> {
        let content = match self.page {
            Page::Overview => self.overview_view(),
            Page::Browser => self.browser_view(),
            Page::Network => self.network_view(),
            Page::Wifi => self.wifi_view(),
            Page::Certs => self.certs_view(),
            Page::Policies => self.policies_view(),
            Page::Storage => self.storage_view(),
            Page::Audio => self.audio_view(),
            Page::Time => self.time_view(),
            Page::Schedules => self.schedules_view(),
            Page::Access => self.access_view(),
            Page::Ssh => self.ssh_view(),
            Page::Files => self.files_view(),
            Page::Update => self.update_view(),
            Page::Screen | Page::Log => space().into(),
        };
        let key = page_key(self.page);
        let mut page = column![content].spacing(6).height(Length::Fill);
        if let Some(error) = self.pages.errors.get(key) {
            page = page.push(text(error.clone()).size(theme::SMALL).style(text::danger));
        }
        page.into()
    }

    /// The running jobs of `owner`, with progress and Cancel.
    fn jobs_view(&self, owner: &'static str) -> Option<Element<'_, Message>> {
        let jobs: Vec<Element<'_, Message>> = self
            .jobs
            .iter()
            .filter(|job| job.owner == owner && job.running)
            .map(|job| {
                let (label, done, total) = job
                    .progress
                    .clone()
                    .unwrap_or_else(|| (Line::of(Tone::Muted, "starting"), 0, 1));
                row![
                    text(&job.label).size(theme::SMALL).font(bold()).width(180),
                    progress_bar(0.0..=total.max(1) as f32, done as f32)
                        .length(Length::Fill)
                        .girth(10),
                    theme::text_line(&label, iced::Font::MONOSPACE),
                    theme::tool("Cancel", Some(Message::P(Msg::CancelJob(job.id)))),
                ]
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center)
                .into()
            })
            .collect();
        (!jobs.is_empty()).then(|| Column::with_children(jobs).spacing(4).into())
    }

    /// The page's stream output, when there is some.
    fn output_view(&self, owner: &'static str) -> Option<Element<'_, Message>> {
        let lines = self
            .pages
            .output
            .get(owner)
            .filter(|lines| !lines.is_empty())?;
        let body = Column::with_children(
            lines
                .iter()
                .map(|line| theme::text_line(line, iced::Font::MONOSPACE)),
        );
        Some(
            container(column![
                row![
                    text("Output").size(theme::SMALL).font(bold()),
                    space::horizontal(),
                    theme::tool("Clear", Some(Message::P(Msg::ClearOutput))),
                ]
                .align_y(iced::alignment::Vertical::Center),
                scrollable(body)
                    .anchor_bottom()
                    .height(140)
                    .width(Length::Fill),
            ])
            .padding([4, 6])
            .style(theme::panel)
            .into(),
        )
    }

    /// A page: its toolbar, its tables, its jobs and its output.
    fn page<'a>(
        &'a self,
        owner: &'static str,
        list: Vec<section::Action<Message>>,
        record: Vec<section::Action<Message>>,
        body: Vec<Element<'a, Message>>,
    ) -> Element<'a, Message> {
        // Configure comes first: the page's settings, in their own window.
        // One flat row, so a narrow page (the VNC panel open) wraps the
        // buttons onto a second line instead of squeezing them.
        let mut toolbar = row(self
            .configure()
            .map(|configure| section::action("Configure", Some(configure)))
            .into_iter()
            .chain(list)
            .map(|action| theme::tool(action.label, action.message)))
        .spacing(4);
        if !record.is_empty() {
            toolbar = toolbar.push(container(rule::vertical(1)).height(20));
            for action in record {
                toolbar = toolbar.push(theme::tool(action.label, action.message));
            }
        }
        let mut page = column![toolbar
            .align_y(iced::alignment::Vertical::Center)
            .wrap()
            .vertical_spacing(4)]
        .spacing(6)
        .height(Length::Fill);
        for part in body {
            page = page.push(part);
        }
        if let Some(jobs) = self.jobs_view(owner) {
            page = page.push(jobs);
        }
        if let Some(output) = self.output_view(owner) {
            page = page.push(output);
        }
        page.into()
    }

    /// A small table: fixed height, rows keyed for selection.
    fn table<'a>(
        &self,
        name: &'static str,
        columns: &[Col],
        rows: Vec<(String, Vec<Cell<'a, Message>>)>,
        height: Length,
    ) -> Element<'a, Message> {
        let selected = self.selected(name);
        let at = rows.iter().position(|(key, _)| Some(key) == selected);
        let keys: Vec<String> = rows.iter().map(|(key, _)| key.clone()).collect();
        let keys_too = keys.clone();
        container(grid(
            self.tables.state(name),
            move |event| Message::Table(name.into(), event),
            columns,
            rows.into_iter().map(|(_, cells)| cells).collect(),
            at,
            move |at| Message::P(Msg::Select(name, keys[at].clone())),
            move |at| Message::P(Msg::Activate(name, keys_too[at].clone())),
        ))
        .height(height)
        .into()
    }

    fn online(&self) -> bool {
        self.link == Link::Online
    }

    fn when(&self, message: Msg) -> Option<Message> {
        self.online().then_some(Message::P(message))
    }

    /// From the last status poll, so a claim or unclaim made elsewhere
    /// shows; the handshake's answer until the first poll.
    fn claimed(&self) -> bool {
        match (&self.status, &self.info) {
            (Some((status, _)), _) => status.node.claimed,
            (None, Some(info)) => info.claimed,
            (None, None) => false,
        }
    }

    /// The DevTools tunnel is up already; one is all Chrome needs.
    fn devtools_open(&self) -> bool {
        self.jobs
            .iter()
            .any(|job| job.running && matches!(job.kind, jobs::Kind::DevTools))
    }

    /// A two-column table of facts.
    fn facts<'a>(
        &self,
        name: &'static str,
        facts: Vec<(&'static str, String)>,
    ) -> Element<'a, Message> {
        const COLUMNS: &[Col] = &[col("", Length::Fixed(150.0)), col("", Length::Fill)];
        let rows = facts
            .into_iter()
            .map(|(label, value)| {
                (
                    label.to_string(),
                    vec![cell(label).style(theme::muted).into(), cell(value).into()],
                )
            })
            .collect();
        self.table(name, COLUMNS, rows, Length::Shrink)
    }

    /// The shared facts (`tessaro_client::describe`) as the same grid:
    /// the label capitalized, the value in its loudest tone.
    fn shared_facts<'a>(&self, name: &'static str, facts: Vec<Fact>) -> Element<'a, Message> {
        const COLUMNS: &[Col] = &[col("", Length::Fixed(150.0)), col("", Length::Fill)];
        let rows = facts
            .into_iter()
            .enumerate()
            .map(|(at, fact)| {
                let mut label = fact.label.clone();
                if let Some(first) = label.get_mut(..1) {
                    first.make_ascii_uppercase();
                }
                let key = if fact.label.is_empty() {
                    format!("#{at}")
                } else {
                    fact.label
                };
                (
                    key,
                    vec![
                        cell(label).style(theme::muted).into(),
                        cell(fact.value.to_string())
                            .style(theme::toned(fact.value.tone()))
                            .into(),
                    ],
                )
            })
            .collect();
        self.table(name, COLUMNS, rows, Length::Shrink)
    }

    fn overview_view(&self) -> Element<'_, Message> {
        // What `tessaro-ctl device status` says, the units in a table of
        // their own; before the first status, who the device is.
        let mut facts = Vec::new();
        let mut units = Vec::new();
        let mut pending = None;
        if let Some((status, _)) = &self.status {
            let described = describe::device::status(status);
            facts = described.facts;
            facts.extend(described.more);
            pending = described.pending;
            units = described
                .units
                .into_iter()
                .map(|unit| {
                    let state = cell(unit.value.to_string())
                        .style(theme::toned(unit.value.tone()))
                        .into();
                    (unit.label.clone(), vec![cell(unit.label).into(), state])
                })
                .collect();
        } else if let Some(info) = &self.info {
            facts = describe::device::node(info);
        }
        const UNITS: &[Col] = &[
            col("Unit", Length::Fixed(260.0)),
            col("State", Length::Fill),
        ];
        self.page(
            "overview",
            vec![
                action("Ping", self.when(Msg::ControlPing)),
                action("Factory reset", self.when(Msg::FactoryReset)),
            ],
            Vec::new(),
            pending
                .map(|pending| theme::text_line(&pending, theme::FONT))
                .into_iter()
                .chain([
                    self.shared_facts("facts", facts),
                    self.table("units", UNITS, units, Length::Fill),
                ])
                .collect(),
        )
    }

    /// What the browser shows, and the `browser` commands.
    fn browser_view(&self) -> Element<'_, Message> {
        let yes = |on: bool| if on { "yes" } else { "no" }.to_string();
        let mut facts = Vec::new();
        if let Some((status, _)) = &self.status {
            facts.push(("Kiosk page", status.kiosk_url.clone()));
            facts.push(("Showing", status.current_url.clone().unwrap_or_default()));
            facts.push(("Browser answers", yes(status.browser_answering)));
            facts.push(("Maintenance", yes(status.maintenance)));
            facts.push(("Debug screen", yes(status.debug_screen)));
            facts.push(("Page zoom", format!("{}%", self.zoom())));
            facts.push((
                "DevTools",
                if status.devtools {
                    "connected - the agent leaves the tab alone".to_string()
                } else {
                    "not connected".to_string()
                },
            ));
            if let Some(bridge) = &status.bridge {
                facts.push(("Page bridge", bridge.mode.clone()));
                let script = match (&bridge.script_problem, bridge.script.is_empty()) {
                    (_, true) => "none".to_string(),
                    (Some(problem), false) => {
                        format!("{} (not injected: {problem})", bridge.script)
                    }
                    (None, false) => bridge.script.clone(),
                };
                facts.push(("Injected script", script));
            }
        }
        let maintenance = self
            .status
            .as_ref()
            .is_some_and(|(status, _)| status.maintenance);
        let debug = self
            .status
            .as_ref()
            .is_some_and(|(status, _)| status.debug_screen);
        self.page(
            "browser",
            vec![
                action("Navigate", self.when(Msg::Navigate)),
                action(
                    if maintenance {
                        "Maintenance off"
                    } else {
                        "Maintenance on"
                    },
                    self.when(Msg::Maintenance(!maintenance)),
                ),
                action(
                    if debug {
                        "Debug screen off"
                    } else {
                        "Debug screen on"
                    },
                    self.when(Msg::DebugScreen(!debug)),
                ),
                action("Zoom", self.when(Msg::Zoom)),
                action(
                    "DevTools",
                    self.when(Msg::DevTools).filter(|_| !self.devtools_open()),
                ),
                action("Reload", self.when(Msg::Reload)),
                action("Clear cache", self.when(Msg::ClearCache)),
                action("Inject", self.when(Msg::Inject)),
                action("Bridge", self.when(Msg::Bridge)),
                action("Run JavaScript", self.when(Msg::Eval)),
            ],
            Vec::new(),
            vec![self.facts("browserfacts", facts)],
        )
    }

    /// The display modes, under the screenshot on the Screen page.
    pub(super) fn modes_view(&self) -> Element<'_, Message> {
        const COLUMNS: &[Col] = &[
            col("Output", Length::Fixed(140.0)),
            col("Mode", Length::Fixed(160.0)),
            col("", Length::Fill),
        ];
        let current = self
            .setting("screen.resolution")
            .and_then(|setting| setting.value.clone())
            .unwrap_or_default();
        let rows = self
            .pages
            .modes
            .iter()
            .flat_map(|connector| {
                connector.modes.iter().enumerate().map(|(at, mode)| {
                    let note = match (at == 0, *mode == current) {
                        (_, true) => "set",
                        (true, false) => "preferred",
                        _ => "",
                    };
                    (
                        format!("{} {mode}", connector.name),
                        vec![
                            cell(connector.name.clone()).into(),
                            cell(mode.clone()).into(),
                            cell(note).style(theme::muted).into(),
                        ],
                    )
                })
            })
            .collect();
        let screen_off = self
            .status
            .as_ref()
            .is_some_and(|(status, _)| status.screen_on == Some(false));
        let mut toolbar = row![
            theme::tool("Configure", self.configure()),
            theme::tool(
                "Use this mode",
                self.selected("modes").and_then(|_| self.when(Msg::UseMode))
            ),
            theme::tool(
                if screen_off {
                    "Screen on"
                } else {
                    "Screen off"
                },
                self.when(Msg::ScreenPower(screen_off)),
            ),
            theme::tool("Show keyboard", self.when(Msg::Keyboard(true))),
            theme::tool("Hide keyboard", self.when(Msg::Keyboard(false))),
        ]
        .spacing(4)
        .align_y(iced::alignment::Vertical::Center);
        // A guarded change reverts on its own: the countdown is on the
        // button that keeps it.
        if let Some((status, at)) = &self.status {
            if let Some(pending) = &status.pending {
                let left = pending.seconds_left.saturating_sub(at.elapsed().as_secs());
                toolbar = toolbar.push(theme::confirm_tool(
                    format!("Confirm ({left}s)"),
                    Message::ConfirmPending,
                ));
            }
        }
        column![
            toolbar,
            self.table("modes", COLUMNS, rows, Length::Fixed(TABLE_HEIGHT)),
        ]
        .spacing(4)
        .into()
    }

    fn network_view(&self) -> Element<'_, Message> {
        let proxy_on = self
            .pages
            .net
            .as_ref()
            .is_some_and(|net| net.proxy.is_some());
        let mut facts = Vec::new();
        let mut interfaces = Vec::new();
        if let Some(net) = &self.pages.net {
            facts.push(("Hostname", net.hostname.clone()));
            facts.push(("Default route", net.interface.clone().unwrap_or_default()));
            facts.push(("Gateway", net.gateway.clone().unwrap_or_default()));
            facts.push(("DNS", net.dns.join(", ")));
            facts.push(("Public IP", net.public_ip.clone().unwrap_or_default()));
            facts.push((
                "Proxy",
                net.proxy.clone().unwrap_or_else(|| "none".to_string()),
            ));
            interfaces = net
                .interfaces
                .iter()
                .map(|interface| {
                    let addresses = interface
                        .addresses
                        .iter()
                        .map(|address| format!("{}/{}", address.address, address.prefix))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let state: Cell<'_, Message> = cell(interface.state.clone())
                        .style(theme::toned(tessaro_client::text::link_state(
                            &interface.state,
                        )))
                        .into();
                    (
                        interface.name.clone(),
                        vec![
                            cell(interface.name.clone()).into(),
                            cell(interface.kind.clone()).into(),
                            state,
                            cell(match interface.carrier {
                                Some(true) => "yes",
                                Some(false) => "no",
                                None => "",
                            })
                            .into(),
                            cell(
                                interface
                                    .speed_mbps
                                    .map(|speed| format!("{speed} Mb/s"))
                                    .unwrap_or_default(),
                            )
                            .into(),
                            cell(interface.mac.clone().unwrap_or_default())
                                .style(theme::muted)
                                .into(),
                            cell(if interface.default_route {
                                "default"
                            } else {
                                ""
                            })
                            .into(),
                            cell(addresses).into(),
                        ],
                    )
                })
                .collect();
        }
        const INTERFACES: &[Col] = &[
            col("Interface", Length::Fixed(90.0)),
            col("Kind", Length::Fixed(70.0)),
            col("State", Length::Fixed(60.0)),
            col("Carrier", Length::Fixed(55.0)),
            col("Speed", Length::Fixed(80.0)),
            col("MAC", Length::Fixed(130.0)),
            col("Route", Length::Fixed(60.0)),
            col("Addresses", Length::Fill),
        ];
        const PROFILES: &[Col] = &[
            col("Profile", Length::Fixed(160.0)),
            col("Kind", Length::Fixed(90.0)),
            col("Device", Length::Fixed(80.0)),
            col("Active", Length::Fixed(55.0)),
            col("Auto", Length::Fixed(45.0)),
            col("Priority", Length::Fixed(60.0)),
            col("Managed", Length::Fill),
        ];
        let profiles = self
            .pages
            .profiles
            .iter()
            .map(|profile| {
                (
                    profile.name.clone(),
                    vec![
                        cell(profile.name.clone()).into(),
                        cell(profile.kind.clone()).into(),
                        cell(profile.device.clone().unwrap_or_default()).into(),
                        if profile.active {
                            cell("yes").style(text::success).into()
                        } else {
                            cell("").into()
                        },
                        cell(if profile.autoconnect { "yes" } else { "no" }).into(),
                        cell(profile.priority.to_string()).into(),
                        cell(if profile.managed { "by tessaro" } else { "" })
                            .style(theme::muted)
                            .into(),
                    ],
                )
            })
            .collect();
        self.page(
            "net",
            vec![
                action("Last change", self.when(Msg::NetLast)),
                action("Ping ...", self.when(Msg::NetPing)),
                action("Speed test ...", self.when(Msg::Speedtest)),
                action("Proxy ...", self.when(Msg::Proxy)),
                action(
                    "Proxy off",
                    proxy_on
                        .then_some(())
                        .and_then(|()| self.when(Msg::ProxyOff)),
                ),
                action(
                    "Test proxy",
                    proxy_on
                        .then_some(())
                        .and_then(|()| self.when(Msg::ProxyTest)),
                ),
            ],
            vec![],
            vec![
                self.facts("netfacts", facts),
                self.table(
                    "interfaces",
                    INTERFACES,
                    interfaces,
                    Length::Fixed(TABLE_HEIGHT),
                ),
                self.table("profiles", PROFILES, profiles, Length::Fill),
            ],
        )
    }

    fn certs_view(&self) -> Element<'_, Message> {
        const CERTS: &[Col] = &[
            col("Certificate authority", Length::Fixed(260.0)),
            col("Expires (UTC)", Length::Fixed(110.0)),
            col("Issued by", Length::Fixed(200.0)),
            col("Fingerprint", Length::Fill),
        ];
        let certs = self
            .pages
            .certs
            .iter()
            .map(|cert| {
                let date = tessaro_client::certs::date(cert.not_after);
                let expires: Cell<'_, Message> = if tessaro_client::certs::expired(cert.not_after) {
                    cell(format!("{date} expired")).style(text::danger).into()
                } else {
                    cell(date).into()
                };
                (
                    cert.fingerprint.clone(),
                    vec![
                        cell(cert.subject.clone()).into(),
                        expires,
                        cell(if cert.self_signed {
                            "itself".to_string()
                        } else {
                            cert.issuer.clone()
                        })
                        .style(theme::muted)
                        .into(),
                        cell(cert.fingerprint.clone()).style(theme::muted).into(),
                    ],
                )
            })
            .collect();
        self.page(
            "certs",
            vec![action("Trust a CA ...", self.when(Msg::CertPick))],
            vec![action(
                "Revoke CA",
                self.selected("certs")
                    .and_then(|_| self.when(Msg::CertRevoke)),
            )],
            vec![self.table("certs", CERTS, certs, Length::Fill)],
        )
    }

    fn policies_view(&self) -> Element<'_, Message> {
        const POLICIES: &[Col] = &[
            col("Policy", Length::Fixed(180.0)),
            col("Sets", Length::Fill),
        ];
        let policies = self
            .pages
            .policies
            .iter()
            .map(|info| {
                let sets: Cell<'_, Message> = match &info.problem {
                    Some(problem) => cell(format!("left out: {problem}"))
                        .style(text::danger)
                        .into(),
                    None if info.keys.is_empty() => {
                        cell("(sets nothing)").style(theme::muted).into()
                    }
                    None => cell(info.keys.join(", ")).into(),
                };
                (
                    info.name.clone(),
                    vec![cell(info.name.clone()).into(), sets],
                )
            })
            .collect();
        self.page(
            "policies",
            vec![
                action("New policy ...", self.when(Msg::PolicyNew)),
                action("Open a file ...", self.when(Msg::PolicyPick)),
                action("Effective policy", self.when(Msg::PolicyEffective)),
            ],
            vec![
                action(
                    "Edit policy ...",
                    self.selected("policies")
                        .and_then(|_| self.when(Msg::PolicyEdit)),
                ),
                action(
                    "Remove policy",
                    self.selected("policies")
                        .and_then(|_| self.when(Msg::PolicyRemove)),
                ),
            ],
            vec![self.table("policies", POLICIES, policies, Length::Fill)],
        )
    }

    fn schedules_view(&self) -> Element<'_, Message> {
        const SCHEDULES: &[Col] = &[
            col("Schedule", Length::Fixed(160.0)),
            col("State", Length::Fixed(50.0)),
            col("Calendar", Length::Fixed(200.0)),
            col("Next run", Length::Fixed(190.0)),
            col("Last run", Length::Fill),
        ];
        let now = tessaro_client::schedule::now();
        let schedules = self
            .pages
            .schedules
            .iter()
            .map(|info| {
                let state: Cell<'_, Message> = if info.spec.enabled {
                    cell("on").style(text::success).into()
                } else {
                    cell("off").style(text::danger).into()
                };
                let next = info.next.as_ref().map_or_else(
                    || "-".to_string(),
                    |next| tessaro_client::schedule::moment(next, now).to_string(),
                );
                let last = tessaro_client::schedule::last_run(info, now);
                let last: Cell<'_, Message> = cell(last.to_string())
                    .style(theme::toned(last.tone()))
                    .into();
                (
                    info.id.clone(),
                    vec![
                        cell(info.spec.name.clone()).into(),
                        state,
                        cell(info.spec.calendar.join("  |  ")).into(),
                        cell(next).into(),
                        last,
                    ],
                )
            })
            .collect();
        let chosen = self.selected_schedule();
        let toggle = if chosen.as_ref().is_some_and(|info| info.spec.enabled) {
            "Disable"
        } else {
            "Enable"
        };
        let with_one = |message: Msg| chosen.as_ref().and_then(|_| self.when(message));
        self.page(
            "schedules",
            vec![action("New schedule ...", self.when(Msg::ScheduleNew))],
            vec![
                action("Edit ...", with_one(Msg::ScheduleEdit)),
                action(toggle, with_one(Msg::ScheduleToggle)),
                action("Run now", with_one(Msg::ScheduleRun)),
                action("Logs", with_one(Msg::ScheduleLogs)),
                action("Remove ...", with_one(Msg::ScheduleRemove)),
            ],
            vec![self.table("schedules", SCHEDULES, schedules, Length::Fill)],
        )
    }

    fn wifi_view(&self) -> Element<'_, Message> {
        let mut facts = Vec::new();
        if let Some(wifi) = &self.pages.wifi {
            facts.push((
                "WiFi",
                match (wifi.enabled, wifi.hardware_enabled) {
                    (_, false) => "off by a hardware switch".to_string(),
                    (true, true) => "on".to_string(),
                    (false, true) => "off".to_string(),
                },
            ));
            for device in &wifi.devices {
                facts.push((
                    "Interface",
                    format!(
                        "{}: {}{}{}",
                        device.interface,
                        device.state,
                        device
                            .ssid
                            .as_ref()
                            .map(|ssid| format!(", on {ssid}"))
                            .unwrap_or_default(),
                        device
                            .signal
                            .map(|signal| format!(", signal {signal}%"))
                            .unwrap_or_default()
                    ),
                ));
            }
            if let Some(fallback) = &wifi.fallback {
                facts.push(("Fallback", fallback.clone()));
            }
        }
        const COLUMNS: &[Col] = &[
            col("SSID", Length::Fixed(200.0)),
            col("Signal", Length::Fixed(60.0)),
            col("Security", Length::Fixed(90.0)),
            col("Band", Length::Fixed(80.0)),
            col("Interface", Length::Fixed(80.0)),
            col("BSSID", Length::Fixed(140.0)),
            col("", Length::Fill),
        ];
        let rows = self
            .pages
            .networks
            .iter()
            .map(|network| {
                let state = match (network.active, network.known) {
                    (true, _) => "connected",
                    (false, true) => "known",
                    _ => "",
                };
                (
                    network.bssid.clone(),
                    vec![
                        cell(if network.ssid.is_empty() {
                            "(hidden)".to_string()
                        } else {
                            network.ssid.clone()
                        })
                        .into(),
                        cell(format!("{}%", network.signal))
                            .style(theme::toned(describe::net::signal_tone(network.signal)))
                            .into(),
                        cell(network.security.clone()).into(),
                        cell(describe::net::band(network.frequency_mhz)).into(),
                        cell(network.interface.clone()).into(),
                        cell(network.bssid.clone()).style(theme::muted).into(),
                        cell(state).style(text::success).into(),
                    ],
                )
            })
            .collect();
        self.page(
            "wifi",
            vec![
                if self.waiting("wifi.scan") {
                    action("Scanning ...", None)
                } else {
                    action("Scan", self.when(Msg::WifiScan))
                },
                action("Join ...", self.when(Msg::WifiJoin)),
                action("Hotspot password", self.when(Msg::Hotspot)),
            ],
            Vec::new(),
            vec![
                self.facts("wififacts", facts),
                self.table("wifi", COLUMNS, rows, Length::Fill),
            ],
        )
    }

    fn storage_view(&self) -> Element<'_, Message> {
        let mut facts = Vec::new();
        let mut partitions = Vec::new();
        let mut filesystems = Vec::new();
        if let Some(storage) = &self.pages.storage {
            facts.push((
                "Disk",
                format!(
                    "{} {}{}",
                    storage.device,
                    size_label(storage.size),
                    storage
                        .model
                        .as_ref()
                        .map(|model| format!(", {model}"))
                        .unwrap_or_default()
                ),
            ));
            facts.push(("Table", storage.table.clone()));
            facts.push(("Not partitioned", size_label(storage.unallocated)));
            partitions = storage
                .partitions
                .iter()
                .map(|partition| {
                    (
                        partition.name.clone(),
                        vec![
                            cell(partition.number.to_string()).into(),
                            cell(partition.name.clone()).into(),
                            cell(partition.label.clone().unwrap_or_default()).into(),
                            cell(partition.fstype.clone().unwrap_or_default()).into(),
                            cell(size_label(partition.start))
                                .sort_number(partition.start as f64)
                                .style(theme::muted)
                                .into(),
                            cell(size_label(partition.size))
                                .sort_number(partition.size as f64)
                                .into(),
                            cell(partition.mountpoint.clone().unwrap_or_default()).into(),
                        ],
                    )
                })
                .collect();
            filesystems = storage
                .filesystems
                .iter()
                .map(|fs| {
                    let percent = fs.used_percent();
                    let used: Cell<'_, Message> = cell(format!("{percent}%"))
                        .style(theme::toned(tessaro_client::text::usage_level(percent)))
                        .into();
                    (
                        fs.mountpoint.clone(),
                        vec![
                            cell(fs.mountpoint.clone()).into(),
                            cell(fs.source.clone()).style(theme::muted).into(),
                            cell(fs.fstype.clone()).into(),
                            cell(size_label(fs.size)).sort_number(fs.size as f64).into(),
                            cell(size_label(fs.used)).sort_number(fs.used as f64).into(),
                            cell(size_label(fs.available))
                                .sort_number(fs.available as f64)
                                .into(),
                            used,
                        ],
                    )
                })
                .collect();
        }
        let growable = self
            .pages
            .storage
            .as_ref()
            .is_some_and(|storage| storage.unallocated >= protocol::GROW_MIN);
        const PARTITIONS: &[Col] = &[
            col("#", Length::Fixed(30.0)),
            col("Partition", Length::Fixed(140.0)),
            col("Label", Length::Fixed(90.0)),
            col("Filesystem", Length::Fixed(80.0)),
            col("Start", Length::Fixed(80.0)),
            col("Size", Length::Fixed(80.0)),
            col("Mounted", Length::Fill),
        ];
        const FILESYSTEMS: &[Col] = &[
            col("Mounted", Length::Fixed(120.0)),
            col("Source", Length::Fixed(160.0)),
            col("Type", Length::Fixed(70.0)),
            col("Size", Length::Fixed(80.0)),
            col("Used", Length::Fixed(80.0)),
            col("Free", Length::Fixed(80.0)),
            col("Use", Length::Fill),
        ];
        self.page(
            "storage",
            vec![
                action("Check growing /data", self.when(Msg::GrowCheck)),
                action(
                    "Grow /data ...",
                    growable.then_some(()).and_then(|()| self.when(Msg::Grow)),
                ),
            ],
            Vec::new(),
            vec![
                self.facts("storagefacts", facts),
                self.table(
                    "partitions",
                    PARTITIONS,
                    partitions,
                    Length::Fixed(TABLE_HEIGHT),
                ),
                self.table("filesystems", FILESYSTEMS, filesystems, Length::Fill),
            ],
        )
    }

    fn time_view(&self) -> Element<'_, Message> {
        let ntp_on = self
            .pages
            .time
            .as_ref()
            .and_then(|time| time.ntp)
            .unwrap_or(true);
        let actions = vec![
            action("Timezone ...", self.when(Msg::Timezone)),
            action("NTP ...", self.when(Msg::Ntp)),
            action(
                "Sync now",
                ntp_on.then_some(()).and_then(|()| self.when(Msg::TimeSync)),
            ),
            action(
                "Set the clock ...",
                (!ntp_on)
                    .then_some(())
                    .and_then(|()| self.when(Msg::SetClock)),
            ),
        ];
        let Some(time) = &self.pages.time else {
            return self.page("time", actions, Vec::new(), Vec::new());
        };
        let mut body = Vec::new();
        if let Some(error) = describe::time::error(time) {
            body.push(theme::text_line(&error, theme::FONT));
        }
        body.push(self.shared_facts("timefacts", describe::time::facts(time)));
        body.push(text("Servers").size(theme::SMALL).font(bold()).into());
        body.push(self.shared_facts("timeservers", describe::time::servers(time)));
        if let Some(hint) = describe::time::hint(time) {
            body.push(theme::text_line(&hint, theme::FONT));
        }
        self.page("time", actions, Vec::new(), body)
    }

    fn audio_view(&self) -> Element<'_, Message> {
        let Some(audio) = &self.pages.audio else {
            return self.page("audio", Vec::new(), Vec::new(), Vec::new());
        };
        const DEVICES: &[Col] = &[
            col("Name", Length::Fixed(110.0)),
            col("Device", Length::Fixed(260.0)),
            col("Kind", Length::Fixed(80.0)),
            col("Plugged", Length::Fixed(70.0)),
            col("", Length::Fill),
        ];
        let devices = |devices: &[AudioDevice]| -> Vec<(String, Vec<Cell<'_, Message>>)> {
            devices
                .iter()
                .map(|device| {
                    (
                        device.name.clone(),
                        vec![
                            cell(device.name.clone()).into(),
                            cell(device.description.clone()).into(),
                            cell(device.kind.clone()).into(),
                            cell(match device.available {
                                Some(true) => "yes",
                                Some(false) => "no",
                                None => "",
                            })
                            .into(),
                            if device.in_use {
                                cell("in use").style(text::success).into()
                            } else {
                                cell("").into()
                            },
                        ],
                    )
                })
                .collect()
        };
        let side = |label: &'static str,
                    side: &protocol::AudioSide,
                    volume: u8,
                    on_volume: fn(u8) -> Msg,
                    on_release: Msg,
                    muted_message: Msg|
         -> Element<'_, Message> {
            let using = side
                .using
                .as_ref()
                .map_or("nothing".to_string(), |device| device.description.clone());
            let mut line = row![
                text(label).size(theme::SMALL).font(bold()).width(60),
                text(format!("{} -> {using}", side.setting)).size(theme::SMALL),
                space::horizontal(),
                text(format!("{volume}%")).size(theme::SMALL).width(40),
                slider(0..=100u8, volume, move |volume| Message::P(on_volume(
                    volume
                )))
                .on_release(Message::P(on_release))
                .width(180),
                checkbox(side.muted)
                    .label("muted")
                    .on_toggle(move |_| Message::P(muted_message.clone()))
                    .text_size(theme::SMALL)
                    .size(14),
            ]
            .spacing(8)
            .align_y(iced::alignment::Vertical::Center);
            if let Some(fallback) = &side.fallback {
                line = line.push(
                    text(fallback.clone())
                        .size(theme::SMALL)
                        .style(text::warning),
                );
            }
            line.into()
        };
        let mut body = Vec::new();
        if let Some(error) = &audio.error {
            body.push(
                text(error.clone())
                    .size(theme::SMALL)
                    .style(text::danger)
                    .into(),
            );
        }
        body.push(side(
            "Output",
            &audio.output,
            self.pages.volume.unwrap_or(audio.output.volume),
            Msg::Volume,
            Msg::VolumeDone,
            Msg::Mute(!audio.output.muted),
        ));
        body.push(self.table(
            "outputs",
            DEVICES,
            devices(&audio.output.devices),
            Length::Fixed(TABLE_HEIGHT),
        ));
        body.push(side(
            "Input",
            &audio.input,
            self.pages.input_volume.unwrap_or(audio.input.volume),
            Msg::InputVolume,
            Msg::InputVolumeDone,
            Msg::InputMute(!audio.input.muted),
        ));
        body.push(self.table(
            "inputs",
            DEVICES,
            devices(&audio.input.devices),
            Length::Fill,
        ));
        self.page(
            "audio",
            vec![
                action("Test tone", self.when(Msg::Test(false))),
                action("Test recording", self.when(Msg::Test(true))),
            ],
            vec![
                action(
                    "Use output",
                    self.selected("outputs")
                        .and_then(|_| self.when(Msg::UseAudio(keys::AUDIO_OUTPUT))),
                ),
                action(
                    "Use input",
                    self.selected("inputs")
                        .and_then(|_| self.when(Msg::UseAudio(keys::AUDIO_INPUT))),
                ),
            ],
            body,
        )
    }

    fn access_view(&self) -> Element<'_, Message> {
        const COLUMNS: &[Col] = &[
            col("Token", Length::Fixed(120.0)),
            col("Name", Length::Fixed(220.0)),
            col("Issued by", Length::Fill),
        ];
        let rows = self
            .pages
            .tokens
            .iter()
            .map(|token| {
                (
                    token.id.clone(),
                    vec![
                        cell(token.id.clone()).into(),
                        cell(token.name.clone()).into(),
                        cell(token.issued_by.clone()).style(theme::muted).into(),
                    ],
                )
            })
            .collect();
        self.page(
            "access",
            vec![
                action(
                    "Claim ...",
                    self.when(Msg::Claim).filter(|_| !self.claimed()),
                ),
                action("New token ...", self.when(Msg::TokenNew)),
                action("Root password ...", self.when(Msg::Password)),
                // The device answers unclaim even when unclaimed, and the
                // node would then be forgotten here for nothing.
                action(
                    "Unclaim ...",
                    self.when(Msg::Unclaim).filter(|_| self.claimed()),
                ),
                action("Open Webconfig", self.when(Msg::Webconfig)),
            ],
            vec![action(
                "Revoke",
                self.selected("tokens")
                    .and_then(|_| self.when(Msg::TokenRevoke)),
            )],
            vec![self.table("tokens", COLUMNS, rows, Length::Fill)],
        )
    }

    fn ssh_view(&self) -> Element<'_, Message> {
        const COLUMNS: &[Col] = &[
            col("Type", Length::Fixed(110.0)),
            col("Fingerprint", Length::Fixed(380.0)),
            col("Comment", Length::Fill),
        ];
        let rows = self
            .pages
            .ssh_keys
            .iter()
            .map(|key| {
                (
                    key.fingerprint.clone(),
                    vec![
                        cell(key.kind.clone()).into(),
                        cell(key.fingerprint.clone()).style(theme::muted).into(),
                        cell(key.comment.clone()).into(),
                    ],
                )
            })
            .collect();
        self.page(
            "ssh",
            vec![
                action("Open terminal", self.when(Msg::Authorize(true))),
                action("Authorize my key", self.when(Msg::Authorize(false))),
            ],
            vec![action(
                "Revoke",
                self.selected("ssh").and_then(|_| self.when(Msg::KeyRevoke)),
            )],
            vec![self.table("ssh", COLUMNS, rows, Length::Fill)],
        )
    }

    fn files_view(&self) -> Element<'_, Message> {
        const COLUMNS: &[Col] = &[
            col("Name", Length::Fill),
            col("Size", Length::Fixed(90.0)),
            col("Modified (UTC)", Length::Fixed(130.0)),
        ];
        let dir = &self.pages.files_dir;
        let rows = self
            .pages
            .files
            .iter()
            .map(|entry| {
                let name = entry
                    .path
                    .strip_prefix(dir.as_str())
                    .map(|rest| rest.trim_start_matches('/'))
                    .unwrap_or(&entry.path)
                    .to_string();
                let (name, size) = match entry.kind {
                    FileKind::Dir => (format!("{name}/"), String::new()),
                    FileKind::File => (name, size_label(entry.size)),
                };
                (
                    entry.path.clone(),
                    vec![
                        match entry.kind {
                            FileKind::Dir => cell(name).font(bold()).into(),
                            FileKind::File => cell(name).into(),
                        },
                        cell(size).sort_number(entry.size as f64).into(),
                        cell(date(entry.mtime)).style(theme::muted).into(),
                    ],
                )
            })
            .collect();
        let selected = self.selected("files").is_some();
        let path = row![
            text("/files/").size(theme::SMALL).style(theme::muted),
            text(dir.clone()).size(theme::SMALL).font(bold()),
            space::horizontal(),
            text(format!("served at http://127.0.0.1/files/{dir}"))
                .size(theme::SMALL)
                .style(theme::muted),
        ]
        .align_y(iced::alignment::Vertical::Center);
        self.page(
            "files",
            vec![
                action(
                    "Up",
                    (!dir.is_empty())
                        .then_some(())
                        .and_then(|()| self.when(Msg::FilesUp)),
                ),
                action("Upload files ...", self.when(Msg::Upload(false))),
                action("Upload folder ...", self.when(Msg::Upload(true))),
                action("New folder ...", self.when(Msg::Mkdir)),
            ],
            vec![
                action(
                    "Download ...",
                    selected
                        .then_some(())
                        .and_then(|()| self.when(Msg::Download)),
                ),
                action(
                    "Move ...",
                    selected.then_some(()).and_then(|()| self.when(Msg::Move)),
                ),
                action(
                    "Delete ...",
                    selected.then_some(()).and_then(|()| self.when(Msg::Delete)),
                ),
            ],
            vec![
                path.into(),
                self.table("files", COLUMNS, rows, Length::Fill),
            ],
        )
    }

    fn update_view(&self) -> Element<'_, Message> {
        // What `tessaro-ctl update status` says.
        let mut lines = Vec::new();
        let mut cancellable = false;
        if let Some(status) = &self.pages.update {
            cancellable = !matches!(status.phase, UpdatePhase::Idle);
            lines = tessaro_client::update::status_lines(status)
                .iter()
                .map(|line| theme::text_line(line, iced::Font::MONOSPACE))
                .collect();
        }
        let sending = self
            .jobs
            .iter()
            .any(|job| job.owner == "update" && job.running);
        self.page(
            "update",
            vec![
                action(
                    "Send image ...",
                    (!sending)
                        .then_some(())
                        .and_then(|()| self.when(Msg::UpdatePick)),
                ),
                action(
                    "Cancel update ...",
                    cancellable
                        .then_some(())
                        .and_then(|()| self.when(Msg::UpdateCancel)),
                ),
            ],
            Vec::new(),
            vec![Column::with_children(lines).spacing(2).into()],
        )
    }
}

/// The key a page's jobs, output and errors are kept under.
pub(super) fn page_key(page: Page) -> &'static str {
    match page {
        Page::Overview => "overview",
        Page::Browser => "browser",
        Page::Network => "net",
        Page::Wifi => "wifi",
        Page::Certs => "certs",
        Page::Policies => "policies",
        Page::Storage => "storage",
        Page::Audio => "audio",
        Page::Time => "time",
        Page::Schedules => "schedules",
        Page::Access => "access",
        Page::Ssh => "ssh",
        Page::Files => "files",
        Page::Update => "update",
        Page::Screen => "screen",
        Page::Log => "",
    }
}

/// A stream event as a line, the way `tessaro-ctl` prints it; none for a
/// grow's plan, which the job has already shown.
fn stream_line(owner: &str, value: &Value) -> Option<Line> {
    if let Ok(event) = serde_json::from_value::<PingEvent>(value.clone()) {
        return Some(tessaro_client::ping::event_line(&event));
    }
    if owner == "net" {
        if let Ok(event) = serde_json::from_value::<SpeedtestEvent>(value.clone()) {
            return Some(tessaro_client::speedtest::event_line(&event));
        }
    }
    if let Ok(event) = serde_json::from_value::<StorageGrowEvent>(value.clone()) {
        return tessaro_client::storage::event_line(&event);
    }
    Some(Line::plain(value.to_string()))
}

/// What the policy editor says under the text: what it sets, or the
/// device's own check's objection, with its line.
fn policy_note(text: &str) -> String {
    match policy::check(text) {
        Ok(entries) if entries.is_empty() => "sets nothing".to_string(),
        Ok(entries) => format!(
            "sets {}",
            entries.keys().cloned().collect::<Vec<_>>().join(", ")
        ),
        Err(err) => err.to_string(),
    }
}

/// A policy file's text as it is, checked only for size: the editor shows
/// what is wrong with it.
fn read_policy_file(file: &std::path::Path) -> Result<String, String> {
    let size = std::fs::metadata(file)
        .map_err(|err| format!("{}: {err}", file.display()))?
        .len();
    if size > policy::POLICY_TEXT_MAX as u64 {
        return Err(format!(
            "{}: {size} bytes; a policy is at most {}",
            file.display(),
            policy::POLICY_TEXT_MAX
        ));
    }
    std::fs::read_to_string(file).map_err(|err| format!("{}: {err}", file.display()))
}

/// A name for the policy in `file`: its stem, as far as it is one.
fn policy_name_of(file: &std::path::Path) -> String {
    let stem = file
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let name: String = stem
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_start_matches(['-', '_'])
        .chars()
        .take(policy::POLICY_NAME_MAX)
        .collect();
    name
}

/// Shared lines as one text, for a dialog that shows plain text.
fn joined(lines: &[Line]) -> String {
    lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_policy_file_suggests_a_name_the_device_takes() {
        let name = policy_name_of(std::path::Path::new("/tmp/Corp Lockdown.v2.json"));
        assert_eq!(name, "corp-lockdown-v2");
        assert!(policy::check_name(&name).is_ok());
        assert_eq!(policy_name_of(std::path::Path::new("-x.json")), "x");
    }

    #[test]
    fn the_policy_note_says_what_it_sets_or_where_it_is_wrong() {
        assert_eq!(policy_note("{\"B\": 1, \"A\": 2}"), "sets A, B");
        assert_eq!(policy_note("{}"), "sets nothing");
        assert!(policy_note("{\n\"A\": 1\n\"B\": 2}").starts_with("line 3 column 1: "));
    }

    #[test]
    fn a_ping_event_reads_as_a_line() {
        let line = stream_line(
            "net",
            &json!({ "event": "reply", "seq": 2, "bytes": 64, "rtt_ms": 1.25 }),
        );
        assert_eq!(line.unwrap().to_string(), "reply     1.2 ms seq=2 64 bytes");
    }

    #[test]
    fn an_unknown_event_is_shown_as_it_came() {
        assert_eq!(
            stream_line("net", &json!({ "x": 1 })).unwrap().to_string(),
            r#"{"x":1}"#
        );
    }

    #[test]
    fn a_destructive_form_wants_the_name() {
        let form = Form::new("Unclaim", "Unclaim", Action::Unclaim).typed();
        assert!(form.typed);
        assert_eq!(form.value("Device name"), "");
    }
}
