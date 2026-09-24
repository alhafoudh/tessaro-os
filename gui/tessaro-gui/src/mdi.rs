//! Inner windows: the node list and every open device float on a desk in
//! the one app window, WinBox style. A window moves by its title bar,
//! resizes from its bottom-right corner, maximizes (button or double-click
//! on the title) and closes from the title bar. The last one clicked is on
//! top, and it is the one the keyboard talks to.
//!
//! Built from iced's `stack` (one layer per window, in z-order), `pin`
//! (its position) and `opaque` (so a window hides what is under it from the
//! mouse as well as the eye). While a window is dragged, the app listens
//! for the cursor itself: the title bar only says where on it the drag
//! began.

use iced::widget::{column, container, mouse_area, opaque, pin, row, space, stack, text};
use iced::{mouse, Element, Length, Point, Size, Vector};

use crate::grid::bold;
use crate::theme;

pub type Id = u64;

/// The height of the app's own header, above the desk: where the desk
/// begins in window coordinates.
pub const DESK_TOP: f32 = 34.0;
const TITLE: f32 = 24.0;
const GRIP: f32 = 14.0;
const MIN: Size = Size::new(380.0, 220.0);
/// How much of a window stays on the desk however far it is dragged.
const KEEP: f32 = 80.0;
/// Each new window opens this far below and right of the last.
const CASCADE: f32 = 28.0;

#[derive(Debug, Clone)]
struct Frame {
    id: Id,
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
    Maximize(Id),
    Close(Id),
    /// The cursor, in window coordinates, while dragging.
    Cursor(Point),
    Release,
    /// The app window's new size.
    Resized(Size),
}

pub struct Desk {
    /// Bottom to top.
    frames: Vec<Frame>,
    drag: Option<Drag>,
    /// The last cursor position over a title bar or grip, for the drag that
    /// a press there starts.
    over: Vector,
    size: Size,
    opened: u32,
}

impl Desk {
    pub fn new(window: Size) -> Self {
        Self {
            frames: Vec::new(),
            drag: None,
            over: Vector::ZERO,
            size: desk_size(window),
            opened: 0,
        }
    }

    /// A new window, cascaded from the last, on top.
    pub fn open(&mut self, id: Id, size: Size) {
        if self.contains(id) {
            return self.raise(id);
        }
        let step = (self.opened % 8) as f32 * CASCADE;
        self.opened += 1;
        self.frames.push(Frame {
            id,
            position: Point::new(16.0 + step, 12.0 + step),
            size,
            maximized: false,
        });
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
    }

    pub fn raise(&mut self, id: Id) {
        if let Some(at) = self.frames.iter().position(|frame| frame.id == id) {
            let frame = self.frames.remove(at);
            self.frames.push(frame);
        }
    }

    /// The window on top: the one the keyboard talks to.
    pub fn top(&self) -> Option<Id> {
        self.frames.last().map(|frame| frame.id)
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Handle `message`; a window whose close was asked for comes back, for
    /// the app to drop what it shows.
    pub fn update(&mut self, message: Message) -> Option<Id> {
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
            Message::Maximize(id) => {
                self.raise(id);
                if let Some(frame) = self.frame_mut(id) {
                    frame.maximized = !frame.maximized;
                }
            }
            Message::Close(id) => return Some(id),
            Message::Cursor(at) => self.dragged(Point::new(at.x, at.y - DESK_TOP)),
            Message::Release => self.drag = None,
            Message::Resized(window) => self.size = desk_size(window),
        }
        None
    }

