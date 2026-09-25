//! Inner windows: every open device and settings window floats on a desk
//! in the one app window, WinBox style, over the desk's own screen (the
//! node list). A window moves by its title bar, resizes from its
//! bottom-right corner, maximizes (button or double-click on the title) and
//! closes from the title bar. The last one clicked is on top and has the
//! keyboard; clicking the screen under them gives the keyboard to it.
//!
//! Built from iced's `stack` (the screen, then one layer per window, in
//! z-order), `pin` (its position) and `opaque` (so a window hides what is
//! under it from the mouse as well as the eye). While a window is dragged,
//! the app listens for the cursor itself: the title bar only says where on
//! it the drag began.
//!
//! `pin` gives a window at most what is left of the desk right of and below
//! its position, so every window is kept whole on the desk: what is stored
//! is what is drawn.
//!
//! Where a window was is remembered per kind, not per window: the next
//! window of that kind opens there. A maximized one is remembered as
//! maximized, next to the size it restores to.

use std::collections::BTreeMap;

use iced::widget::text::Wrapping;
use iced::widget::{column, container, mouse_area, opaque, pin, row, space, stack, text};
use iced::{mouse, Element, Length, Point, Size, Vector};
use serde::{Deserialize, Serialize};

use crate::grid::bold;
use crate::icon::{self, Icon};
use crate::theme;

pub type Id = u64;

/// What kind of window: the key its placement is remembered under.
pub type Kind = &'static str;

/// The height of the app's own header, above the desk: where the desk
/// begins in window coordinates.
pub const DESK_TOP: f32 = 34.0;
const TITLE: f32 = 28.0;
const GRIP: f32 = 18.0;
const MIN: Size = Size::new(380.0, 220.0);
/// Where a kind's first window opens, and how far each further one at the
/// same spot is cascaded below and right of it.
const ORIGIN: Point = Point::new(16.0, 12.0);
const CASCADE: f32 = 28.0;

/// Where a window of some kind was. The geometry is always the one it
/// restores to, so maximizing only sets `maximized`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub maximized: bool,
}

#[derive(Debug, Clone)]
struct Frame {
    id: Id,
    kind: Kind,
    position: Point,
    size: Size,
    maximized: bool,
}

#[derive(Debug, Clone, Copy)]
enum Drag {
    /// `grab` is where on the window the cursor holds it.
    Move { id: Id, grab: Vector },
    /// `grab` is how far the cursor is from the bottom-right corner.
    Resize { id: Id, grab: Vector },
}

#[derive(Debug, Clone)]
pub enum Message {
    /// The cursor over a title bar, in its own coordinates.
    OverTitle(Point),
    /// The cursor over a resize grip, in its own coordinates.
    OverGrip(Point),
    StartMove(Id),
    StartResize(Id),
    Raise(Id),
    /// The screen under the windows was clicked.
    FocusBase,
    Maximize(Id),
    Close(Id),
    /// The cursor, in window coordinates, while dragging.
    Cursor(Point),
    Release,
    /// The app window's new size.
    Resized(Size),
}

/// What the app has to act on after an update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// A window's close was asked for: the app drops what it shows.
    Close(Id),
    /// A window moved, resized or (un)maximized: `placements` changed.
    Placed,
}

/// What the app gives for one window.
pub struct Window<'a, M> {
    pub title: String,
    pub closable: bool,
    /// Buttons in the title bar, before maximize and close.
    pub tools: Option<Element<'a, M>>,
    pub content: Element<'a, M>,
}

pub struct Desk {
    /// Bottom to top.
    frames: Vec<Frame>,
    /// The window with the keyboard; `None` is the screen under them.
    focus: Option<Id>,
    drag: Option<Drag>,
    /// The last cursor position over a title bar or grip, for the drag that
    /// a press there starts.
    over: Vector,
    size: Size,
    placements: BTreeMap<String, Placement>,
    /// Device pixels per point, for the title-bar icons (`icon.rs`).
    pixel: f32,
}

impl Desk {
    pub fn new(window: Size, placements: BTreeMap<String, Placement>) -> Self {
        Self {
            frames: Vec::new(),
            focus: None,
            drag: None,
            over: Vector::ZERO,
            size: desk_size(window),
            placements,
            pixel: 1.0,
        }
    }

    pub fn set_pixel(&mut self, pixel: f32) {
        self.pixel = pixel;
    }

    /// Where each kind of window was last left.
    pub fn placements(&self) -> &BTreeMap<String, Placement> {
        &self.placements
    }

