//! tessaro-gui: Tessaro kiosks from a technician's desktop.
//!
//! One app window. The node list (`nodes_view.rs`) fills its desk
//! (`mdi.rs`), and each opened device (`device.rs`, with a worker thread
//! behind it, `worker.rs`) is an inner window on top. How it fits together
//! is in docs/gui.md.

mod blocking;
mod copy_menu;
mod device;
mod dialog;
mod discovery;
mod grid;
mod icon;
mod jobs;
mod logs;
mod mdi;
mod nodes_view;
mod section;
mod theme;
mod vnc;
mod worker;

use std::collections::BTreeMap;

use iced::keyboard::{self, key};
use iced::widget::{container, row, space, text};
use iced::{event, mouse, window, Element, Length, Size, Subscription, Task};

use device::Device;
use nodes_view::NodesView;

/// How this client names itself in the hello, for the device's log.
pub const CLIENT: &str = concat!("tessaro-gui ", env!("CARGO_PKG_VERSION"));

/// The window the app opens with.
const WINDOW: Size = Size::new(1400.0, 860.0);
const DEVICE_SIZE: Size = Size::new(1060.0, 640.0);
const CONFIG_SIZE: Size = Size::new(760.0, 440.0);
/// The kinds of inner window, each remembering where it was left.
const DEVICE: mdi::Kind = "device";
const SETTINGS: mdi::Kind = "settings";

fn main() -> iced::Result {
    iced::application(App::boot, App::update, App::view)
        .title(|_: &App| "Tessaro".to_string())
        .theme(|_: &App| theme::theme())
        .subscription(App::subscription)
        .scale_factor(|app: &App| app.zoom.scale())
        .window(window::Settings {
            size: WINDOW,
            min_size: Some(Size::new(800.0, 500.0)),
            ..window::Settings::default()
        })
        .settings(iced::Settings {
            default_text_size: theme::TEXT.into(),
            ..iced::Settings::default()
        })
        .run()
}

/// Cmd + and Cmd -, in tenths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Zoom(i32);

impl Zoom {
    const MIN: i32 = 6;
    const MAX: i32 = 20;

    fn scale(self) -> f32 {
        self.0 as f32 / 10.0
    }

    fn step(self, by: i32) -> Self {
        Self((self.0 + by).clamp(Self::MIN, Self::MAX))
    }

    fn from_scale(scale: Option<f32>) -> Self {
        scale.map_or(Self(10), |scale| {
            Self((scale * 10.0).round() as i32).step(0)
        })
    }
}

/// What `gui.json`, next to nodes.json, keeps: the zoom, where each kind of
/// inner window was left, and whether device windows show their messages.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct Prefs {
    scale: Option<f32>,
    #[serde(default)]
    windows: BTreeMap<String, mdi::Placement>,
    messages: Option<bool>,
}

impl Prefs {
    fn path() -> std::path::PathBuf {
        tessaro_client::nodes::dir().join("gui.json")
    }

    fn load() -> Self {
        std::fs::read(Self::path())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        let Ok(body) = serde_json::to_string(self) else {
            return;
        };
        let path = Self::path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, body + "\n");
    }
}

struct App {
    nodes: NodesView,
    devices: BTreeMap<mdi::Id, Device>,
    /// The settings windows: each one's device window and prefix.
    configs: BTreeMap<mdi::Id, (mdi::Id, String)>,
    desk: mdi::Desk,
    next: mdi::Id,
    zoom: Zoom,
    /// Device pixels per point of the screen the app is on, for the desk's
    /// pixel-snapped icons along with the zoom.
    dpi: f32,
    /// Whether a new device window shows its message log: as the last one
    /// toggled it.
    messages: bool,
}

#[derive(Debug, Clone, Copy)]
enum Key {
    Escape,
    Enter,
    Up,
    Down,
    Zoom(i32),
    ZoomReset,
}

#[derive(Debug, Clone)]
enum Message {
    Nodes(nodes_view::Message),
    Discovery(discovery::Event),
    Device(mdi::Id, device::Message),
    /// From a settings window, for its device.
    Settings(mdi::Id, device::Message),
    Worker(mdi::Id, worker::Event),
    Journal(mdi::Id, logs::Event),
    Vnc(mdi::Id, vnc::Event),
    Job(mdi::Id, u64, jobs::Event),
    Desk(mdi::Message),
    Key(Key),
    /// The screen's device pixels per point, at start and when it changes.
    Rescaled(f32),
}

