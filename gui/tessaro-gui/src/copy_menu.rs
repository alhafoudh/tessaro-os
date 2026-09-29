//! Right-click Copy on a table cell. The cell's text is read back from the
//! widgets it is drawn with (`Text` reports its string to `operate`), so no
//! view has to hand it over, and the clipboard is written from here, so no
//! page needs a message for it. iced has no context menu of its own.
//! Messages hands its text over instead (`copy_menu_with`): a selection is
//! not something its widget reports.

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Quad};
use iced::advanced::text::{self, Renderer as _, Text};
use iced::advanced::widget::{self, tree, Operation, Tree, Widget};
use iced::advanced::{clipboard, mouse, overlay, Clipboard, Renderer as _, Shell};
use iced::keyboard::{self, key};
use iced::{
    alignment, Border, Element, Event, Length, Pixels, Point, Rectangle, Renderer, Size, Theme,
    Vector,
};

use crate::theme;

const WIDTH: f32 = 90.0;
const HEIGHT: f32 = 22.0;

/// `content`, with a menu offering Copy of its text on right-click.
pub fn copy_menu<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Element<'a, M> {
    Element::new(CopyMenu {
        content: content.into(),
        source: None,
    })
}

/// `content`, with a menu on right-click whose entry and text `source`
/// gives, asked only then: for text the widgets do not report, like a
/// selection.
pub fn copy_menu_with<'a, M: 'a>(
    content: impl Into<Element<'a, M>>,
    source: impl Fn() -> (&'static str, String) + 'a,
) -> Element<'a, M> {
    Element::new(CopyMenu {
        content: content.into(),
        source: Some(Box::new(source)),
    })
}

type Source<'a> = Box<dyn Fn() -> (&'static str, String) + 'a>;

struct CopyMenu<'a, M> {
    content: Element<'a, M>,
    source: Option<Source<'a>>,
}

/// Where the menu is open, in the cell's layout coordinates, its entry and
/// the text it copies.
#[derive(Default)]
struct State {
    open: Option<(Point, &'static str, String)>,
}

impl<M> Widget<M, Theme, Renderer> for CopyMenu<'_, M> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        if let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) = event {
            if let Some(at) = cursor.position_over(layout.bounds()) {
                let (label, text) = match &self.source {
                    Some(source) => source(),
                    None => {
                        let mut collect = Collect::default();
                        self.content.as_widget_mut().operate(
                            &mut tree.children[0],
                            layout,
                            renderer,
                            &mut collect,
                        );
                        ("Copy", collect.text)
                    }
                };
                if !text.is_empty() {
                    tree.state.downcast_mut::<State>().open = Some((at, label, text));
                    shell.invalidate_layout();
                    shell.request_redraw();
                }
            }
        }
        // Not captured: the row under it selects on the same right-click. A
        // text editor answers only the left button, so its selection stays.
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, M, Theme, Renderer>> {
        let (children, state) = (&mut tree.children, &mut tree.state);
        let content = self.content.as_widget_mut().overlay(
            &mut children[0],
            layout,
            renderer,
            viewport,
            translation,
        );
        let state = state.downcast_mut::<State>();
        let menu = match &state.open {
            Some((at, _, _)) => {
                let at = *at + translation;
                Some(overlay::Element::new(Box::new(Menu { state, at })))
            }
            None => None,
        };
        match (content, menu) {
            (None, None) => None,
            (content, menu) => {
                let children = content.into_iter().chain(menu).collect();
                Some(overlay::Group::with_children(children).overlay())
            }
        }
    }
}

/// Every piece of text under a widget, in order, space-separated.
#[derive(Default)]
struct Collect {
    text: String,
}

impl Operation for Collect {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn text(&mut self, _id: Option<&widget::Id>, _bounds: Rectangle, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if !self.text.is_empty() {
            self.text.push(' ');
        }
        self.text.push_str(text);
    }
}

/// The open menu: its one entry, at the right-click.
struct Menu<'b> {
    state: &'b mut State,
    at: Point,
}

impl Menu<'_> {
    fn close<M>(&mut self, shell: &mut Shell<'_, M>) {
        self.state.open = None;
        shell.invalidate_layout();
        shell.request_redraw();
    }
}

impl<M> overlay::Overlay<M, Theme, Renderer> for Menu<'_> {
    fn layout(&mut self, _renderer: &Renderer, bounds: Size) -> layout::Node {
        // Kept inside the window when the click is near its edge.
        let x = self.at.x.min(bounds.width - WIDTH).max(0.0);
        let y = self.at.y.min(bounds.height - HEIGHT).max(0.0);
        layout::Node::new(Size::new(WIDTH, HEIGHT)).move_to(Point::new(x, y))
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        let bounds = layout.bounds();
        let hovered = cursor.is_over(bounds);
        renderer.fill_quad(
            Quad {
                bounds,
                border: Border {
                    color: theme::BORDER,
                    width: 1.0,
                    radius: 2.0.into(),
                },
                ..Quad::default()
            },
            if hovered {
                theme::BUTTON_HOVER
            } else {
                theme::PANEL
            },
        );
        renderer.fill_text(
            Text {
                content: self
                    .state
                    .open
                    .as_ref()
                    .map_or("Copy", |(_, label, _)| label)
                    .to_string(),
                bounds: bounds.size(),
                size: Pixels(theme::SMALL),
                line_height: text::LineHeight::default(),
                font: renderer.default_font(),
                align_x: text::Alignment::Left,
                align_y: alignment::Vertical::Center,
                shaping: text::Shaping::Basic,
                wrapping: text::Wrapping::None,
            },
            Point::new(bounds.x + 8.0, bounds.center_y()),
            if hovered {
                iced::Color::WHITE
            } else {
                theme::TEXT_COLOR
            },
            bounds,
        );
    }

    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
    ) {
        match event {
            Event::Mouse(mouse::Event::CursorMoved { .. }) => shell.request_redraw(),
            Event::Mouse(mouse::Event::ButtonPressed(button)) => {
                if cursor.is_over(layout.bounds()) {
                    if *button == mouse::Button::Left {
                        if let Some((_, _, text)) = self.state.open.take() {
                            clipboard.write(clipboard::Kind::Standard, text);
                        }
                    }
                    shell.capture_event();
                }
                // A click anywhere else closes it and still does what it does.
                self.close(shell);
            }
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(key::Named::Escape),
                ..
            }) => {
                self.close(shell);
                shell.capture_event();
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        if cursor.is_over(layout.bounds()) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_joins_the_text_it_is_shown() {
        let mut collect = Collect::default();
        let bounds = Rectangle::default();
        collect.text(None, bounds, "eth0");
        collect.text(None, bounds, "  ");
        collect.text(None, bounds, " 192.168.1.20/24 ");
        assert_eq!(collect.text, "eth0 192.168.1.20/24");
    }
}
