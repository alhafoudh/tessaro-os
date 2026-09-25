//! tessaro-gui: Tessaro kiosks from a technician's desktop.
//!
//! One app window. The node list (`nodes_view.rs`) and each opened device
//! (`device.rs`, with a worker thread behind it, `worker.rs`) are inner
//! windows on its desk (`mdi.rs`). How it fits together is in docs/gui.md.

mod blocking;
mod copy_menu;
mod device;
mod dialog;
mod discovery;
mod grid;
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
const NODES_SIZE: Size = Size::new(900.0, 420.0);
const DEVICE_SIZE: Size = Size::new(1060.0, 640.0);

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

/// Cmd + and Cmd -, in tenths, remembered in `gui.json` next to nodes.json.
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

    fn path() -> std::path::PathBuf {
        tessaro_client::nodes::dir().join("gui.json")
    }

    fn load() -> Self {
        std::fs::read(Self::path())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|value| value.get("scale")?.as_f64())
            .map_or(Self(10), |scale| {
                Self((scale * 10.0).round() as i32).step(0)
            })
    }

    fn save(self) {
        let body = serde_json::json!({ "scale": self.scale() }).to_string();
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
    desk: mdi::Desk,
    next: mdi::Id,
    zoom: Zoom,
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
    Worker(mdi::Id, worker::Event),
    Journal(mdi::Id, logs::Event),
    Vnc(mdi::Id, vnc::Event),
    Job(mdi::Id, u64, jobs::Event),
    Desk(mdi::Message),
    Key(Key),
    /// Show the node list, and bring it to the top.
    ShowNodes,
}

impl App {
    fn boot() -> (Self, Task<Message>) {
        let mut desk = mdi::Desk::new(WINDOW);
        desk.open(NODES, NODES_SIZE);
        let app = Self {
            nodes: NodesView::new(),
            devices: BTreeMap::new(),
            desk,
            next: NODES + 1,
            zoom: Zoom::load(),
        };
        (app, Task::none())
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Nodes(message) => {
                self.desk.raise(NODES);
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
            Message::Worker(id, event) => {
                // A device that moved was written to nodes.json by its worker.
                let moved = matches!(event, worker::Event::Note(_));
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
                if let Some(id) = self.desk.update(message) {
                    self.close(id);
                }
                Task::none()
            }
            Message::Key(key) => self.key(key),
            Message::ShowNodes => {
                self.desk.open(NODES, NODES_SIZE);
                self.desk.raise(NODES);
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
        match self.devices.get_mut(&id) {
            Some(device) => device
                .update(message)
                .map(move |message| Message::Device(id, message)),
            None => Task::none(),
        }
    }

    /// The keyboard talks to the window on top.
    fn key(&mut self, key: Key) -> Task<Message> {
        match key {
            Key::Zoom(by) => {
                self.zoom = self.zoom.step(by);
                self.zoom.save();
                return Task::none();
            }
            Key::ZoomReset => {
                self.zoom = Zoom(10);
                self.zoom.save();
                return Task::none();
            }
            _ => {}
        }
        let Some(top) = self.desk.top() else {
            return Task::none();
        };
        if top == NODES {
            let message = match key {
                Key::Escape => nodes_view::Message::Cancel,
                Key::Enter => nodes_view::Message::Enter,
                Key::Up => nodes_view::Message::Step(-1),
                Key::Down => nodes_view::Message::Step(1),
                Key::Zoom(_) | Key::ZoomReset => return Task::none(),
            };
            return self.nodes_update(message);
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
        self.devices.insert(id, Device::new(node));
        self.desk.open(id, DEVICE_SIZE);
    }

    /// Dropping a device drops its subscriptions, and with them its worker
    /// thread, its journal stream and their sessions.
    fn close(&mut self, id: mdi::Id) {
        if id == NODES {
            return;
        }
        self.devices.remove(&id);
        self.desk.close(id);
    }

    fn view(&self) -> Element<'_, Message> {
        let header = container(
            row![
                text("Tessaro").size(15).font(grid::bold()),
                text("kiosk manager").size(theme::SMALL).style(theme::muted),
                space::horizontal(),
                theme::tool("Devices", Some(Message::ShowNodes)),
                theme::tool(
                    "Add address",
                    Some(Message::Nodes(nodes_view::Message::AddAddress))
                ),
                theme::tool("Rescan", Some(Message::Nodes(nodes_view::Message::Rescan))),
            ]
            .spacing(8)
            .align_y(iced::alignment::Vertical::Center),
        )
        .height(mdi::DESK_TOP)
        .width(Length::Fill)
        .padding([0, 10])
        .align_y(iced::alignment::Vertical::Center)
        .style(theme::app_header);

        let desk = self.desk.view(
            |id| {
                if id == NODES {
                    (
                        "Devices".to_string(),
                        false,
                        self.nodes.view().map(Message::Nodes),
                    )
                } else {
                    match self.devices.get(&id) {
                        Some(device) => (
                            device.title(),
                            true,
                            device
                                .view()
                                .map(move |message| Message::Device(id, message)),
                        ),
                        None => (String::new(), true, space().into()),
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

/// The node list's inner window: always there, never closed.
const NODES: mdi::Id = 0;

/// The keys the app handles, and the window's size for the desk. A key a
/// widget took (Esc leaving a text field, Enter submitting one) is left to
/// it, except the zoom.
fn keys(event: iced::Event, status: event::Status, _: window::Id) -> Option<Message> {
    match event {
        iced::Event::Window(window::Event::Resized(size)) => {
            Some(Message::Desk(mdi::Message::Resized(size)))
        }
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