    /// A new window of `kind`, where the last one of its kind was left (or
    /// at `size` in the corner), cascaded off any of its kind already
    /// there, on top.
    pub fn open(&mut self, id: Id, kind: Kind, size: Size) {
        if self.contains(id) {
            return self.raise(id);
        }
        let placed = self.placements.get(kind).copied();
        let (mut position, size, maximized) = match placed {
            Some(at) => (
                Point::new(at.x, at.y),
                Size::new(at.width, at.height),
                at.maximized,
            ),
            None => (ORIGIN, size, false),
        };
        for _ in 0..8 {
            let taken = self
                .frames
                .iter()
                .any(|frame| frame.kind == kind && frame.position == position);
            if !taken {
                break;
            }
            position += Vector::new(CASCADE, CASCADE);
        }
        let (position, size) = fit(position, size, self.size);
        self.frames.push(Frame {
            id,
            kind,
            position,
            size,
            maximized,
        });
        self.focus = Some(id);
    }

    pub fn contains(&self, id: Id) -> bool {
        self.frames.iter().any(|frame| frame.id == id)
    }

    pub fn close(&mut self, id: Id) {
        self.frames.retain(|frame| frame.id != id);
        if matches!(self.drag, Some(Drag::Move { id: held, .. } | Drag::Resize { id: held, .. }) if held == id)
        {
            self.drag = None;
        }
        if self.focus == Some(id) {
            self.focus = self.frames.last().map(|frame| frame.id);
        }
    }

    pub fn raise(&mut self, id: Id) {
        if let Some(at) = self.frames.iter().position(|frame| frame.id == id) {
            let frame = self.frames.remove(at);
            self.frames.push(frame);
            self.focus = Some(id);
        }
    }

    /// Give the keyboard to the screen under the windows.
    pub fn focus_base(&mut self) {
        self.focus = None;
    }

    /// The window the keyboard talks to; `None` is the screen under them.
    pub fn top(&self) -> Option<Id> {
        self.focus
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn update(&mut self, message: Message) -> Option<Event> {
        match message {
            Message::OverTitle(at) => self.over = Vector::new(at.x, at.y),
            Message::OverGrip(at) => self.over = Vector::new(GRIP - at.x, GRIP - at.y),
            Message::StartMove(id) => {
                self.raise(id);
                if self.frame(id).is_some_and(|frame| !frame.maximized) {
                    self.drag = Some(Drag::Move {
                        id,
                        grab: self.over,
                    });
                }
            }
            Message::StartResize(id) => {
                self.raise(id);
                self.drag = Some(Drag::Resize {
                    id,
                    grab: self.over,
                });
            }
            Message::Raise(id) => self.raise(id),
            Message::FocusBase => self.focus_base(),
            Message::Maximize(id) => {
                self.raise(id);
                if let Some(frame) = self.frame_mut(id) {
                    frame.maximized = !frame.maximized;
                }
                return self.remember(id);
            }
            Message::Close(id) => return Some(Event::Close(id)),
            Message::Cursor(at) => self.dragged(Point::new(at.x, at.y - DESK_TOP)),
            Message::Release => {
                if let Some(Drag::Move { id, .. } | Drag::Resize { id, .. }) = self.drag.take() {
                    return self.remember(id);
                }
            }
            Message::Resized(window) => {
                self.size = desk_size(window);
                let desk = self.size;
                for frame in &mut self.frames {
                    (frame.position, frame.size) = fit(frame.position, frame.size, desk);
                }
            }
        }
        None
    }

    /// Keep where window `id` is as where its kind goes.
    fn remember(&mut self, id: Id) -> Option<Event> {
        let frame = self.frame(id)?;
        let placement = Placement {
            x: frame.position.x,
            y: frame.position.y,
            width: frame.size.width,
            height: frame.size.height,
            maximized: frame.maximized,
        };
        self.placements.insert(frame.kind.to_string(), placement);
        Some(Event::Placed)
    }

    fn dragged(&mut self, at: Point) {
        let desk = self.size;
        match self.drag {
            Some(Drag::Move { id, grab }) => {
                if let Some(frame) = self.frame_mut(id) {
                    let x = (at.x - grab.x).min(desk.width - frame.size.width).max(0.0);
                    let y = (at.y - grab.y)
                        .min(desk.height - frame.size.height)
                        .max(0.0);
                    frame.position = Point::new(x, y);
                }
            }
            Some(Drag::Resize { id, grab }) => {
                if let Some(frame) = self.frame_mut(id) {
                    let room = Size::new(
                        (desk.width - frame.position.x).max(MIN.width),
                        (desk.height - frame.position.y).max(MIN.height),
                    );
                    frame.size = Size::new(
                        (at.x + grab.x - frame.position.x)
                            .max(MIN.width)
                            .min(room.width),
                        (at.y + grab.y - frame.position.y)
                            .max(MIN.height)
                            .min(room.height),
                    );
                }
            }
            None => {}
        }
    }

    fn frame(&self, id: Id) -> Option<&Frame> {
        self.frames.iter().find(|frame| frame.id == id)
    }

    fn frame_mut(&mut self, id: Id) -> Option<&mut Frame> {
        self.frames.iter_mut().find(|frame| frame.id == id)
    }

    /// The desk: `base` filling it, and every window on top of it. `window`
    /// gives what a window shows.
    pub fn view<'a, M: Clone + 'a>(
        &'a self,
        base: Element<'a, M>,
        window: impl Fn(Id) -> Window<'a, M>,
        map: fn(Message) -> M,
    ) -> Element<'a, M> {
        let mut layers = stack![mouse_area(
            container(base)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(theme::desk)
        )
        .on_press(map(Message::FocusBase))]
        .width(Length::Fill)
        .height(Length::Fill);

        for frame in &self.frames {
            let focused = self.focus == Some(frame.id);
            let (position, size) = if frame.maximized {
                (Point::ORIGIN, self.size)
            } else {
                (frame.position, frame.size)
            };
            layers = layers.push(
                pin(opaque(self.frame_view(
                    frame,
                    window(frame.id),
                    focused,
                    size,
                    map,
                )))
                .position(position),
            );
        }
        layers.into()
    }

