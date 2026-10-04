//! Try Tessaro: a Tessaro device in a VM on this computer, for someone
//! trying Tessaro out. One window starts and stops it, opens `tessaro-gui`,
//! a terminal with `tessaro-ctl` and Webconfig, and offers things to try.
//! The kiosk itself is in QEMU's own window. How it fits together is in
//! docs/try-tessaro.md.

mod activities;
mod blocking;
mod device;
mod open;
mod paths;
mod settings;
mod vm;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::alignment::Vertical;
use iced::widget::{
    button, center, column, container, image, opaque, pick_list, progress_bar, row, scrollable,
    space, stack, text, text_input, toggler,
};
use iced::{border, window, Color, Element, Font, Length, Size, Subscription, Task, Theme};
use protocol::NodeInfo;
use tessaro_client::text::Line;
use tessaro_style::theme;

use activities::{Action, Activity, Outcome};
use paths::Bundle;
use settings::{Resolution, Settings};
use vm::disk::{self, Prepared};
use vm::Running;

const WINDOW: Size = Size::new(1000.0, 780.0);
/// A step bigger than tessaro-gui's dense tables: this window is read by
/// someone new to Tessaro, not scanned by a technician.
const TEXT: f32 = 15.0;
const SMALL: f32 = 13.0;
const TITLE: f32 = 17.0;
/// How wide a tab's content gets.
const PAGE: f32 = 760.0;
/// The app's own icon, tessaro-gui's with a DEMO ribbon (icons/).
const LOGO: &[u8] = include_bytes!("../icons/try-tessaro.png");
/// How often the device is asked whether it is up while it boots, and
/// whether anything changed (a claim) once it is.
const PROBE_BOOTING: Duration = Duration::from_secs(2);
const PROBE_READY: Duration = Duration::from_secs(10);
/// A boot slower than this gets a word in the window.
const SLOW_BOOT: Duration = Duration::from_secs(300);

fn main() -> iced::Result {
    iced::application(App::boot, App::update, App::view)
        .title(|_: &App| "Try Tessaro".to_string())
        .theme(|_: &App| theme::theme())
        .subscription(App::subscription)
        .exit_on_close_request(false)
        .window(window::Settings {
            size: WINDOW,
            min_size: Some(Size::new(880.0, 620.0)),
            ..window::Settings::default()
        })
        .settings(iced::Settings {
            default_text_size: TEXT.into(),
            ..tessaro_style::settings()
        })
        .run()
}

/// Where the VM is.
enum Vm {
    Stopped,
    /// Unpacking the image or making the disk.
    Preparing,
    Starting {
        running: Running,
        since: Instant,
    },
    Ready {
        running: Running,
    },
    Stopping {
        running: Running,
    },
}

impl Vm {
    fn running(&mut self) -> Option<&mut Running> {
        match self {
            Vm::Starting { running, .. } | Vm::Ready { running } | Vm::Stopping { running } => {
                Some(running)
            }
            Vm::Stopped | Vm::Preparing => None,
        }
    }

    fn api(&self) -> Option<u16> {
        match self {
            Vm::Starting { running, .. } | Vm::Ready { running } | Vm::Stopping { running } => {
                Some(running.ports.api)
            }
            Vm::Stopped | Vm::Preparing => None,
        }
    }

    fn ready_api(&self) -> Option<u16> {
        match self {
            Vm::Ready { running } => Some(running.ports.api),
            _ => None,
        }
    }

    fn label(&self) -> (&'static str, Color) {
        match self {
            Vm::Stopped => ("Stopped", theme::MUTED),
            Vm::Preparing => ("Preparing", theme::WARNING),
            Vm::Starting { .. } => ("Starting", theme::WARNING),
            Vm::Ready { .. } => ("Running", theme::SUCCESS),
            Vm::Stopping { .. } => ("Shutting down", theme::WARNING),
        }
    }
}

/// A card's last result, ready to draw.
enum Shown {
    Lines(Vec<Line>),
    Picture {
        handle: image::Handle,
        saved: PathBuf,
    },
    Failed(String),
}

/// The tabs at the top: the device itself, and things to try on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Basic,
    Tasks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Confirm {
    Reset,
    Quit,
}