    fn dragged(&mut self, at: Point) {
        let desk = self.size;
        match self.drag {
            Some(Drag::Move { id, grab }) => {
                if let Some(frame) = self.frame_mut(id) {
                    let x = (at.x - grab.x).clamp(KEEP - frame.size.width, desk.width - KEEP);
                    let y = (at.y - grab.y).clamp(0.0, (desk.height - TITLE).max(0.0));
                    frame.position = Point::new(x, y);
                }
            }
            Some(Drag::Resize { id, grab }) => {
                if let Some(frame) = self.frame_mut(id) {
                    frame.size = Size::new(
                        (at.x + grab.x - frame.position.x).max(MIN.width),
                        (at.y + grab.y - frame.position.y).max(MIN.height),
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

    /// The desk with every window on it. `window` gives a window's title,
    /// whether it may be closed, and its content.
    pub fn view<'a, M: Clone + 'a>(
        &'a self,
        window: impl Fn(Id) -> (String, bool, Element<'a, M>),
        map: fn(Message) -> M,
    ) -> Element<'a, M> {
        let top = self.top();
        let mut layers = stack![container(space())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::desk)]
        .width(Length::Fill)
        .height(Length::Fill);

        for frame in &self.frames {
            let (title, closable, content) = window(frame.id);
            let focused = top == Some(frame.id);
            let (position, size) = if frame.maximized {
                (Point::ORIGIN, self.size)
            } else {
                (frame.position, frame.size)
            };
            layers = layers.push(
                pin(opaque(self.frame_view(
                    frame, title, closable, focused, content, size, map,
                )))
                .position(position),
            );
        }
        layers.into()
    }

    #[allow(clippy::too_many_arguments)]
    fn frame_view<'a, M: Clone + 'a>(
        &'a self,
        frame: &Frame,
        title: String,
        closable: bool,
        focused: bool,
        content: Element<'a, M>,
        size: Size,
        map: fn(Message) -> M,
    ) -> Element<'a, M> {
        let id = frame.id;
        let mut buttons = row![theme::title_button(
            if frame.maximized { "❐" } else { "□" },
            map(Message::Maximize(id))
        )]
        .spacing(2);
        if closable {
            buttons = buttons.push(theme::title_button("×", map(Message::Close(id))));
        }
        let bar = mouse_area(
            container(
                row![
                    text(title).size(theme::SMALL).font(bold()),
                    space::horizontal(),
                    buttons,
                ]
                .align_y(iced::alignment::Vertical::Center),
            )
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

        let body = column![bar, container(content).height(Length::Fill)];
        let mut layers = stack![body].width(Length::Fill).height(Length::Fill);
        if !frame.maximized {
            let grip = mouse_area(
                container(text("◢").size(10.0).style(theme::muted))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn desk() -> Desk {
        let mut desk = Desk::new(Size::new(1200.0, 800.0 + DESK_TOP));
        desk.open(1, Size::new(500.0, 300.0));
        desk.open(2, Size::new(500.0, 300.0));
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
    fn a_window_moves_where_it_is_held_and_stays_reachable() {
        let mut desk = desk();
        desk.update(Message::OverTitle(Point::new(10.0, 5.0)));
        desk.update(Message::StartMove(1));
        desk.update(Message::Cursor(Point::new(310.0, 205.0 + DESK_TOP)));
        assert_eq!(desk.frame(1).unwrap().position, Point::new(300.0, 200.0));

        desk.update(Message::Cursor(Point::new(-5000.0, -5000.0)));
        let frame = desk.frame(1).unwrap();
        assert_eq!(frame.position, Point::new(KEEP - 500.0, 0.0));
        desk.update(Message::Release);
        assert!(!desk.dragging());
    }

    #[test]
    fn a_window_does_not_shrink_below_its_minimum() {
        let mut desk = desk();
        desk.update(Message::StartResize(2));
        desk.update(Message::Cursor(Point::new(0.0, 0.0)));
        assert_eq!(desk.frame(2).unwrap().size, MIN);
    }

    #[test]
    fn close_is_the_apps_to_carry_out() {
        let mut desk = desk();
        assert_eq!(desk.update(Message::Close(1)), Some(1));
        assert!(desk.contains(1));
        desk.close(1);
        assert!(!desk.contains(1));
    }
}