    fn frame_view<'a, M: Clone + 'a>(
        &'a self,
        frame: &Frame,
        window: Window<'a, M>,
        focused: bool,
        size: Size,
        map: fn(Message) -> M,
    ) -> Element<'a, M> {
        let id = frame.id;
        let maximize = if frame.maximized {
            Icon::Restore
        } else {
            Icon::Maximize
        };
        let mut buttons = row![theme::title_button(
            icon::view(maximize, self.pixel),
            map(Message::Maximize(id))
        )]
        .spacing(2);
        if window.closable {
            buttons = buttons.push(theme::title_button(
                icon::view(Icon::Close, self.pixel),
                map(Message::Close(id)),
            ));
        }
        let mut strip = row![container(
            text(window.title)
                .size(theme::SMALL)
                .font(bold())
                .wrapping(Wrapping::None)
        )
        .width(Length::Fill)
        .clip(true)]
        .spacing(6)
        .align_y(iced::alignment::Vertical::Center);
        if let Some(tools) = window.tools {
            strip = strip.push(tools);
        }
        let bar = mouse_area(
            container(strip.push(space().width(4)).push(buttons))
                .height(TITLE)
                .padding(iced::Padding {
                    left: 8.0,
                    right: 4.0,
                    ..iced::Padding::ZERO
                })
                .width(Length::Fill)
                .align_y(iced::alignment::Vertical::Center)
                .style(theme::frame_title(focused)),
        )
        .on_press(map(Message::StartMove(id)))
        .on_double_click(map(Message::Maximize(id)))
        .on_move(move |at| map(Message::OverTitle(at)));

        let body = column![bar, container(window.content).height(Length::Fill)];
        let mut layers = stack![body].width(Length::Fill).height(Length::Fill);
        if !frame.maximized {
            let grip = mouse_area(
                container(text("◢").size(12.0).color(theme::TEXT_COLOR))
                    .width(GRIP)
                    .height(GRIP)
                    .align_right(GRIP)
                    .align_bottom(GRIP),
            )
            .interaction(mouse::Interaction::ResizingDiagonallyDown)
            .on_press(map(Message::StartResize(id)))
            .on_move(move |at| map(Message::OverGrip(at)));
            layers = layers.push(
                container(grip)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_right(Length::Fill)
                    .align_bottom(Length::Fill),
            );
        }

        mouse_area(
            container(layers)
                .width(size.width)
                .height(size.height)
                .padding(1)
                .style(theme::frame(focused)),
        )
        .on_press(map(Message::Raise(id)))
        .into()
    }
}

fn desk_size(window: Size) -> Size {
    Size::new(window.width, (window.height - DESK_TOP).max(0.0))
}