#[derive(Debug, Clone)]
enum Message {
    Tick,
    Start,
    Prepared(Result<Prepared, String>),
    Probed(Result<NodeInfo, String>),
    Stop,
    /// The window opened: time to measure the screen it is on.
    WindowOpened(window::Id),
    /// This computer's screen, in pixels.
    Measured((u32, u32)),
    Resolution(Resolution),
    Tab(Tab),
    Sound(bool),
    Microphone(bool),
    Accelerated(bool),
    OpenGui,
    OpenTerminal,
    OpenWebconfig,
    OpenDocs,
    OpenFolder,
    Opened(Result<(), String>),
    AskReset,
    CloseRequested,
    Confirmed,
    Cancelled,
    ResetDone(Result<(), String>),
    Url(String),
    Run(Action),
    Ran(Activity, Result<Outcome, String>),
    ToggleCommands(Activity),
}

struct App {
    bundle: Result<Bundle, String>,
    dir: PathBuf,
    /// Held for the app's life: one launcher per data directory, so two
    /// cannot boot the same disk.
    _lock: Option<File>,
    /// Why nothing can be done at all.
    blocked: Option<String>,
    settings: Settings,
    /// The screen the window opened on, in pixels, which "best for this
    /// screen" is decided from. None until measured, or when iced cannot.
    screen: Option<(u32, u32)>,
    tab: Tab,
    vm: Vm,
    /// Compressed bytes of the image read so far, and how many there are.
    read: Arc<AtomicU64>,
    image_size: u64,
    unpacking: bool,
    probing: bool,
    last_probe: Option<Instant>,
    device: Option<NodeInfo>,
    newer_bundled: bool,
    problem: Option<String>,
    info: Option<String>,
    url: String,
    results: BTreeMap<Activity, Shown>,
    busy: BTreeSet<Activity>,
    commands: BTreeSet<Activity>,
    confirm: Option<Confirm>,
    quit_when_stopped: bool,
    logo: image::Handle,
}

