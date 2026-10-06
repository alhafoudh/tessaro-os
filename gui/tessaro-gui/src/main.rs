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
mod jobs;
mod keysym;
mod logs;
mod mdi;
mod messages;
mod nodes_view;
#[cfg(test)]
mod screenshot;
mod section;
mod vnc;
mod worker;

// The look is shared with Try Tessaro; `crate::theme` and `crate::icon`
// still name it everywhere here.
use tessaro_style::{icon, theme};

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use iced::futures::channel::mpsc;
use iced::keyboard::{self, key};
use iced::widget::{container, row, space, text};
use iced::{event, mouse, window, Element, Length, Point, Size, Subscription, Task};

use device::Device;
use nodes_view::NodesView;

/// How this client names itself in the hello, for the device's log.
pub const CLIENT: &str = concat!("tessaro-gui ", env!("CARGO_PKG_VERSION"));

/// The window the app opens with.
const WINDOW: Size = Size::new(1400.0, 860.0);
const DEVICE_SIZE: Size = Size::new(1060.0, 640.0);
const CONFIG_SIZE: Size = Size::new(760.0, 440.0);
/// How far the header's title starts from the window's left edge, in the
/// screen's points: past the macOS traffic lights, which the zoom does not
/// scale.
const TRAFFIC_LIGHTS: f32 = if cfg!(target_os = "macos") { 78.0 } else { 0.0 };
/// The kinds of inner window, each remembering where it was left.
const DEVICE: mdi::Kind = "device";
const SETTINGS: mdi::Kind = "settings";

fn main() -> iced::Result {
    let prefs = Prefs::load();
    iced::application(App::boot, App::update, App::view)
        .title(|_: &App| "Tessaro".to_string())
        .theme(|_: &App| theme::theme())
        .subscription(App::subscription)
        .scale_factor(|app: &App| app.zoom.scale())
        .window(window::Settings {
            size: prefs.window_size(),
            position: prefs.window.map_or(window::Position::Default, |at| {
                window::Position::Specific(Point::new(at.x, at.y))
            }),
            maximized: prefs.window.is_some_and(|at| at.maximized),
            min_size: Some(Size::new(800.0, 500.0)),
            // No title bar of its own: the header takes its place, with the
            // traffic lights over its left end (`TRAFFIC_LIGHTS`).
            #[cfg(target_os = "macos")]
            platform_specific: window::settings::PlatformSpecific {
                title_hidden: true,
                titlebar_transparent: true,
                fullsize_content_view: true,
            },
            ..window::Settings::default()
        })
        .settings(settings())
        .run()
}

