//! Modal dialogs: a framed box over the dimmed window, which takes no input
//! while the dialog is up.

use std::sync::atomic::{AtomicU64, Ordering};

use iced::widget::{center, column, container, opaque, operation, row, space, stack, text, Id};
use iced::{Element, Length, Task};

use crate::grid::bold;
use crate::theme;

/// The ids of a window's dialog fields. Every inner window shares one widget
/// tree, where the same id twice would take the focus in both, so each
/// window numbers its own.
#[derive(Debug)]
pub struct Fields(u64);

impl Default for Fields {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

impl Fields {
    /// The id of the dialog's field `at`.
    pub fn id(&self, at: usize) -> Id {
        Id::from(format!("dialog:{}:{at}", self.0))
    }

    /// Puts the cursor in field `at`, so typing goes there without a click.
    /// Asked once, as the dialog opens: a field clicked into afterwards
    /// keeps the focus.
    pub fn focus<T>(&self, at: usize) -> Task<T> {
        operation::focus(self.id(at))
    }
}

/// `dialog` over `base`.
pub fn modal<'a, M: Clone + 'a>(base: Element<'a, M>, dialog: Element<'a, M>) -> Element<'a, M> {
    stack![base, opaque(center(opaque(dialog)).style(theme::backdrop))].into()
}

/// A dialog box: a title strip, the body, and the buttons bottom right.
pub fn frame<'a, M: Clone + 'a>(
    title: String,
    body: Element<'a, M>,
    buttons: Vec<Element<'a, M>>,
) -> Element<'a, M> {
    frame_sized(title, body, buttons, 480.0)
}

/// `frame`, `width` wide: room for a document.
pub fn frame_sized<'a, M: Clone + 'a>(
    title: String,
    body: Element<'a, M>,
    buttons: Vec<Element<'a, M>>,
    width: f32,
) -> Element<'a, M> {
    container(column![
        container(text(title).size(theme::TEXT).font(bold()))
            .width(Length::Fill)
            .padding([4, 10])
            .style(theme::title_bar),
        container(body).padding(12),
        container(row(buttons).spacing(6))
            .width(Length::Fill)
            .padding([8, 12])
            .align_right(Length::Fill),
    ])
    .width(width)
    .style(theme::panel)
    .into()
}

/// A labelled line in a dialog body: `label` in a fixed column, then the
/// field.
pub fn field<'a, M: 'a>(label: &'a str, field: impl Into<Element<'a, M>>) -> Element<'a, M> {
    row![
        container(text(label).size(theme::SMALL)).width(110),
        field.into()
    ]
    .spacing(6)
    .align_y(iced::alignment::Vertical::Center)
    .into()
}

/// A dialog's error line; nothing when there is none.
pub fn error<'a, M: 'a>(error: Option<String>) -> Element<'a, M> {
    match error {
        Some(error) => text(error).size(theme::SMALL).style(text::danger).into(),
        None => space().into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_windows_never_share_a_field() {
        let (a, b) = (Fields::default(), Fields::default());
        assert_ne!(a.id(0), b.id(0));
        assert_eq!(a.id(0), a.id(0));
    }
}
