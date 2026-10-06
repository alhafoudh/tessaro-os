//! The bulk window: one action on every device marked in the node list,
//! `tessaro-ctl --tag` / `-n a,b` for the desktop.
//!
//! Each device's run is a job (`jobs.rs`) on a connection of its own, at
//! most `bulk::PARALLEL` at once, the next starting as one ends. A row per
//! device says where it is, and the summary under them is the ctl's
//! (`describe::bulk::summary`). Stop drops the jobs that have not ended,
//! as a device window's Cancel does, and leaves the waiting ones unrun.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use iced::widget::{
    checkbox, column, container, pick_list, row, scrollable, space, text, text_input,
};
use iced::{Element, Length, Task};
use protocol::files as store;
use protocol::RestartTarget;
use tessaro_client::bulk::{self, Outcome};
use tessaro_client::describe::bulk as words;
use tessaro_client::nodes::{Node, Nodes};
use tessaro_client::text::{Line, Tone};
use tessaro_client::update::Plan;

use crate::dialog::{self, field};
use crate::jobs::{self, Kind};
use crate::theme;

/// Job ids of a bulk window, far from a device window's, which count from 1
/// for the same nodes.
const FIRST_JOB: u64 = 1 << 32;

/// What runs on every device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Reload,
    RestartBrowser,
    RestartWeston,
    RestartAgent,
    Reboot,
    Set,
    Script,
    Upload,
    Update,
}