/// The bundled font, as the default at the default size.
fn settings() -> iced::Settings {
    tessaro_style::settings()
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

/// What the `gui_prefs` table of the client's `tessaro.db` keeps, next to
/// the known nodes: the zoom, where the app window and each kind of inner
/// window was left, and whether device windows show their messages, plus
/// column widths and sorting per table. One row each, a JSON value by name:
/// `scale`, `window`, `messages`, `window:<kind>`, `table:<table>`.
#[derive(Debug, Default)]
struct Prefs {
    scale: Option<f32>,
    /// The app window, in the screen's points, unzoomed: iced scales a new
    /// window's size by the zoom, and reports sizes and positions divided
    /// by it.
    window: Option<mdi::Placement>,
    windows: BTreeMap<String, mdi::Placement>,
    messages: Option<bool>,
    tables: BTreeMap<String, grid::Preferences>,
}

const WINDOW_PREFIX: &str = "window:";
const TABLE_PREFIX: &str = "table:";

impl Prefs {
    /// The app window's size to open with, in the zoom's points.
    fn window_size(&self) -> Size {
        let zoom = Zoom::from_scale(self.scale).scale();
        self.window
            .map_or(WINDOW, |at| Size::new(at.width / zoom, at.height / zoom))
    }

    /// A row that does not parse is left out, as if it was never saved.
    fn from_rows(rows: &BTreeMap<String, String>) -> Self {
        fn parse<T: serde::de::DeserializeOwned>(text: &str) -> Option<T> {
            serde_json::from_str(text).ok()
        }
        let mut prefs = Self::default();
        for (key, value) in rows {
            if let Some(kind) = key.strip_prefix(WINDOW_PREFIX) {
                if let Some(at) = parse(value) {
                    prefs.windows.insert(kind.to_string(), at);
                }
            } else if let Some(table) = key.strip_prefix(TABLE_PREFIX) {
                if let Some(table_prefs) = parse(value) {
                    prefs.tables.insert(table.to_string(), table_prefs);
                }
            } else {
                match key.as_str() {
                    "scale" => prefs.scale = parse(value),
                    "window" => prefs.window = parse(value),
                    "messages" => prefs.messages = parse(value),
                    _ => {}
                }
            }
        }
        prefs
    }

    fn rows(&self) -> BTreeMap<String, String> {
        fn json<T: serde::Serialize>(value: &T) -> Option<String> {
            serde_json::to_string(value).ok()
        }
        let mut rows = BTreeMap::new();
        let mut put = |key: String, value: Option<String>| {
            if let Some(value) = value {
                rows.insert(key, value);
            }
        };
        put("scale".into(), self.scale.as_ref().and_then(json));
        put("window".into(), self.window.as_ref().and_then(json));
        put("messages".into(), self.messages.as_ref().and_then(json));
        for (kind, at) in &self.windows {
            put(format!("{WINDOW_PREFIX}{kind}"), json(at));
        }
        for (table, table_prefs) in &self.tables {
            put(format!("{TABLE_PREFIX}{table}"), json(table_prefs));
        }
        rows
    }

    fn load() -> Self {
        Self::from_rows(&tessaro_client::store::gui_prefs())
    }

    fn save(&self) {
        let _ = tessaro_client::store::save_gui_prefs(&self.rows());
    }
}

struct App {
    table_preferences: BTreeMap<String, grid::Preferences>,
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
    /// Where the app window is, as `Prefs::window` keeps it; `None` until
    /// it first reports.
    window: Option<mdi::Placement>,
    /// What the app window reported since `window` was last written.
    moving: Option<Moving>,
}

/// The app window's geometry not yet written to the prefs: the latest it
/// reported, unzoomed, and when. Written once it has been still for
/// `SETTLE`, so a drag or a resize is one write.
#[derive(Debug, Clone, Copy)]
struct Moving {
    id: window::Id,
    position: Option<Point>,
    size: Option<Size>,
    at: Instant,
}

#[derive(Debug, Clone, Copy)]
enum Key {
    Escape,
    Enter,
    Up,
    Down,
    /// Tab, or Shift-Tab (true).
    Tab(bool),
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
    /// A VNC frame on the GPU, ready to show; not `Device`, which would
    /// raise the window on every frame.
    VncUploaded(mdi::Id, device::VncUploaded),
    Job(mdi::Id, u64, jobs::Event),
    Desk(mdi::Message),
    Key(Key),
    /// Escape, taken by a text field to let go of the cursor.
    FieldEscape,
    /// A key for the VNC panel the pointer is over.
    VncKey(device::VncInput),
    /// The screen's device pixels per point, at start and when it changes.
    Rescaled(f32),
    /// The app window opened, moved or resized: whichever it reported.
    Geometry(window::Id, Option<Point>, Option<Size>),
    /// While the app window's geometry is unwritten: whether it settled.
    Settle,
    /// It settled, and the window has said whether it is maximized.
    Placed(Moving, bool),
    /// The header, which stands in for the title bar, was pressed: move
    /// the app window with the mouse.
    DragWindow,
    /// It was double-clicked: maximize or restore the app window.
    ToggleMaximize,
}

impl App {
    fn boot() -> (Self, Task<Message>) {
        let prefs = Prefs::load();
        let mut app = Self {
            nodes: NodesView::new(),
            table_preferences: prefs.tables.clone(),
            devices: BTreeMap::new(),
            configs: BTreeMap::new(),
            desk: mdi::Desk::new(prefs.window_size(), prefs.windows.clone()),
            next: 1,
            zoom: Zoom::from_scale(prefs.scale),
            dpi: 1.0,
            messages: prefs.messages.unwrap_or(true),
            window: prefs.window,
            moving: None,
        };
        app.nodes.tables =
            grid::Tables::new(prefs.tables.get("nodes").cloned().unwrap_or_default());
        app.rescale();
        let dpi = window::oldest()
            .and_then(window::scale_factor)
            .map(Message::Rescaled);
        (app, dpi)
    }

    fn save(&self) {
        Prefs {
            scale: Some(self.zoom.scale()),
            window: self.window,
            windows: self.desk.placements().clone(),
            messages: Some(self.messages),
            tables: self.table_preferences.clone(),
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
                // The pointer moving over a VNC picture does not raise its
                // window; a click does.
                let passing = matches!(
                    message,
                    device::Message::Vnc(
                        device::VncInput::Move(..)
                            | device::VncInput::Exit
                            | device::VncInput::Scroll(_)
                    )
                );
                if !passing {
                    self.desk.raise(id);
                }
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
                // A device that moved or was claimed was written to the
                // known nodes by its worker.
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
            Message::Vnc(id, event) => match self.devices.get_mut(&id) {
                Some(device) => device
                    .vnc_event(event)
                    .map(move |uploaded| Message::VncUploaded(id, uploaded)),
                None => Task::none(),
            },
            Message::VncUploaded(id, uploaded) => match self.devices.get_mut(&id) {
                Some(device) => device
                    .vnc_uploaded(uploaded)
                    .map(move |uploaded| Message::VncUploaded(id, uploaded)),
                None => Task::none(),
            },
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
            // Over a VNC picture, keys are the device's; only the zoom
            // stays the app's.
            Message::Key(Key::Zoom(by)) => self.key(Key::Zoom(by)),
            Message::Key(Key::ZoomReset) => self.key(Key::ZoomReset),
            Message::Key(_) if self.vnc_keys().is_some() => Task::none(),
            Message::Key(key) => self.key(key),
            // A dialog opens with the cursor in its field, so the first
            // Escape closes it all the same; elsewhere the field keeps it.
            Message::FieldEscape if self.vnc_keys().is_none() && self.dialog_on_top() => {
                self.key(Key::Escape)
            }
            Message::FieldEscape => Task::none(),
            Message::VncKey(input) => match self.vnc_keys() {
                Some(id) => self.device_update(id, device::Message::Vnc(input)),
                None => Task::none(),
            },
            Message::Rescaled(dpi) => {
                self.dpi = dpi;
                self.rescale();
                Task::none()
            }
            Message::Geometry(id, position, size) => {
                if let Some(size) = size {
                    // The desk's branch has no task.
                    let _ = self.update(Message::Desk(mdi::Message::Resized(size)));
                }
                let zoom = self.zoom.scale();
                let was = self.moving.take();
                self.moving = Some(Moving {
                    id,
                    position: position
                        .map(|at| Point::new(at.x * zoom, at.y * zoom))
                        .or(was.and_then(|was| was.position)),
                    size: size
                        .map(|size| size * zoom)
                        .or(was.and_then(|was| was.size)),
                    at: Instant::now(),
                });
                Task::none()
            }
            Message::Settle => match self.moving {
                Some(moving) if moving.at.elapsed() >= SETTLE => {
                    self.moving = None;
                    window::is_maximized(moving.id)
                        .map(move |maximized| Message::Placed(moving, maximized))
                }
                _ => Task::none(),
            },
            Message::Placed(moving, maximized) => {
                let at = placed(self.window, moving, maximized);
                if self.window != Some(at) {
                    self.window = Some(at);
                    self.save();
                }
                Task::none()
            }
            Message::DragWindow => window::oldest().and_then(window::drag),
            Message::ToggleMaximize => window::oldest().and_then(window::toggle_maximize),
        }
    }

    fn nodes_update(&mut self, message: nodes_view::Message) -> Task<Message> {
        let remember = matches!(&message, nodes_view::Message::Table(event) if event.remember());
        let (task, open) = self.nodes.update(message);
        if remember {
            self.table_preferences
                .insert("nodes".into(), self.nodes.tables.preferences());
            self.save();
        }
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
        let remember = matches!(&message, device::Message::Table(_, event) if event.remember());
        let Some(device) = self.devices.get_mut(&id) else {
            return Task::none();
        };
        let task = device
            .update(message)
            .map(move |message| Message::Device(id, message));
        if remember {
            self.table_preferences.insert(
                format!("device:{}", device.node.id),
                device.tables.preferences(),
            );
        }
        if toggled {
            self.messages = device.log_open();
        }
        if toggled || remember {
            self.save();
        }
        task
    }

    /// The device whose VNC panel has the keyboard: the pointer is over its
    /// picture.
    fn vnc_keys(&self) -> Option<mdi::Id> {
        self.devices
            .iter()
            .find(|(_, device)| device.vnc_has_keys())
            .map(|(&id, _)| id)
    }

    /// Whether the window on top shows a dialog.
    fn dialog_on_top(&self) -> bool {
        let Some(top) = self.desk.top() else {
            return self.nodes.has_dialog();
        };
        let id = self.configs.get(&top).map_or(top, |(id, _)| *id);
        self.devices.get(&id).is_some_and(Device::has_dialog)
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
                Key::Tab(back) => return self.nodes.tab(back),
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
                Key::Tab(back) => device::Message::Tab(back),
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
            Key::Tab(back) => device::Message::Tab(back),
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
        let preferences = self
            .table_preferences
            .get(&format!("device:{}", node.id))
            .cloned()
            .unwrap_or_default();
        let mut device = Device::new(node, self.messages);
        device.tables = grid::Tables::new(preferences);
        self.devices.insert(id, device);
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
        .padding(iced::Padding {
            left: 10.0 + TRAFFIC_LIGHTS / self.zoom.scale(),
            right: 10.0,
            ..iced::Padding::ZERO
        })
        .align_y(iced::alignment::Vertical::Center)
        .style(theme::app_header);
        let header = iced::widget::mouse_area(header)
            .on_press(Message::DragWindow)
            .on_double_click(Message::ToggleMaximize);

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
        if self.vnc_keys().is_some() {
            all.push(event::listen_with(vnc_keys));
        }
        if self.moving.is_some() {
            all.push(Subscription::run(settling).map(|()| Message::Settle));
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

/// The keys the app handles, and the window's place, size and scale, for
/// the desk and the prefs. A key a widget took (Esc leaving a text field,
/// Enter submitting one) is left to it, except the zoom.
fn keys(event: iced::Event, status: event::Status, id: window::Id) -> Option<Message> {
    match event {
        iced::Event::Window(window::Event::Opened { position, size }) => {
            Some(Message::Geometry(id, position, Some(size)))
        }
        iced::Event::Window(window::Event::Moved(position)) => {
            Some(Message::Geometry(id, Some(position), None))
        }
        iced::Event::Window(window::Event::Resized(size)) => {
            Some(Message::Geometry(id, None, Some(size)))
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
            let escape = key == keyboard::Key::Named(key::Named::Escape);
            if status == event::Status::Captured {
                return escape.then_some(Message::FieldEscape);
            }
            let key = match key {
                keyboard::Key::Named(key::Named::Escape) => Key::Escape,
                keyboard::Key::Named(key::Named::Enter) => Key::Enter,
                keyboard::Key::Named(key::Named::ArrowUp) => Key::Up,
                keyboard::Key::Named(key::Named::ArrowDown) => Key::Down,
                // No iced field takes Tab, so it comes here from the one
                // with the cursor.
                keyboard::Key::Named(key::Named::Tab) => Key::Tab(modifiers.shift()),
                _ => return None,
            };
            Some(Message::Key(key))
        }
        _ => None,
    }
}

/// Keys for the VNC panel under the pointer, pressed and released. The
/// zoom stays the app's, and a key a text field took is left to it.
fn vnc_keys(event: iced::Event, status: event::Status, _: window::Id) -> Option<Message> {
    let iced::Event::Keyboard(event) = event else {
        return None;
    };
    let (physical, keysym, down) = match event {
        keyboard::Event::KeyPressed {
            key,
            modified_key,
            physical_key,
            location,
            modifiers,
            ..
        } => {
            let zoom = matches!(
                key.as_ref(),
                keyboard::Key::Character("=" | "+" | "-" | "0")
            );
            if (modifiers.command() && zoom) || status == event::Status::Captured {
                return None;
            }
            (physical_key, keysym::of(&modified_key, location), true)
        }
        keyboard::Event::KeyReleased { physical_key, .. } => (physical_key, None, false),
        _ => return None,
    };
    Some(Message::VncKey(device::VncInput::Key {
        physical,
        keysym,
        down,
    }))
}

/// Where the app window is now, from where it was and what it reported.
/// Maximized, only that changes: the geometry stays the one it restores to.
fn placed(was: Option<mdi::Placement>, moving: Moving, maximized: bool) -> mdi::Placement {
    let mut at = was.unwrap_or(mdi::Placement {
        x: 0.0,
        y: 0.0,
        width: WINDOW.width,
        height: WINDOW.height,
        maximized,
    });
    at.maximized = maximized;
    if !maximized {
        if let Some(position) = moving.position {
            (at.x, at.y) = (position.x, position.y);
        }
        if let Some(size) = moving.size {
            (at.width, at.height) = (size.width, size.height);
        }
    }
    at
}

/// How long the app window stays still before its geometry is written.
const SETTLE: Duration = Duration::from_secs(1);

/// A tick every quarter of `SETTLE`, while anyone listens: the pool
/// executor iced runs on has no timer.
fn settling() -> mpsc::UnboundedReceiver<()> {
    let (send, receive) = mpsc::unbounded();
    std::thread::spawn(move || {
        while send.unbounded_send(()).is_ok() {
            std::thread::sleep(SETTLE / 4);
        }
    });
    receive
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

    #[test]
    fn gui_preferences_restore_table_widths_and_sorting_and_skip_bad_rows() {
        let old = Prefs::from_rows(&BTreeMap::from([
            ("scale".to_string(), "1.2".to_string()),
            ("messages".to_string(), "false".to_string()),
            ("window:device".to_string(), "not json".to_string()),
            ("table:nodes".to_string(), "[".to_string()),
        ]));
        assert!(old.tables.is_empty());
        assert!(old.windows.is_empty());
        let mut tables = grid::Tables::default();
        let _ = tables.update::<()>(
            "files",
            grid::Event::Drag {
                column: 1,
                width: 90.0,
                offset: 45.0,
            },
        );
        // A partially completed drag never replaces the last saved width.
        let pending = serde_json::to_value(tables.preferences()).unwrap();
        assert!(pending["files"]["widths"].as_object().unwrap().is_empty());
        let _ = tables.update::<()>("files", grid::Event::Release);
        let _ = tables.update::<()>("files", grid::Event::Sort(1));
        let _ = tables.update::<()>("files", grid::Event::Sort(1));
        let prefs = Prefs {
            tables: BTreeMap::from([("device:node-1".into(), tables.preferences())]),
            ..old
        };
        let decoded = Prefs::from_rows(&prefs.rows());
        assert_eq!(decoded.scale, Some(1.2));
        assert_eq!(decoded.messages, Some(false));
        let restored = grid::Tables::new(decoded.tables["device:node-1"].clone());
        assert_eq!(restored.preferences(), tables.preferences());
        let values = serde_json::to_value(restored.preferences()).unwrap();
        assert_eq!(values["files"]["widths"]["1"], 135.0);
        assert_eq!(values["files"]["sort"]["column"], 1);
        assert_eq!(values["files"]["sort"]["descending"], true);
    }

    #[test]
    fn a_maximized_app_window_keeps_what_it_restores_to_and_opens_unzoomed() {
        let moving = |position, size| Moving {
            id: window::Id::unique(),
            position,
            size,
            at: Instant::now(),
        };
        let at = placed(
            None,
            moving(
                Some(Point::new(100.0, 80.0)),
                Some(Size::new(1200.0, 800.0)),
            ),
            false,
        );
        assert_eq!(
            (at.x, at.y, at.width, at.height),
            (100.0, 80.0, 1200.0, 800.0)
        );

        let max = placed(
            Some(at),
            moving(None, Some(Size::new(1800.0, 1400.0))),
            true,
        );
        assert!(max.maximized);
        assert_eq!(
            (max.x, max.y, max.width, max.height),
            (100.0, 80.0, 1200.0, 800.0)
        );

        let prefs = Prefs {
            scale: Some(2.0),
            window: Some(max),
            ..Prefs::default()
        };
        assert_eq!(prefs.window_size(), Size::new(600.0, 400.0));
    }
}