impl App {
    fn boot() -> (Self, Task<Message>) {
        let prefs = Prefs::load();
        let mut app = Self {
            nodes: NodesView::new(),
            devices: BTreeMap::new(),
            configs: BTreeMap::new(),
            desk: mdi::Desk::new(WINDOW, prefs.windows),
            next: 1,
            zoom: Zoom::from_scale(prefs.scale),
            dpi: 1.0,
            messages: prefs.messages.unwrap_or(true),
        };
        app.rescale();
        let dpi = window::oldest()
            .and_then(window::scale_factor)
            .map(Message::Rescaled);
        (app, dpi)
    }

    fn save(&self) {
        Prefs {
            scale: Some(self.zoom.scale()),
            windows: self.desk.placements().clone(),
            messages: Some(self.messages),
        }
        .save();
    }

    /// Tell the desk how many device pixels a point is now.
    fn rescale(&mut self) {
        self.desk.set_pixel(self.dpi * self.zoom.scale());
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Nodes(message) => {
                self.desk.focus_base();
                self.nodes_update(message)
            }
            Message::Discovery(event) => {
                self.nodes.discovered(event);
                Task::none()
            }
            Message::Device(id, message) => {
                self.desk.raise(id);
                self.device_update(id, message)
            }
            Message::Settings(window, message) => {
                self.desk.raise(window);
                match self.configs.get(&window) {
                    Some(&(id, _)) => self.device_update(id, message),
                    None => Task::none(),
                }
            }
            Message::Worker(id, event) => {
                // A device that moved or was claimed was written to
                // nodes.json by its worker.
                let moved = matches!(event, worker::Event::Note(_) | worker::Event::Pinned(_));
                if let Some(device) = self.devices.get_mut(&id) {
                    device.event(event);
                }
                if moved {
                    self.nodes.reload();
                }
                Task::none()
            }
            Message::Journal(id, event) => {
                if let Some(device) = self.devices.get_mut(&id) {
                    device.journal_event(event);
                }
                Task::none()
            }
            Message::Vnc(id, event) => {
                if let Some(device) = self.devices.get_mut(&id) {
                    device.vnc_event(event);
                }
                Task::none()
            }
            Message::Job(id, job, event) => {
                if let Some(device) = self.devices.get_mut(&id) {
                    device.job_event(job, event);
                }
                Task::none()
            }
            Message::Desk(message) => {
                match self.desk.update(message) {
                    Some(mdi::Event::Close(id)) => self.close(id),
                    Some(mdi::Event::Placed) => self.save(),
                    None => {}
                }
                Task::none()
            }
            Message::Key(key) => self.key(key),
            Message::Rescaled(dpi) => {
                self.dpi = dpi;
                self.rescale();
                Task::none()
            }
        }
    }

    fn nodes_update(&mut self, message: nodes_view::Message) -> Task<Message> {
        let (task, open) = self.nodes.update(message);
        let task = task.map(Message::Nodes);
        if let Some(node) = open {
            self.open(node);
        }
        task
    }

    fn device_update(&mut self, id: mdi::Id, message: device::Message) -> Task<Message> {
        if let device::Message::Configure(scope) = &message {
            self.configure(id, &scope.prefix);
        }
        let toggled = matches!(message, device::Message::ToggleLog);
        let Some(device) = self.devices.get_mut(&id) else {
            return Task::none();
        };
        let task = device
            .update(message)
            .map(move |message| Message::Device(id, message));
        if toggled {
            self.messages = device.log_open();
            self.save();
        }
        task
    }

    /// The keyboard talks to the window on top.
    fn key(&mut self, key: Key) -> Task<Message> {
        match key {
            Key::Zoom(by) => {
                self.zoom = self.zoom.step(by);
                self.rescale();
                self.save();
                return Task::none();
            }
            Key::ZoomReset => {
                self.zoom = Zoom(10);
                self.rescale();
                self.save();
                return Task::none();
            }
            _ => {}
        }
        let Some(top) = self.desk.top() else {
            let message = match key {
                Key::Escape => nodes_view::Message::Cancel,
                Key::Enter => nodes_view::Message::Enter,
                Key::Up => nodes_view::Message::Step(-1),
                Key::Down => nodes_view::Message::Step(1),
                Key::Zoom(_) | Key::ZoomReset => return Task::none(),
            };
            return self.nodes_update(message);
        };
        if let Some((id, prefix)) = self.configs.get(&top).cloned() {
            let dialog = self.devices.get(&id).is_some_and(Device::has_dialog);
            let cfg = |cfg| device::Message::Cfg(prefix.clone(), cfg);
            let message = match key {
                Key::Escape if dialog => device::Message::Cancel,
                Key::Escape => {
                    self.close(top);
                    return Task::none();
                }
                Key::Enter => cfg(device::Cfg::Enter),
                Key::Up => cfg(device::Cfg::Step(-1)),
                Key::Down => cfg(device::Cfg::Step(1)),
                Key::Zoom(_) | Key::ZoomReset => return Task::none(),
            };
            return self.device_update(id, message);
        }
        let dialog = self.devices.get(&top).is_some_and(Device::has_dialog);
        let message = match key {
            Key::Escape if dialog => device::Message::Cancel,
            Key::Escape => {
                self.close(top);
                return Task::none();
            }
            Key::Enter => device::Message::Enter,
            Key::Up => device::Message::Step(-1),
            Key::Down => device::Message::Step(1),
            Key::Zoom(_) | Key::ZoomReset => return Task::none(),
        };
        self.device_update(top, message)
    }

    /// An inner window for `node`, or the one it already has, on top.
    fn open(&mut self, node: tessaro_client::nodes::Node) {
        if let Some((&id, _)) = self
            .devices
            .iter()
            .find(|(_, device)| device.node.id == node.id)
        {
            return self.desk.raise(id);
        }
        let id = self.next;
        self.next += 1;
        self.devices.insert(id, Device::new(node, self.messages));
        self.desk.open(id, DEVICE, DEVICE_SIZE);
    }

    /// The settings window of `prefix` on the device in window `id`, opened
    /// or raised; the device keeps its table's state.
    fn configure(&mut self, id: mdi::Id, prefix: &str) {
        let open = self
            .configs
            .iter()
            .find(|(_, (device, open))| *device == id && open == prefix)
            .map(|(&window, _)| window);
        if let Some(window) = open {
            return self.desk.raise(window);
        }
        let window = self.next;
        self.next += 1;
        self.configs.insert(window, (id, prefix.to_string()));
        self.desk.open(window, SETTINGS, CONFIG_SIZE);
    }

    /// Dropping a device drops its subscriptions, and with them its worker
    /// thread, its journal stream and their sessions. Its settings windows
    /// close with it.
    fn close(&mut self, id: mdi::Id) {
        if let Some((device, prefix)) = self.configs.remove(&id) {
            if let Some(device) = self.devices.get_mut(&device) {
                let _ = device.update(device::Message::Unconfigure(prefix));
            }
            return self.desk.close(id);
        }
        let windows: Vec<mdi::Id> = self
            .configs
            .iter()
            .filter(|(_, (device, _))| *device == id)
            .map(|(&window, _)| window)
            .collect();
        for window in windows {
            self.configs.remove(&window);
            self.desk.close(window);
        }
        self.devices.remove(&id);
        self.desk.close(id);
    }

    fn view(&self) -> Element<'_, Message> {
        let header = container(
            row![
                // Bottom-aligned boxes as tall as their font: iced has no
                // baseline alignment, and centred ones of two sizes put the
                // smaller text's baseline higher.
                row![
                    text("Tessaro").size(15).line_height(1.0).font(grid::bold()),
                    text("kiosk manager")
                        .size(theme::SMALL)
                        .line_height(1.0)
                        .style(theme::muted),
                ]
                .spacing(8)
                .align_y(iced::alignment::Vertical::Bottom),
            ]
            .align_y(iced::alignment::Vertical::Center),
        )
        .height(mdi::DESK_TOP)
        .width(Length::Fill)
        .padding([0, 10])
        .align_y(iced::alignment::Vertical::Center)
        .style(theme::app_header);

        let empty = || mdi::Window {
            title: String::new(),
            closable: true,
            tools: None,
            content: space().into(),
        };
        let desk = self.desk.view(
            self.nodes.view().map(Message::Nodes),
            |id| {
                if let Some((device, prefix)) = self.configs.get(&id) {
                    match self.devices.get(device) {
                        Some(device) => mdi::Window {
                            title: device.config_title(prefix),
                            closable: true,
                            tools: None,
                            content: device
                                .config_view(prefix)
                                .map(move |message| Message::Settings(id, message)),
                        },
                        None => empty(),
                    }
                } else {
                    let to_device = move |message| Message::Device(id, message);
                    match self.devices.get(&id) {
                        Some(device) => mdi::Window {
                            title: device.title(),
                            closable: true,
                            tools: Some(device.title_tools().map(to_device)),
                            content: device.view().map(to_device),
                        },
                        None => empty(),
                    }
                }
            },
            Message::Desk,
        );
        iced::widget::column![header, desk].into()
    }

    fn subscription(&self) -> Subscription<Message> {
        let workers = self.devices.iter().map(|(&id, device)| {
            worker::subscription(device.node.clone())
                .with(id)
                .map(|(id, event)| Message::Worker(id, event))
        });
        let journals = self.devices.iter().filter_map(|(&id, device)| {
            let (unit, generation) = device.journal_stream()?;
            Some(
                logs::subscription(device.node.clone(), unit, generation)
                    .with(id)
                    .map(|(id, event)| Message::Journal(id, event)),
            )
        });
        let viewers = self.devices.iter().filter_map(|(&id, device)| {
            let generation = device.vnc_stream()?;
            Some(
                vnc::subscription(device.node.clone(), generation)
                    .with(id)
                    .map(|(id, event)| Message::Vnc(id, event)),
            )
        });
        let work: Vec<Subscription<Message>> = self
            .devices
            .iter()
            .flat_map(|(&id, device)| {
                device.active_jobs().map(move |job| {
                    jobs::subscription(device.node.clone(), job.id, job.kind.clone())
                        .with((id, job.id))
                        .map(|((id, job), event)| Message::Job(id, job, event))
                })
            })
            .collect();
        let mut all = vec![
            discovery::subscription(self.nodes.generation).map(Message::Discovery),
            event::listen_with(keys),
        ];
        if self.desk.dragging() {
            all.push(event::listen_with(dragging));
        }
        Subscription::batch(
            all.into_iter()
                .chain(workers)
                .chain(journals)
                .chain(viewers)
                .chain(work),
        )
    }
}