impl App {
    fn boot() -> (Self, Task<Message>) {
        let dir = paths::data_dir();
        let (lock, blocked) = match lock(&dir) {
            Ok(file) => (Some(file), None),
            Err(why) => (None, Some(why)),
        };
        let bundle = Bundle::locate();
        let app = Self {
            blocked: blocked.or_else(|| bundle.as_ref().err().cloned()),
            bundle,
            settings: Settings::load(&dir),
            screen: None,
            tab: Tab::Basic,
            dir,
            _lock: lock,
            vm: Vm::Stopped,
            read: Arc::default(),
            image_size: 0,
            unpacking: false,
            probing: false,
            last_probe: None,
            device: None,
            newer_bundled: false,
            problem: None,
            info: None,
            url: String::new(),
            results: BTreeMap::new(),
            busy: BTreeSet::new(),
            commands: BTreeSet::new(),
            confirm: None,
            quit_when_stopped: false,
            logo: image::Handle::from_bytes(LOGO),
        };
        (app, Task::none())
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => return self.tick(),
            Message::Start => return self.start(),
            Message::Prepared(Ok(prepared)) => {
                self.newer_bundled = prepared.newer_bundled;
                let Ok(bundle) = &self.bundle else {
                    self.vm = Vm::Stopped;
                    return Task::none();
                };
                let resolution = self.settings.resolution_on(self.screen);
                match Running::spawn(
                    bundle,
                    &self.dir,
                    self.settings,
                    resolution,
                    &prepared.overlay,
                ) {
                    Ok(running) => {
                        self.vm = Vm::Starting {
                            running,
                            since: Instant::now(),
                        };
                        self.last_probe = None;
                        self.info = Some(
                            "Booting: the kiosk opens in a window of its own, and Try it \
                             unlocks once the device answers."
                                .to_string(),
                        );
                    }
                    Err(why) => {
                        self.vm = Vm::Stopped;
                        self.problem = Some(why);
                    }
                }
                if self.quit_when_stopped {
                    return self.stop();
                }
            }
            Message::Prepared(Err(why)) => {
                self.vm = Vm::Stopped;
                self.problem = Some(why);
                if self.quit_when_stopped {
                    return iced::exit();
                }
            }
            Message::Probed(answer) => {
                self.probing = false;
                if let Ok(node) = answer {
                    if let Vm::Starting { .. } = self.vm {
                        let Vm::Starting { running, .. } =
                            std::mem::replace(&mut self.vm, Vm::Stopped)
                        else {
                            unreachable!()
                        };
                        self.vm = Vm::Ready { running };
                        self.info = None;
                        let mut state = disk::State::load(&self.dir);
                        if state.node.as_deref() != Some(&node.id) {
                            state.node = Some(node.id.clone());
                            if let Err(why) = state.save(&self.dir) {
                                self.problem = Some(why);
                            }
                        }
                    }
                    if matches!(self.vm, Vm::Ready { .. }) {
                        self.device = Some(node);
                    }
                }
            }
            Message::Stop => return self.stop(),
            Message::WindowOpened(id) => {
                // The monitor's size is in points; the scale factor makes
                // it the pixels QEMU's window shows the device in.
                return window::monitor_size(id).and_then(move |size| {
                    window::scale_factor(id).map(move |scale| {
                        Message::Measured((
                            (size.width * scale).round() as u32,
                            (size.height * scale).round() as u32,
                        ))
                    })
                });
            }
            Message::Measured(pixels) => self.screen = Some(pixels),
            Message::Tab(tab) => self.tab = tab,
            Message::Resolution(resolution) => {
                self.settings.resolution = Some(resolution);
                self.save_settings();
            }
            Message::Sound(on) => {
                self.settings.sound = on;
                self.save_settings();
            }
            Message::Microphone(on) => {
                self.settings.microphone = on;
                self.save_settings();
            }
            Message::Accelerated(on) => {
                self.settings.accelerated = on;
                self.save_settings();
            }
            Message::OpenGui => {
                if let Ok(bundle) = &self.bundle {
                    self.problem = open::gui(bundle).err();
                }
            }
            Message::OpenTerminal => {
                if let (Ok(bundle), Some(api)) = (&self.bundle, self.vm.ready_api()) {
                    self.problem = open::terminal(bundle, &self.dir, api).err();
                }
            }
            Message::OpenWebconfig => {
                if let Some(api) = self.vm.ready_api() {
                    return Task::perform(
                        blocking::run(move || open::webconfig(api)),
                        Message::Opened,
                    );
                }
            }
            Message::OpenDocs => self.problem = open::url(open::DOCS).err(),
            Message::OpenFolder => self.problem = open::folder(&self.dir).err(),
            Message::Opened(result) => self.problem = result.err(),
            Message::AskReset => {
                if matches!(self.vm, Vm::Stopped) {
                    self.confirm = Some(Confirm::Reset);
                }
            }
            Message::CloseRequested => {
                if matches!(self.vm, Vm::Stopped) {
                    return iced::exit();
                }
                self.confirm = Some(Confirm::Quit);
            }
            Message::Cancelled => self.confirm = None,
            Message::Confirmed => match self.confirm.take() {
                Some(Confirm::Reset) => return self.reset(),
                Some(Confirm::Quit) => {
                    self.quit_when_stopped = true;
                    return self.stop();
                }
                None => {}
            },
            Message::ResetDone(Ok(())) => {
                self.newer_bundled = false;
                self.device = None;
                self.results.clear();
                self.info = Some(
                    "Back to factory: the next Start makes a fresh, unclaimed device.".to_string(),
                );
            }
            Message::ResetDone(Err(why)) => self.problem = Some(why),
            Message::Url(url) => self.url = url,
            Message::Run(action) => {
                let Some(api) = self.vm.ready_api() else {
                    return Task::none();
                };
                let activity = action.activity();
                self.busy.insert(activity);
                return Task::perform(
                    blocking::run(move || activities::run(api, action)),
                    move |result| Message::Ran(activity, result),
                );
            }
            Message::Ran(activity, result) => {
                self.busy.remove(&activity);
                let shown = match result {
                    Ok(Outcome::Lines(lines)) => Shown::Lines(lines),
                    Ok(Outcome::Picture { jpeg, saved }) => Shown::Picture {
                        handle: image::Handle::from_bytes(jpeg),
                        saved,
                    },
                    Err(why) => Shown::Failed(why),
                };
                self.results.insert(activity, shown);
            }
            Message::ToggleCommands(activity) => {
                if !self.commands.remove(&activity) {
                    self.commands.insert(activity);
                }
            }
        }
        Task::none()
    }

    fn start(&mut self) -> Task<Message> {
        if !matches!(self.vm, Vm::Stopped) {
            return Task::none();
        }
        let Ok(bundle) = self.bundle.clone() else {
            return Task::none();
        };
        let image = match bundle.image() {
            Ok(image) => image,
            Err(why) => {
                self.problem = Some(why);
                return Task::none();
            }
        };
        self.save_settings();
        self.problem = None;
        self.info = None;
        self.image_size = std::fs::metadata(&image).map_or(0, |meta| meta.len());
        self.unpacking =
            !disk::overlay(&self.dir).exists() && !self.dir.join(disk::base_name(&image)).exists();
        self.read.store(0, Ordering::Relaxed);
        self.vm = Vm::Preparing;
        let qemu_img = bundle.qemu_img();
        let dir = self.dir.clone();
        let read = self.read.clone();
        Task::perform(
            blocking::run(move || disk::prepare(&image, &qemu_img, &dir, read)),
            Message::Prepared,
        )
    }

    fn stop(&mut self) -> Task<Message> {
        match std::mem::replace(&mut self.vm, Vm::Stopped) {
            Vm::Starting { mut running, .. } | Vm::Ready { mut running } => {
                // When the power button cannot be pressed, `escalate` quits
                // and kills on its own clock.
                if let Err(why) = running.stop() {
                    self.info = Some(format!("{why}; stopping it the hard way"));
                } else {
                    self.info = Some("Shutting the device down...".to_string());
                }
                self.vm = Vm::Stopping { running };
            }
            Vm::Stopping { running } => self.vm = Vm::Stopping { running },
            // The unpack runs to its end; quitting leaves its part file,
            // which the next start writes again.
            Vm::Preparing if self.quit_when_stopped => return iced::exit(),
            Vm::Preparing => self.vm = Vm::Preparing,
            Vm::Stopped if self.quit_when_stopped => return iced::exit(),
            Vm::Stopped => {}
        }
        Task::none()
    }

    fn tick(&mut self) -> Task<Message> {
        if let Some(running) = self.vm.running() {
            if let Some(how) = running.exited() {
                let expected = matches!(self.vm, Vm::Stopping { .. });
                self.vm = Vm::Stopped;
                self.device = None;
                self.busy.clear();
                self.info = None;
                if !expected {
                    self.problem = Some(format!("{how}. Show logs has QEMU's own words."));
                }
                if self.quit_when_stopped {
                    return iced::exit();
                }
                return Task::none();
            }
        }
        match &mut self.vm {
            Vm::Stopping { running } => running.escalate(),
            Vm::Starting { since, .. } if since.elapsed() > SLOW_BOOT => {
                self.info = Some(
                    "Still starting. The kiosk window shows how far it got; Show logs has the \
                     console."
                        .to_string(),
                );
            }
            _ => {}
        }
        let every = match self.vm {
            Vm::Starting { .. } => PROBE_BOOTING,
            Vm::Ready { .. } => PROBE_READY,
            _ => return Task::none(),
        };
        let due = self.last_probe.is_none_or(|at| at.elapsed() >= every);
        let Some(api) = self.vm.api() else {
            return Task::none();
        };
        if self.probing || !due {
            return Task::none();
        }
        self.probing = true;
        self.last_probe = Some(Instant::now());
        Task::perform(blocking::run(move || device::probe(api)), Message::Probed)
    }

    fn reset(&mut self) -> Task<Message> {
        let bundled = self
            .bundle
            .as_ref()
            .ok()
            .and_then(|bundle| bundle.image().ok())
            .map(|image| disk::base_name(&image))
            .unwrap_or_default();
        let dir = self.dir.clone();
        Task::perform(
            blocking::run(move || {
                if let Some(node) = disk::reset(&dir, &bundled)? {
                    device::forget(&node)?;
                }
                Ok(())
            }),
            Message::ResetDone,
        )
    }

    fn save_settings(&mut self) {
        if let Err(why) = self.settings.save(&self.dir) {
            self.problem = Some(why);
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            Subscription::run(blocking::ticks).map(|_| Message::Tick),
            window::close_requests().map(|_| Message::CloseRequested),
            window::open_events().map(Message::WindowOpened),
        ])
    }

    fn view(&self) -> Element<'_, Message> {
        let body: Element<'_, Message> = match &self.blocked {
            Some(why) => center(
                column![
                    text("Try Tessaro cannot start")
                        .size(20)
                        .font(theme::bold()),
                    text(why.as_str()).style(theme::muted),
                ]
                .spacing(10)
                .max_width(520),
            )
            .into(),
            None => {
                let page = match self.tab {
                    Tab::Basic => self.basic(),
                    Tab::Tasks => self.tasks(),
                };
                column![
                    self.header(),
                    self.tabs(),
                    self.notices(),
                    container(page).height(Length::Fill),
                ]
                .spacing(16)
                .into()
            }
        };
        let base = container(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding([0, 0])
            .style(theme::desk);
        match self.confirm {
            None => base.into(),
            Some(confirm) => stack![
                base,
                opaque(center(opaque(dialog(confirm))).style(theme::backdrop))
            ]
            .into(),
        }
    }

    fn header(&self) -> Element<'_, Message> {
        let (label, color) = self.vm.label();
        let pill = container(text(label).size(SMALL).color(color))
            .padding([4, 12])
            .style(move |_: &Theme| container::Style {
                background: Some(Color { a: 0.12, ..color }.into()),
                border: border::rounded(12)
                    .color(Color { a: 0.5, ..color })
                    .width(1),
                ..container::Style::default()
            });
        let action = match self.vm {
            Vm::Stopped => big("Start", Some(Message::Start), true),
            Vm::Starting { .. } | Vm::Ready { .. } => big("Stop", Some(Message::Stop), false),
            Vm::Preparing | Vm::Stopping { .. } => big("Wait...", None, false),
        };
        container(
            row![
                image(self.logo.clone()).width(52).height(52),
                column![
                    text("Try Tessaro").size(26).font(theme::bold()),
                    text(
                        "A Tessaro kiosk running on this computer, to explore before the real one"
                    )
                    .style(theme::muted),
                ]
                .spacing(2),
                space::horizontal(),
                pill,
                action,
            ]
            .spacing(16)
            .align_y(Vertical::Center),
        )
        .padding([18, 20])
        .style(theme::app_header)
        .width(Length::Fill)
        .into()
    }

    fn tabs(&self) -> Element<'_, Message> {
        let tab = |label: &'static str, tab: Tab| {
            let selected = self.tab == tab;
            button(text(label).size(TITLE).font(theme::bold()))
                .padding([10, 24])
                .style(theme::nav(selected))
                .on_press(Message::Tab(tab))
        };
        row![tab("Basic", Tab::Basic), tab("Try it", Tab::Tasks)]
            .spacing(6)
            .padding([0, 20])
            .into()
    }

    fn notices(&self) -> Element<'_, Message> {
        let mut lines = column![].spacing(6).padding([0, 20]);
        if let Some(problem) = &self.problem {
            lines = lines.push(text(problem.as_str()).color(theme::DANGER));
        }
        if self.newer_bundled {
            lines = lines.push(
                text(
                    "This app carries a newer Tessaro than the device runs. Reset to factory \
                     to run it; the device's settings and claim go with the reset.",
                )
                .color(theme::WARNING),
            );
        }
        if let Some(info) = &self.info {
            lines = lines.push(text(info.as_str()).style(theme::muted));
        }
        lines.into()
    }

    /// The first tab: the device, the ways in, the settings. Big and plain.
    fn basic(&self) -> Element<'_, Message> {
        let ready = self.vm.ready_api().is_some();
        let stopped = matches!(self.vm, Vm::Stopped);

        let mut facts = column![].spacing(10);
        match &self.vm {
            Vm::Preparing if self.unpacking => {
                let done = self.read.load(Ordering::Relaxed) as f32;
                let total = self.image_size.max(1) as f32;
                facts = facts
                    .push(text("Unpacking the image, first start only").style(theme::muted))
                    .push(progress_bar(0.0..=1.0, (done / total).min(1.0)).girth(6));
            }
            Vm::Preparing => facts = facts.push(text("Preparing the disk").style(theme::muted)),
            _ => {}
        }
        let address = self.vm.api().map_or("-".to_string(), device::address);
        facts = facts.push(fact("Address", address));
        if let Some(node) = &self.device {
            facts = facts
                .push(fact("Name", node.name.clone()))
                .push(fact("Version", node.version.clone()))
                .push(fact(
                    "Claimed",
                    if node.claimed {
                        "yes"
                    } else {
                        "no, anyone here may manage it"
                    }
                    .to_string(),
                ));
        }

        let open = row![
            wide("Tessaro GUI", ready.then_some(Message::OpenGui)),
            wide("Terminal", ready.then_some(Message::OpenTerminal)),
            wide("Webconfig", ready.then_some(Message::OpenWebconfig)),
        ]
        .spacing(10);
        let open = column![
            open,
            text(if ready {
                "The desktop app, a terminal with tessaro-ctl ready, or the device's own pages in your browser."
            } else {
                "These open once the device is running."
            })
            .size(SMALL)
            .style(theme::muted),
        ]
        .spacing(10);

        let resolution: Element<'_, Message> = if stopped {
            pick_list(
                Resolution::ALL,
                Some(self.settings.resolution_on(self.screen)),
                Message::Resolution,
            )
            .text_size(TEXT)
            .padding([8, 12])
            .width(Length::Fill)
            .into()
        } else {
            text(self.settings.resolution_on(self.screen).to_string()).into()
        };
        let settings = column![
            row![text("Screen").width(120), resolution]
                .spacing(12)
                .align_y(Vertical::Center),
            toggler(self.settings.sound)
                .label("Sound through this computer")
                .text_size(TEXT)
                .size(22)
                .on_toggle_maybe(stopped.then_some(Message::Sound)),
            toggler(self.settings.sound && self.settings.microphone)
                .label("Microphone")
                .text_size(TEXT)
                .size(22)
                .on_toggle_maybe((stopped && self.settings.sound).then_some(Message::Microphone)),
            toggler(self.settings.accelerated)
                .label("Graphics on this computer's GPU")
                .text_size(TEXT)
                .size(22)
                .on_toggle_maybe(stopped.then_some(Message::Accelerated)),
            text(if stopped {
                "Applied when the device starts."
            } else {
                "Stop the device to change these."
            })
            .size(SMALL)
            .style(theme::muted),
        ]
        .spacing(14);

        let footer = row![
            small("Docs", Some(Message::OpenDocs)),
            small("Show logs", Some(Message::OpenFolder)),
            space::horizontal(),
            small("Reset to factory", stopped.then_some(Message::AskReset)),
        ]
        .spacing(8);

        page(
            column![
                panel("Device", facts.into()),
                panel("Open", open.into()),
                panel("Settings", settings.into()),
                footer,
            ]
            .spacing(16)
            .into(),
        )
    }

    /// The second tab: things to try on the device, each a task card.
    fn tasks(&self) -> Element<'_, Message> {
        let ready = self.vm.ready_api().is_some();
        let mut cards = column![text(if ready {
            "Each task does one thing on the device. Do it yourself shows the same with \
                 tessaro-ctl, for the terminal."
        } else {
            "Start the device; these unlock once it answers."
        })
        .style(theme::muted),]
        .spacing(14);
        for activity in Activity::ALL {
            cards = cards.push(self.card(activity, ready));
        }
        page(cards.into())
    }

    fn card(&self, activity: Activity, ready: bool) -> Element<'_, Message> {
        let can = ready && !self.busy.contains(&activity);
        let act = |action: Action| can.then_some(Message::Run(action));
        let buttons: Element<'_, Message> = match activity {
            Activity::SelfTest => row![
                small("Open it", act(Action::SelfTest)),
                small("Back to welcome", act(Action::Welcome)),
            ]
            .spacing(6)
            .into(),
            Activity::Tone => small("Play", act(Action::Tone)),
            Activity::Website => row![
                text_input("https://example.com", &self.url)
                    .on_input(Message::Url)
                    .on_submit_maybe(act(Action::ShowWebsite(self.url.clone())))
                    .size(SMALL)
                    .padding([5, 8])
                    .width(260),
                small("Show", act(Action::ShowWebsite(self.url.clone()))),
                small("Restore", act(Action::RestoreWebsite)),
            ]
            .spacing(6)
            .align_y(Vertical::Center)
            .into(),
            Activity::Maintenance => row![
                small("On", act(Action::Maintenance(true))),
                small("Off", act(Action::Maintenance(false))),
            ]
            .spacing(6)
            .into(),
            Activity::Screenshot => small("Take one", act(Action::Screenshot)),
        };

        let mut content = column![
            row![
                text(activity.title()).size(TITLE).font(theme::bold()),
                space::horizontal(),
                buttons,
            ]
            .spacing(10)
            .align_y(Vertical::Center),
            text(activity.blurb()).style(theme::muted),
        ]
        .spacing(8);

        if self.busy.contains(&activity) {
            content = content.push(text("Working...").style(theme::muted));
        } else if let Some(shown) = self.results.get(&activity) {
            content = content.push(result(shown));
        }

        let open = self.commands.contains(&activity);
        content = content.push(
            button(text("Do it yourself").size(SMALL))
                .padding([5, 12])
                .style(if open {
                    theme::primary_button
                } else {
                    theme::chrome_button
                })
                .on_press(Message::ToggleCommands(activity)),
        );
        if open {
            let lines = activity
                .commands(&self.url)
                .into_iter()
                .fold(column![].spacing(4), |lines, command| {
                    lines.push(text(command).font(Font::MONOSPACE).size(SMALL))
                });
            content = content.push(
                container(lines)
                    .padding(10)
                    .width(Length::Fill)
                    .style(theme::desk),
            );
        }

        container(content)
            .padding(16)
            .width(Length::Fill)
            .style(theme::panel)
            .into()
    }
}