impl Action {
    pub const ALL: &'static [Action] = &[
        Action::Reload,
        Action::RestartBrowser,
        Action::RestartWeston,
        Action::RestartAgent,
        Action::Reboot,
        Action::Set,
        Action::Script,
        Action::Upload,
        Action::Update,
    ];

    /// Whether it is confirmed first, as in a device window.
    fn confirmed(self) -> bool {
        matches!(
            self,
            Action::RestartBrowser
                | Action::RestartWeston
                | Action::RestartAgent
                | Action::Reboot
                | Action::Update
        )
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Action::Reload => "Reload the page",
            Action::RestartBrowser => "Restart the browser",
            Action::RestartWeston => "Restart the display (Weston)",
            Action::RestartAgent => "Restart the agent",
            Action::Reboot => "Reboot",
            Action::Set => "Set a setting",
            Action::Script => "Run a script",
            Action::Upload => "Upload files",
            Action::Update => "Update the image",
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
enum State {
    /// Not asked to run yet.
    Idle,
    Waiting,
    Running {
        job: u64,
        /// The last progress or line it reported.
        last: Option<Line>,
    },
    Done(Result<String, String>),
}

struct Device {
    node: Node,
    state: State,
}

/// What was confirmed: the job each device runs.
struct Confirm {
    kind: Kind,
    /// Something is lost for good: the device count typed out, the way a
    /// device window has its name typed.
    warning: Option<String>,
    typed: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    Action(Action),
    Key(String),
    Value(String),
    Script(String),
    Into(String),
    PickFiles,
    PickedFiles(Vec<PathBuf>),
    PickImage,
    PickedImage(Option<PathBuf>),
    Reboot(bool),
    Wipe(bool),
    Repartition(bool),
    SkipCheck(bool),
    Run,
    Typed(String),
    Confirmed,
    Cancel,
    Stop,
}

pub struct BulkView {
    devices: Vec<Device>,
    action: Action,
    key: String,
    value: String,
    script: String,
    files: Vec<PathBuf>,
    into: String,
    image: Option<PathBuf>,
    reboot: bool,
    wipe: bool,
    repartition: bool,
    skip_check: bool,
    /// The kind every device runs, once Run was pressed.
    kind: Option<Kind>,
    confirm: Option<Confirm>,
    error: Option<String>,
    next: u64,
    fields: dialog::Fields,
}

impl BulkView {
    pub fn new(nodes: Vec<Node>) -> Self {
        Self {
            devices: nodes
                .into_iter()
                .map(|node| Device {
                    node,
                    state: State::Idle,
                })
                .collect(),
            action: Action::Reload,
            key: String::new(),
            value: String::new(),
            script: String::new(),
            files: Vec::new(),
            into: "/".to_string(),
            image: None,
            reboot: true,
            wipe: false,
            repartition: false,
            skip_check: false,
            kind: None,
            confirm: None,
            error: None,
            next: FIRST_JOB,
            fields: dialog::Fields::default(),
        }
    }

    pub fn title(&self) -> String {
        let names: Vec<&str> = self
            .devices
            .iter()
            .map(|device| device.node.name.as_str())
            .collect();
        format!("Run on {}", names.join(", "))
    }

    pub fn has_dialog(&self) -> bool {
        self.confirm.is_some()
    }

    fn running(&self) -> bool {
        self.devices
            .iter()
            .any(|device| matches!(device.state, State::Waiting | State::Running { .. }))
    }

    /// The jobs to run, for the app's subscriptions: each device's node,
    /// where it is in the list, its job id and what it runs.
    pub fn active_jobs(&self) -> impl Iterator<Item = (usize, Node, u64, Kind)> + '_ {
        self.devices
            .iter()
            .enumerate()
            .filter_map(move |(at, device)| match (&device.state, &self.kind) {
                (State::Running { job, .. }, Some(kind)) => {
                    Some((at, device.node.clone(), *job, kind.clone()))
                }
                _ => None,
            })
    }

    /// An event of device `at`'s job.
    pub fn job_event(&mut self, at: usize, event: jobs::Event) {
        let Some(device) = self.devices.get_mut(at) else {
            return;
        };
        let State::Running { last, .. } = &mut device.state else {
            return;
        };
        match event {
            jobs::Event::Progress { label, done, total } => {
                let percent = tessaro_client::report::percent(done, total);
                *last = Some(label.text(format!(" {percent}%")));
            }
            jobs::Event::Line(line) => *last = Some(line),
            jobs::Event::Value(_) => {}
            jobs::Event::Wiped => {
                // As a device window does: its pin and token are worthless.
                if let Ok(mut nodes) = Nodes::load() {
                    let _ = nodes.forget(&device.node.id);
                }
            }
            jobs::Event::Finished(result) => {
                device.state = State::Done(result);
                self.fill();
            }
        }
    }

    /// Start waiting devices while fewer than `bulk::PARALLEL` run.
    fn fill(&mut self) {
        let mut running = self
            .devices
            .iter()
            .filter(|device| matches!(device.state, State::Running { .. }))
            .count();
        for device in &mut self.devices {
            if running >= bulk::PARALLEL {
                break;
            }
            if device.state == State::Waiting {
                device.state = State::Running {
                    job: self.next,
                    last: None,
                };
                self.next += 1;
                running += 1;
            }
        }
    }

    /// What every device runs for the action and the fields, or why not.
    fn kind(&self) -> Result<Kind, String> {
        Ok(match self.action {
            Action::Reload => Kind::Reload,
            Action::RestartBrowser => Kind::Restart(RestartTarget::Browser),
            Action::RestartWeston => Kind::Restart(RestartTarget::Weston),
            Action::RestartAgent => Kind::Restart(RestartTarget::Agent),
            Action::Reboot => Kind::Reboot,
            Action::Set => {
                let key = self.key.trim();
                if key.is_empty() {
                    return Err("name the setting".to_string());
                }
                let value = crate::device::check(key, &self.value)?;
                Kind::Set(BTreeMap::from([(key.to_string(), value)]))
            }
            Action::Script => {
                let name = self.script.trim();
                if name.is_empty() {
                    return Err("name the script".to_string());
                }
                Kind::Script(name.to_string())
            }
            Action::Upload => {
                if self.files.is_empty() {
                    return Err("pick the files to upload".to_string());
                }
                Kind::Upload {
                    local: self.files.clone(),
                    into: store::normalize(self.into.trim())?,
                }
            }
            Action::Update => {
                let Some(image) = self.image.clone() else {
                    return Err("pick the image to send".to_string());
                };
                Kind::Update(Plan {
                    bmap: tessaro_client::transfer::bmap_for(&image),
                    image,
                    wipe_data: self.wipe,
                    repartition: self.repartition,
                    verify: !self.skip_check,
                    reboot: self.reboot,
                })
            }
        })
    }

    /// Every device, from the start.
    fn start(&mut self, kind: Kind) {
        self.kind = Some(kind);
        self.error = None;
        for device in &mut self.devices {
            device.state = State::Waiting;
        }
        self.fill();
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Action(action) => {
                self.action = action;
                self.error = None;
            }
            Message::Key(key) => self.key = key,
            Message::Value(value) => self.value = value,
            Message::Script(name) => self.script = name,
            Message::Into(into) => self.into = into,
            Message::PickFiles => {
                return Task::perform(
                    rfd::AsyncFileDialog::new()
                        .set_title("Upload to every device")
                        .pick_files(),
                    |picked| {
                        Message::PickedFiles(
                            picked
                                .unwrap_or_default()
                                .iter()
                                .map(|handle| handle.path().to_path_buf())
                                .collect(),
                        )
                    },
                );
            }
            Message::PickedFiles(files) => {
                if !files.is_empty() {
                    self.files = files;
                }
            }
            Message::PickImage => {
                return Task::perform(
                    rfd::AsyncFileDialog::new()
                        .set_title("Image to send to every device")
                        .add_filter("Tessaro image", &["zst", "bz2", "wic"])
                        .pick_file(),
                    |picked| Message::PickedImage(picked.map(|handle| handle.path().to_path_buf())),
                );
            }
            Message::PickedImage(image) => {
                if image.is_some() {
                    self.image = image;
                }
            }
            Message::Reboot(on) => self.reboot = on,
            Message::Wipe(on) => self.wipe = on,
            Message::Repartition(on) => self.repartition = on,
            Message::SkipCheck(on) => self.skip_check = on,
            Message::Run => {
                if self.running() {
                    return Task::none();
                }
                match self.kind() {
                    Err(error) => self.error = Some(error),
                    Ok(kind) if self.action.confirmed() => {
                        let warning = match &kind {
                            Kind::Update(plan) => {
                                plan.name().ok().and_then(|name| plan.warning(&name))
                            }
                            _ => None,
                        };
                        let typed = warning.is_some();
                        self.confirm = Some(Confirm {
                            kind,
                            warning,
                            typed: String::new(),
                        });
                        if typed {
                            return self.fields.focus(0);
                        }
                    }
                    Ok(kind) => self.start(kind),
                }
            }
            Message::Typed(typed) => {
                if let Some(confirm) = &mut self.confirm {
                    confirm.typed = typed;
                }
            }
            Message::Confirmed => {
                let count = self.devices.len().to_string();
                let ready = self.confirm.as_ref().is_some_and(|confirm| {
                    confirm.warning.is_none() || confirm.typed.trim() == count
                });
                if ready {
                    if let Some(confirm) = self.confirm.take() {
                        self.start(confirm.kind);
                    }
                }
            }
            Message::Cancel => self.confirm = None,
            Message::Stop => {
                for device in &mut self.devices {
                    match device.state {
                        State::Waiting => device.state = State::Idle,
                        State::Running { .. } => {
                            device.state = State::Done(Err("stopped".to_string()))
                        }
                        _ => {}
                    }
                }
            }
        }
        Task::none()
    }

    /// Enter: the confirmation's default button, else Run.
    pub fn enter(&mut self) -> Task<Message> {
        if self.confirm.is_some() {
            self.update(Message::Confirmed)
        } else {
            self.update(Message::Run)
        }
    }

    /// The fields of the action picked.
    fn inputs(&self) -> Element<'_, Message> {
        let input = |placeholder: &'static str, value: &str, on: fn(String) -> Message| {
            text_input(placeholder, value)
                .on_input(on)
                .on_submit(Message::Run)
                .size(theme::SMALL)
        };
        let check =
            |on: bool, message: fn(bool) -> Message| checkbox(on).on_toggle(message).size(14);
        match self.action {
            Action::Set => column![
                field("Key", input("browser.url", &self.key, Message::Key)),
                field("Value", input("", &self.value, Message::Value)),
            ]
            .spacing(6)
            .into(),
            Action::Script => field(
                "Script",
                input("the script's name", &self.script, Message::Script),
            ),
            Action::Upload => {
                let picked = match self.files.as_slice() {
                    [] => "nothing picked".to_string(),
                    [one] => one.display().to_string(),
                    several => format!("{} files and directories", several.len()),
                };
                column![
                    field(
                        "Files",
                        row![
                            theme::tool("Pick…", Some(Message::PickFiles)),
                            text(picked).size(theme::SMALL).style(theme::muted),
                        ]
                        .spacing(6)
                        .align_y(iced::alignment::Vertical::Center),
                    ),
                    field("Into", input("/", &self.into, Message::Into)),
                ]
                .spacing(6)
                .into()
            }
            Action::Update => {
                let picked = self
                    .image
                    .as_ref()
                    .map_or("nothing picked".to_string(), |image| {
                        image.display().to_string()
                    });
                column![
                    field(
                        "Image",
                        row![
                            theme::tool("Pick…", Some(Message::PickImage)),
                            text(picked).size(theme::SMALL).style(theme::muted),
                        ]
                        .spacing(6)
                        .align_y(iced::alignment::Vertical::Center),
                    ),
                    field("Reboot to apply", check(self.reboot, Message::Reboot)),
                    field("Erase /data", check(self.wipe, Message::Wipe)),
                    field(
                        "Rewrite the whole disk",
                        check(self.repartition, Message::Repartition)
                    ),
                    field(
                        "Skip the checksum check",
                        check(self.skip_check, Message::SkipCheck)
                    ),
                ]
                .spacing(6)
                .into()
            }
            _ => space().into(),
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let running = self.running();
        let controls = column![
            field(
                "Action",
                pick_list(Action::ALL, Some(self.action), Message::Action)
                    .text_size(theme::SMALL)
                    .padding([2, 6]),
            ),
            self.inputs(),
            row![
                theme::default_button("Run", (!running).then_some(Message::Run)),
                theme::dialog_button("Stop", running.then_some(Message::Stop)),
            ]
            .spacing(6),
            dialog::error(self.error.clone()),
        ]
        .spacing(8);

        let mut list = column![].spacing(2);
        for device in &self.devices {
            let (state, detail): (Line, Option<Line>) = match &device.state {
                State::Idle => (Line::of(Tone::Muted, "-"), None),
                State::Waiting => (Line::of(Tone::Muted, "waiting"), None),
                State::Running { last, .. } => (Line::of(Tone::Warn, "running"), last.clone()),
                State::Done(Ok(message)) => (
                    Line::of(Tone::Ok, "done"),
                    Some(Line::plain(message.clone())),
                ),
                State::Done(Err(error)) => (
                    Line::of(Tone::Bad, "failed"),
                    Some(Line::plain(error.clone())),
                ),
            };
            list = list.push(
                row![
                    container(text(&device.node.name).size(theme::SMALL)).width(180),
                    container(
                        text(&device.node.address)
                            .size(theme::SMALL)
                            .style(theme::muted)
                    )
                    .width(150),
                    container(theme::text_line(&state, theme::FONT)).width(80),
                    match detail {
                        Some(detail) => theme::text_line(&detail, theme::FONT),
                        None => space().into(),
                    },
                ]
                .spacing(6),
            );
        }

        let outcomes: Vec<Outcome<String>> = self
            .devices
            .iter()
            .filter_map(|device| match &device.state {
                State::Done(result) => Some(Outcome {
                    member: bulk::member(&device.node),
                    result: result.clone(),
                }),
                _ => None,
            })
            .collect();
        let summary: Element<'_, Message> = if outcomes.is_empty() || running {
            space().into()
        } else {
            theme::text_line(&words::summary(&outcomes), theme::FONT)
        };

        let page = container(
            column![controls, scrollable(list).height(Length::Fill), summary,].spacing(10),
        )
        .padding(10)
        .height(Length::Fill);

        match &self.confirm {
            None => page.into(),
            Some(confirm) => dialog::modal(page.into(), self.confirm_view(confirm)),
        }
    }

    fn confirm_view<'a>(&'a self, confirm: &'a Confirm) -> Element<'a, Message> {
        let mut body = column![text(format!("{} on:", self.action)).size(theme::SMALL)].spacing(6);
        for line in words::devices(
            &self
                .devices
                .iter()
                .map(|device| bulk::member(&device.node))
                .collect::<Vec<_>>(),
        ) {
            body = body.push(theme::text_line(&line, theme::FONT));
        }
        let count = self.devices.len().to_string();
        let ready = match &confirm.warning {
            None => true,
            Some(warning) => {
                body = body
                    .push(
                        text(format!("This will {warning}, on every one of them."))
                            .size(theme::SMALL)
                            .style(text::danger),
                    )
                    .push(field(
                        "Type the count",
                        text_input(&count, &confirm.typed)
                            .id(self.fields.id(0))
                            .on_input(Message::Typed)
                            .on_submit(Message::Confirmed)
                            .size(theme::SMALL),
                    ));
                confirm.typed.trim() == count
            }
        };
        dialog::frame(
            self.action.to_string(),
            body.into(),
            vec![
                theme::dialog_button("Cancel", Some(Message::Cancel)),
                theme::default_button("Go ahead", ready.then_some(Message::Confirmed)),
            ],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(at: usize) -> Node {
        Node {
            id: format!("id{at}"),
            name: format!("kiosk-{at}"),
            address: format!("10.0.0.{at}:7400"),
            fingerprint: String::new(),
            token: Some("t".to_string()),
            tags: Vec::new(),
        }
    }

    fn view(count: usize) -> BulkView {
        BulkView::new((0..count).map(node).collect())
    }

    #[test]
    fn no_more_than_the_limit_run_and_the_next_starts_as_one_ends() {
        let mut view = view(bulk::PARALLEL + 2);
        let _ = view.update(Message::Run);
        assert_eq!(view.active_jobs().count(), bulk::PARALLEL);
        let (at, _, _, _) = view.active_jobs().next().unwrap();
        view.job_event(at, jobs::Event::Finished(Ok("done".to_string())));
        assert_eq!(view.active_jobs().count(), bulk::PARALLEL);
        // Job ids are the window's own, each once.
        let ids: std::collections::BTreeSet<u64> =
            view.active_jobs().map(|(_, _, job, _)| job).collect();
        assert_eq!(ids.len(), bulk::PARALLEL);
        assert!(ids.iter().all(|id| *id >= FIRST_JOB));
    }

    #[test]
    fn a_reboot_is_confirmed_first_and_a_wiping_update_needs_the_count() {
        let mut view = view(2);
        let _ = view.update(Message::Action(Action::Reboot));
        let _ = view.update(Message::Run);
        assert!(view.has_dialog());
        assert_eq!(view.active_jobs().count(), 0);
        let _ = view.update(Message::Confirmed);
        assert_eq!(view.active_jobs().count(), 2);

        let mut view = self::view(2);
        let _ = view.update(Message::Action(Action::Update));
        let _ = view.update(Message::PickedImage(Some("/tmp/image.wic.zst".into())));
        let _ = view.update(Message::Wipe(true));
        let _ = view.update(Message::Run);
        let _ = view.update(Message::Confirmed);
        assert_eq!(view.active_jobs().count(), 0, "the count was not typed");
        let _ = view.update(Message::Typed("2".to_string()));
        let _ = view.update(Message::Confirmed);
        assert_eq!(view.active_jobs().count(), 2);
    }

    #[test]
    fn a_setting_is_checked_before_anything_runs() {
        let mut view = view(2);
        let _ = view.update(Message::Action(Action::Set));
        let _ = view.update(Message::Run);
        assert!(view.error.is_some());
        let _ = view.update(Message::Key("browser.url".to_string()));
        let _ = view.update(Message::Value("https://example.com".to_string()));
        let _ = view.update(Message::Run);
        assert_eq!(view.active_jobs().count(), 2);
        let kind = view.active_jobs().next().unwrap().3;
        match kind {
            Kind::Set(values) => assert_eq!(values["browser.url"], "https://example.com"),
            other => panic!("not a set: {other:?}"),
        }
    }

    #[test]
    fn stop_leaves_the_waiting_devices_unrun() {
        let mut view = view(bulk::PARALLEL + 1);
        let _ = view.update(Message::Run);
        let _ = view.update(Message::Stop);
        assert_eq!(view.active_jobs().count(), 0);
        assert_eq!(view.devices.last().unwrap().state, State::Idle);
        assert!(!view.running());
    }
}