/// The keys the app handles, and the window's size and scale for the desk.
/// A key a widget took (Esc leaving a text field, Enter submitting one) is
/// left to it, except the zoom.
fn keys(event: iced::Event, status: event::Status, _: window::Id) -> Option<Message> {
    match event {
        iced::Event::Window(window::Event::Resized(size)) => {
            Some(Message::Desk(mdi::Message::Resized(size)))
        }
        iced::Event::Window(window::Event::Rescaled(dpi)) => Some(Message::Rescaled(dpi)),
        iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
            if modifiers.command() {
                return match key.as_ref() {
                    keyboard::Key::Character("=" | "+") => Some(Message::Key(Key::Zoom(1))),
                    keyboard::Key::Character("-") => Some(Message::Key(Key::Zoom(-1))),
                    keyboard::Key::Character("0") => Some(Message::Key(Key::ZoomReset)),
                    _ => None,
                };
            }
            if status == event::Status::Captured {
                return None;
            }
            let key = match key {
                keyboard::Key::Named(key::Named::Escape) => Key::Escape,
                keyboard::Key::Named(key::Named::Enter) => Key::Enter,
                keyboard::Key::Named(key::Named::ArrowUp) => Key::Up,
                keyboard::Key::Named(key::Named::ArrowDown) => Key::Down,
                _ => return None,
            };
            Some(Message::Key(key))
        }
        _ => None,
    }
}

/// While an inner window is dragged: the cursor, wherever it is.
fn dragging(event: iced::Event, _: event::Status, _: window::Id) -> Option<Message> {
    match event {
        iced::Event::Mouse(mouse::Event::CursorMoved { position }) => {
            Some(Message::Desk(mdi::Message::Cursor(position)))
        }
        iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
        | iced::Event::Mouse(mouse::Event::CursorLeft) => {
            Some(Message::Desk(mdi::Message::Release))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_steps_in_tenths_and_stays_in_range() {
        assert_eq!(Zoom(10).step(1).scale(), 1.1);
        assert_eq!(Zoom(Zoom::MAX).step(1), Zoom(Zoom::MAX));
        assert_eq!(Zoom(Zoom::MIN).step(-1), Zoom(Zoom::MIN));
    }
}