/// One launcher per data directory: a lock file held while it runs.
fn lock(dir: &std::path::Path) -> Result<File, String> {
    std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let path = dir.join("lock");
    let file = File::create(&path).map_err(|err| format!("{}: {err}", path.display()))?;
    file.try_lock()
        .map_err(|_| "Try Tessaro is already running; use the window that is open.".to_string())?;
    Ok(file)
}

/// A tab's content: scrolled, centred and no wider than reads well.
fn page(content: Element<'_, Message>) -> Element<'_, Message> {
    scrollable(
        container(container(content).max_width(PAGE))
            .center_x(Length::Fill)
            .padding(iced::Padding::from([0, 20]).bottom(24)),
    )
    .into()
}

fn panel<'a>(title: &'a str, content: Element<'a, Message>) -> Element<'a, Message> {
    container(column![text(title).size(TITLE).font(theme::bold()), content].spacing(14))
        .padding(20)
        .width(Length::Fill)
        .style(theme::panel)
        .into()
}

fn fact<'a>(label: &'a str, value: String) -> Element<'a, Message> {
    row![
        text(label).style(theme::muted).width(120),
        text(value).font(Font::MONOSPACE),
    ]
    .spacing(12)
    .into()
}

/// One of the Open buttons: big, sharing the row.
fn wide(label: &str, on_press: Option<Message>) -> Element<'_, Message> {
    button(text(label).size(TITLE).center().width(Length::Fill))
        .padding([12, 16])
        .width(Length::Fill)
        .style(theme::chrome_button)
        .on_press_maybe(on_press)
        .into()
}

