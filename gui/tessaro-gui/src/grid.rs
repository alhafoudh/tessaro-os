//! The one table every list in the GUI is drawn with: a header, then rows
//! of single-line cells in fixed columns. Click selects a row, double-click
//! activates it (opens, edits), right-click selects it and offers Copy of
//! the cell's text (`copy_menu.rs`). iced's own `table` cannot mark a
//! selected row, so this is rows of containers in a scrollable.

use iced::widget::{column, container, mouse_area, row, scrollable, text, Column};
use iced::{Element, Length};

use crate::copy_menu::copy_menu;
use crate::theme;

pub struct Col {
    pub title: &'static str,
    pub width: Length,
}

pub const fn col(title: &'static str, width: Length) -> Col {
    Col { title, width }
}

/// `rows` are the cells of each row, in column order.
pub fn grid<'a, M: Clone + 'a>(
    columns: &[Col],
    rows: Vec<Vec<Element<'a, M>>>,
    selected: Option<usize>,
    on_select: impl Fn(usize) -> M,
    on_activate: impl Fn(usize) -> M,
) -> Element<'a, M> {
    let body = rows.into_iter().enumerate().map(|(at, cells)| {
        mouse_area(
            container(cells_row(columns, cells))
                .width(Length::Fill)
                .style(theme::table_row(selected == Some(at), at % 2 == 1)),
        )
        .on_press(on_select(at))
        .on_right_press(on_select(at))
        .on_double_click(on_activate(at))
        .into()
    });
    frame(columns, Column::with_children(body), false)
}

/// A table nothing in is selected, kept scrolled to its end while `follow`
/// is on and the view is already at the end: the journal.
pub fn grid_following<'a, M: Clone + 'a>(
    columns: &[Col],
    rows: Vec<Vec<Element<'a, M>>>,
    follow: bool,
) -> Element<'a, M> {
    let body = rows.into_iter().enumerate().map(|(at, cells)| {
        container(cells_row(columns, cells))
            .width(Length::Fill)
            .style(theme::table_row(false, at % 2 == 1))
            .into()
    });
    frame(columns, Column::with_children(body), follow)
}

fn cells_row<'a, M: 'a>(columns: &[Col], cells: Vec<Element<'a, M>>) -> Element<'a, M> {
    row(cells.into_iter().zip(columns).map(|(cell, column)| {
        copy_menu(
            container(cell)
                .width(column.width)
                .padding([2, 6])
                .clip(true),
        )
    }))
    .into()
}

fn frame<'a, M: 'a>(columns: &[Col], body: Column<'a, M>, follow: bool) -> Element<'a, M> {
    let header = row(columns.iter().map(|column| {
        container(text(column.title).size(theme::SMALL).font(bold()))
            .width(column.width)
            .padding([3, 6])
            .into()
    }));
    let mut body = scrollable(body).height(Length::Fill);
    if follow {
        body = body.anchor_bottom();
    }
    container(column![
        container(header)
            .width(Length::Fill)
            .style(theme::table_header),
        body,
    ])
    .height(Length::Fill)
    .style(theme::panel)
    .into()
}

/// A cell's text: small and on one line, clipped by its column.
pub fn cell<'a>(value: impl text::IntoFragment<'a>) -> text::Text<'a> {
    text(value)
        .size(theme::SMALL)
        .wrapping(text::Wrapping::None)
}

pub fn bold() -> iced::Font {
    iced::Font {
        weight: iced::font::Weight::Bold,
        ..theme::FONT
    }
}