/// `position` and `size` moved and shrunk so the window is whole on `desk`,
/// and no smaller than `MIN` while the desk has room for that.
fn fit(position: Point, size: Size, desk: Size) -> (Point, Size) {
    let width = size.width.min(desk.width).max(MIN.width.min(desk.width));
    let height = size
        .height
        .min(desk.height)
        .max(MIN.height.min(desk.height));
    let x = position.x.min(desk.width - width).max(0.0);
    let y = position.y.min(desk.height - height).max(0.0);
    (Point::new(x, y), Size::new(width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KIND: Kind = "device";

    fn desk() -> Desk {
        let mut desk = Desk::new(Size::new(1200.0, 800.0 + DESK_TOP), BTreeMap::new());
        desk.open(1, KIND, Size::new(500.0, 300.0));
        desk.open(2, KIND, Size::new(500.0, 300.0));
        desk
    }

    #[test]
    fn the_last_opened_or_clicked_window_is_on_top() {
        let mut desk = desk();
        assert_eq!(desk.top(), Some(2));
        desk.update(Message::Raise(1));
        assert_eq!(desk.top(), Some(1));
    }

    #[test]
    fn windows_of_a_kind_cascade_off_each_other() {
        let desk = desk();
        assert_eq!(desk.frame(1).unwrap().position, ORIGIN);
        assert_eq!(
            desk.frame(2).unwrap().position,
            ORIGIN + Vector::new(CASCADE, CASCADE)
        );
    }

    #[test]
    fn a_window_moves_where_it_is_held_and_stays_on_the_desk() {
        let mut desk = desk();
        desk.update(Message::OverTitle(Point::new(10.0, 5.0)));
        desk.update(Message::StartMove(1));
        desk.update(Message::Cursor(Point::new(310.0, 205.0 + DESK_TOP)));
        assert_eq!(desk.frame(1).unwrap().position, Point::new(300.0, 200.0));

        desk.update(Message::Cursor(Point::new(-5000.0, -5000.0)));
        assert_eq!(desk.frame(1).unwrap().position, Point::ORIGIN);
        desk.update(Message::Cursor(Point::new(5000.0, 5000.0)));
        assert_eq!(desk.frame(1).unwrap().position, Point::new(700.0, 500.0));
        assert_eq!(desk.update(Message::Release), Some(Event::Placed));
        assert!(!desk.dragging());
        assert_eq!(desk.placements()[KIND].x, 700.0);
    }

    #[test]
    fn a_window_does_not_shrink_below_its_minimum() {
        let mut desk = desk();
        desk.update(Message::StartResize(2));
        desk.update(Message::Cursor(Point::new(0.0, 0.0)));
        assert_eq!(desk.frame(2).unwrap().size, MIN);
    }

    #[test]
    fn a_window_grows_only_to_the_desk_edge() {
        let mut desk = desk();
        desk.update(Message::OverGrip(Point::new(GRIP, GRIP)));
        desk.update(Message::StartResize(1));
        desk.update(Message::Cursor(Point::new(5000.0, 5000.0)));
        assert_eq!(
            desk.frame(1).unwrap().size,
            Size::new(1200.0 - ORIGIN.x, 800.0 - ORIGIN.y)
        );
    }

    #[test]
    fn a_window_opens_where_its_kind_was_left_within_the_desk() {
        let placed = Placement {
            x: 900.0,
            y: 700.0,
            width: 600.0,
            height: 400.0,
            maximized: true,
        };
        let mut desk = Desk::new(
            Size::new(1200.0, 800.0 + DESK_TOP),
            BTreeMap::from([(KIND.to_string(), placed)]),
        );
        desk.open(1, KIND, Size::new(500.0, 300.0));
        let frame = desk.frame(1).unwrap();
        assert_eq!(frame.position, Point::new(600.0, 400.0));
        assert_eq!(frame.size, Size::new(600.0, 400.0));
        assert!(frame.maximized);

        desk.open(2, "settings", Size::new(2000.0, 300.0));
        let frame = desk.frame(2).unwrap();
        assert_eq!(frame.position, Point::new(0.0, ORIGIN.y));
        assert_eq!(frame.size.width, 1200.0);
    }

    #[test]
    fn maximized_is_remembered_with_the_size_it_restores_to() {
        let mut desk = desk();
        assert_eq!(desk.update(Message::Maximize(1)), Some(Event::Placed));
        let placed = desk.placements()[KIND];
        assert!(placed.maximized);
        assert_eq!((placed.width, placed.height), (500.0, 300.0));
    }

    #[test]
    fn the_keyboard_falls_back_to_the_screen_under_the_windows() {
        let mut desk = desk();
        desk.update(Message::FocusBase);
        assert_eq!(desk.top(), None);
        desk.update(Message::Raise(1));
        desk.close(1);
        assert_eq!(desk.top(), Some(2));
        desk.close(2);
        assert_eq!(desk.top(), None);
    }

    #[test]
    fn close_is_the_apps_to_carry_out() {
        let mut desk = desk();
        assert_eq!(desk.update(Message::Close(1)), Some(Event::Close(1)));
        assert!(desk.contains(1));
        desk.close(1);
        assert!(!desk.contains(1));
    }
}