/// A task's buttons and the footer's.
fn small(label: &str, on_press: Option<Message>) -> Element<'_, Message> {
    button(text(label).size(SMALL))
        .padding([5, 12])
        .style(theme::chrome_button)
        .on_press_maybe(on_press)
        .into()
}

fn big(label: &str, on_press: Option<Message>, primary: bool) -> Element<'_, Message> {
    button(text(label).size(16).font(theme::bold()))
        .padding([10, 30])
        .style(if primary {
            theme::primary_button
        } else {
            theme::chrome_button
        })
        .on_press_maybe(on_press)
        .into()
}

fn result(shown: &Shown) -> Element<'_, Message> {
    match shown {
        Shown::Lines(lines) => lines
            .iter()
            .fold(column![].spacing(2), |column, line| {
                column.push(theme::text_line(line, theme::FONT))
            })
            .into(),
        Shown::Picture { handle, saved } => column![
            image(handle.clone()).width(Length::Fill),
            text(format!("Saved to {}", saved.display()))
                .size(SMALL)
                .style(theme::muted),
        ]
        .spacing(6)
        .into(),
        Shown::Failed(why) => text(why.as_str()).color(theme::DANGER).into(),
    }
}

fn dialog(confirm: Confirm) -> Element<'static, Message> {
    let (title, body, yes) = match confirm {
        Confirm::Reset => (
            "Reset to factory?",
            "The device's settings, claim, scripts and files are deleted, and the next Start \
             makes a fresh device from the image this app carries.",
            "Reset",
        ),
        Confirm::Quit => (
            "Quit Try Tessaro?",
            "The device is shut down first, as with Stop. Everything on it is kept for the next \
             time.",
            "Shut down and quit",
        ),
    };
    container(
        column![
            text(title).size(TITLE).font(theme::bold()),
            text(body),
            row![
                space::horizontal(),
                small("Cancel", Some(Message::Cancelled)),
                button(text(yes).size(SMALL))
                    .padding([5, 12])
                    .style(theme::primary_button)
                    .on_press(Message::Confirmed),
            ]
            .spacing(8),
        ]
        .spacing(14),
    )
    .padding(20)
    .width(440)
    .style(theme::panel)
    .into()
}
