//! Shared iced_table tables. Widths and sorting are saved per table and device;
//! cells retain their tooltips, double-click actions and right-click Copy.

use std::cell::RefCell;
use std::cmp::Ordering as Compare;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{self, tree, Operation, Tree, Widget};
use iced::advanced::{mouse, overlay, renderer, Clipboard, Shell};
use iced::widget::{container, mouse_area, row, rule, scrollable, sensor, text};
use iced::{Element, Length, Rectangle, Renderer, Size, Task, Theme, Vector};
use ouroboros::self_referencing;

use crate::copy_menu::copy_menu;
use crate::theme;

const MIN_WIDTH: f32 = 18.0;

pub type Preferences = BTreeMap<String, TablePreferences>;

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(default)]
pub struct TablePreferences {
    widths: BTreeMap<usize, f32>,
    sort: Option<Sort>,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
struct Sort {
    column: usize,
    descending: bool,
}

#[derive(Clone, Copy)]
pub struct Col {
    pub title: &'static str,
    pub width: Length,
}

pub const fn col(title: &'static str, width: Length) -> Col {
    Col { title, width }
}

/// A separate set per window/device; names keep widths across page switches.
pub struct Tables {
    id: u64,
    states: BTreeMap<String, State>,
    preferences: Preferences,
    orders: RefCell<BTreeMap<String, Rc<RefCell<Vec<usize>>>>>,
}

impl Default for Tables {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            states: BTreeMap::new(),
            preferences: BTreeMap::new(),
            orders: RefCell::new(BTreeMap::new()),
        }
    }
}

#[derive(Clone)]
pub struct State {
    header: widget::Id,
    body: widget::Id,
    widths: BTreeMap<usize, f32>,
    dragging: Option<(usize, f32, f32)>,
    available: f32,
    sort: Option<Sort>,
    order: Rc<RefCell<Vec<usize>>>,
}

#[derive(Debug, Clone)]
pub enum Event {
    Drag {
        column: usize,
        width: f32,
        offset: f32,
    },
    Release,
    Sort(usize),
    Scroll(scrollable::AbsoluteOffset),
    Size(Size),
}

impl Event {
    pub fn remember(&self) -> bool {
        matches!(self, Self::Release | Self::Sort(_))
    }
}

impl Tables {
    pub fn new(mut preferences: Preferences) -> Self {
        for table in preferences.values_mut() {
            table
                .widths
                .retain(|_, width| width.is_finite() && *width >= MIN_WIDTH);
        }
        Self {
            preferences,
            ..Self::default()
        }
    }

    pub fn preferences(&self) -> Preferences {
        let mut preferences = self.preferences.clone();
        for (name, state) in &self.states {
            preferences.insert(
                name.clone(),
                TablePreferences {
                    widths: state.widths.clone(),
                    sort: state.sort,
                },
            );
        }
        preferences
    }

    /// The view records original row indices after sorting. Keyboard actions use
    /// the same permutation; click callbacks already carry the original index.
    pub fn ordered<T: Clone>(&self, name: &str, rows: &[T]) -> Vec<T> {
        let orders = self.orders.borrow();
        let Some(order) = orders.get(name) else {
            return rows.to_vec();
        };
        let order = order.borrow();
        if order.len() != rows.len() {
            return rows.to_vec();
        }
        order.iter().map(|&at| rows[at].clone()).collect()
    }

    pub fn state(&self, name: &str) -> State {
        self.states.get(name).cloned().unwrap_or_else(|| State {
            header: widget::Id::from(format!("table:{}:{name}:header", self.id)),
            body: widget::Id::from(format!("table:{}:{name}:body", self.id)),
            widths: self
                .preferences
                .get(name)
                .map(|prefs| prefs.widths.clone())
                .unwrap_or_default(),
            dragging: None,
            available: 800.0,
            sort: self.preferences.get(name).and_then(|prefs| prefs.sort),
            order: self
                .orders
                .borrow_mut()
                .entry(name.to_owned())
                .or_default()
                .clone(),
        })
    }

    pub fn update<M: Send + 'static>(&mut self, name: &str, event: Event) -> Task<M> {
        let initial = self.state(name);
        let state = self.states.entry(name.to_owned()).or_insert(initial);
        match event {
            Event::Drag {
                column,
                width,
                offset,
            } => state.dragging = Some((column, width, offset)),
            Event::Release => {
                if let Some((column, width, offset)) = state.dragging.take() {
                    state.widths.insert(column, (width + offset).max(MIN_WIDTH));
                }
            }
            Event::Sort(column) => {
                state.sort = match state.sort {
                    Some(sort) if sort.column == column && sort.descending => None,
                    Some(sort) if sort.column == column => Some(Sort {
                        column,
                        descending: true,
                    }),
                    _ => Some(Sort {
                        column,
                        descending: false,
                    }),
                };
            }
            Event::Size(size) => state.available = size.width,
            Event::Scroll(offset) => {
                return iced::widget::operation::scroll_to(state.header.clone(), offset)
            }
        }
        Task::none()
    }
}

impl State {
    fn columns<'a, M>(&self, columns: &[Col]) -> Vec<TableColumn<'a, M>> {
        let override_width = |at: usize| {
            self.dragging
                .filter(|(column, _, _)| *column == at)
                .map(|(_, width, offset)| (width + offset).max(MIN_WIDTH))
                .or_else(|| self.widths.get(&at).copied())
        };
        let fixed: f32 = columns
            .iter()
            .enumerate()
            .map(|(at, col)| {
                override_width(at).unwrap_or(match col.width {
                    Length::Fixed(width) => width,
                    _ => 0.0,
                })
            })
            .sum();
        let portions: u16 = columns
            .iter()
            .enumerate()
            .filter(|(at, _)| override_width(*at).is_none())
            .map(|(_, col)| col.width.fill_factor())
            .sum();
        // Leave space for the body's vertical scrollbar.
        let unit = ((self.available - fixed - 12.0) / f32::from(portions.max(1))).max(0.0);
        columns
            .iter()
            .enumerate()
            .map(|(at, col)| {
                let width = self
                    .widths
                    .get(&at)
                    .copied()
                    .unwrap_or_else(|| match col.width {
                        Length::Fixed(width) => width,
                        _ => (unit * f32::from(col.width.fill_factor())).max(80.0),
                    });
                let (width, offset) = self
                    .dragging
                    .filter(|(index, _, _)| *index == at)
                    .map(|(_, width, offset)| (width, Some(offset)))
                    .unwrap_or((width, None));
                TableColumn {
                    title: col.title,
                    width,
                    offset,
                    cells: Vec::new(),
                    sort: self.sort.filter(|sort| sort.column == at),
                }
            })
            .collect()
    }
}

/// `rows` are the cells of each row, in column order.
pub fn grid<'a, M: Clone + 'a>(
    state: State,
    on_change: impl Fn(Event) -> M + 'a,
    columns: &[Col],
    rows: Vec<Vec<Cell<'a, M>>>,
    selected: Option<usize>,
    on_select: impl Fn(usize) -> M,
    on_activate: impl Fn(usize) -> M,
) -> Element<'a, M> {
    let rows = rows
        .into_iter()
        .enumerate()
        .map(|(at, cells)| {
            cells
                .into_iter()
                .map(|cell| {
                    cell.map(|content| {
                        mouse_area(copy_menu(
                            container(content)
                                .width(Length::Fill)
                                .padding([2, 6])
                                .clip(true),
                        ))
                        .on_press(on_select(at))
                        .on_right_press(on_select(at))
                        .on_double_click(on_activate(at))
                        .into()
                    })
                })
                .collect()
        })
        .collect();
    frame(state, on_change, columns, rows, selected, false)
}

/// Follow the journal only while it was already scrolled to the bottom.
pub fn grid_following<'a, M: Clone + 'a>(
    state: State,
    on_change: impl Fn(Event) -> M + 'a,
    columns: &[Col],
    rows: Vec<Vec<Cell<'a, M>>>,
    follow: bool,
) -> Element<'a, M> {
    let rows = rows
        .into_iter()
        .map(|cells| {
            cells
                .into_iter()
                .map(|cell| {
                    cell.map(|content| {
                        copy_menu(
                            container(content)
                                .width(Length::Fill)
                                .padding([2, 6])
                                .clip(true),
                        )
                    })
                })
                .collect()
        })
        .collect();
    frame(state, on_change, columns, rows, None, follow)
}

#[derive(Clone)]
enum TableEvent<M> {
    Cell(M),
    Drag(usize, f32),
    Release,
    Sort(usize),
    Scroll(scrollable::AbsoluteOffset),
    Size(Size),
}

struct TableColumn<'a, M> {
    title: &'static str,
    width: f32,
    offset: Option<f32>,
    cells: Vec<RefCell<Option<Element<'a, TableEvent<M>>>>>,
    sort: Option<Sort>,
}

impl<'a, 'cell: 'a, M: Clone + 'cell> iced_table::table::Column<'a, TableEvent<M>, Theme, Renderer>
    for TableColumn<'cell, M>
{
    type Row = ();

    fn header(&'a self, index: usize) -> Element<'a, TableEvent<M>> {
        let title = match self.sort {
            Some(Sort {
                descending: false, ..
            }) => format!("{} ↑", self.title),
            Some(_) => format!("{} ↓", self.title),
            None => self.title.to_owned(),
        };
        let label = container(
            text(title)
                .size(theme::SMALL)
                .font(bold())
                .wrapping(text::Wrapping::None),
        )
        .width(Length::Fill)
        .padding([3, 6])
        .clip(true);
        let separator = rule::vertical(1).style(|_| rule::Style {
            color: theme::BORDER,
            radius: 0.into(),
            fill_mode: rule::FillMode::Full,
            snap: true,
        });
        let header: Element<'a, TableEvent<M>> =
            container(row![label, separator].height(Length::Shrink))
                .width(Length::Fill)
                .style(theme::table_header)
                .into();
        // Keep this inside the crate's divider so resize presses never sort.
        if self.title.is_empty() {
            header
        } else {
            mouse_area(header)
                .on_press(TableEvent::Sort(index))
                .interaction(mouse::Interaction::Pointer)
                .into()
        }
    }

    fn cell(&'a self, _: usize, row: usize, _: &'a Self::Row) -> Element<'a, TableEvent<M>> {
        // iced_table materializes each cell exactly once when converted to Element.
        self.cells[row]
            .borrow_mut()
            .take()
            .expect("table cell built once")
    }

    fn width(&self) -> f32 {
        self.width
    }
    fn resize_offset(&self) -> Option<f32> {
        self.offset
    }
}

struct Data<'a, M> {
    state: State,
    columns: Vec<TableColumn<'a, M>>,
    rows: Vec<()>,
    on_change: Box<dyn Fn(Event) -> M + 'a>,
}

// The crate borrows its columns and rows even though our cells are owned.
// Keep those inputs with the resulting widget, without leaking allocations.
#[self_referencing]
struct OwnedTable<'a, M: Clone + 'a> {
    data: Data<'a, M>,
    #[borrows(data)]
    #[covariant]
    content: Element<'this, M>,
}

fn frame<'a, M: Clone + 'a>(
    state: State,
    on_change: impl Fn(Event) -> M + 'a,
    columns: &[Col],
    rows: Vec<Vec<Cell<'a, M>>>,
    selected: Option<usize>,
    follow: bool,
) -> Element<'a, M> {
    let body = state.body.clone();
    let follow = follow && state.sort.is_none();
    let mut rows: Vec<_> = rows.into_iter().enumerate().collect();
    if let Some(sort) = state.sort.filter(|sort| sort.column < columns.len()) {
        rows.sort_by(|(_, left), (_, right)| {
            let order = left[sort.column].value.compare(&right[sort.column].value);
            if sort.descending {
                order.reverse()
            } else {
                order
            }
        });
    }
    *state.order.borrow_mut() = rows.iter().map(|(at, _)| *at).collect();
    let mut columns = state.columns(columns);
    let row_count = rows.len();
    for (at, cells) in rows {
        assert_eq!(cells.len(), columns.len(), "one cell per column");
        // iced_table styles a row by its index alone, so the selected row's
        // own cells carry the selection color.
        let chosen = Some(at) == selected;
        for (column, cell) in columns.iter_mut().zip(cells) {
            let mut cell = cell.content;
            if chosen {
                cell = container(cell)
                    .width(Length::Fill)
                    .style(theme::table_selected)
                    .into();
            }
            column
                .cells
                .push(RefCell::new(Some(cell.map(TableEvent::Cell))));
        }
    }
    let data = Data {
        columns,
        state,
        rows: vec![(); row_count],
        on_change: Box::new(on_change),
    };
    let table = OwnedTableBuilder {
        data,
        content_builder: |data| {
            let table = iced_table::table(
                data.state.header.clone(),
                data.state.body.clone(),
                &data.columns,
                &data.rows,
                TableEvent::Scroll,
            )
            .on_column_resize(TableEvent::Drag, TableEvent::Release)
            .min_column_width(MIN_WIDTH)
            .cell_padding(0)
            // Header cells draw a permanent separator. Keep the crate's drag
            // hit area, without reserving divider strips in the body rows.
            .divider_width(0.0);
            let table: Element<'_, TableEvent<M>> = table.into();
            let table: Element<'_, TableEvent<M>> =
                iced::widget::themer(Some(theme::table_theme()), table).into();
            let table = sensor(
                container(table)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(theme::panel),
            )
            .on_show(TableEvent::Size)
            .on_resize(TableEvent::Size);
            Element::from(table).map(move |event| match event {
                TableEvent::Cell(message) => message,
                TableEvent::Drag(column, offset) => (data.on_change)(Event::Drag {
                    column,
                    width: data.columns[column].width,
                    offset,
                }),
                TableEvent::Release => (data.on_change)(Event::Release),
                TableEvent::Sort(column) => (data.on_change)(Event::Sort(column)),
                TableEvent::Scroll(offset) => (data.on_change)(Event::Scroll(offset)),
                TableEvent::Size(size) => (data.on_change)(Event::Size(size)),
            })
        },
    }
    .build();
    Element::new(TableWidget {
        table,
        body,
        follow,
    })
}

struct TableWidget<'a, M: Clone + 'a> {
    table: OwnedTable<'a, M>,
    body: widget::Id,
    follow: bool,
}

#[derive(Default)]
struct Follow {
    body: Option<widget::Id>,
    max_y: Option<f32>,
}

impl<M: Clone> Widget<M, Theme, Renderer> for TableWidget<'_, M> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<Follow>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(Follow {
            body: Some(self.body.clone()),
            max_y: None,
        })
    }
    fn children(&self) -> Vec<Tree> {
        self.table.with_content(|content| vec![Tree::new(content)])
    }
    fn diff(&self, tree: &mut Tree) {
        if tree.state.downcast_ref::<Follow>().body.as_ref() != Some(&self.body) {
            tree.state = self.state();
            tree.children = self.children();
            return;
        }
        self.table
            .with_content(|content| tree.diff_children(std::slice::from_ref(content)));
    }
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }
    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.table.with_content_mut(|content| {
            let node = content
                .as_widget_mut()
                .layout(&mut tree.children[0], renderer, limits);
            let mut follow = FollowScroll {
                body: &self.body,
                follow: self.follow,
                state: tree.state.downcast_mut::<Follow>(),
            };
            content.as_widget_mut().operate(
                &mut tree.children[0],
                Layout::new(&node),
                renderer,
                &mut follow,
            );
            node
        })
    }
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &iced::Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        self.table.with_content_mut(|content| {
            content.as_widget_mut().update(
                &mut tree.children[0],
                event,
                layout,
                cursor,
                renderer,
                clipboard,
                shell,
                viewport,
            )
        });
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
        self.table.with_content(|content| {
            content.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                layout,
                cursor,
                viewport,
            )
        });
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.table.with_content(|content| {
            content.as_widget().mouse_interaction(
                &tree.children[0],
                layout,
                cursor,
                viewport,
                renderer,
            )
        })
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.table.with_content_mut(|content| {
            content
                .as_widget_mut()
                .operate(&mut tree.children[0], layout, renderer, operation)
        });
    }
    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, M, Theme, Renderer>> {
        self.table.with_content_mut(|content| {
            content.as_widget_mut().overlay(
                &mut tree.children[0],
                layout,
                renderer,
                viewport,
                translation,
            )
        })
    }
}

struct FollowScroll<'a> {
    body: &'a widget::Id,
    follow: bool,
    state: &'a mut Follow,
}

impl Operation for FollowScroll<'_> {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }
    fn scrollable(
        &mut self,
        id: Option<&widget::Id>,
        bounds: Rectangle,
        content_bounds: Rectangle,
        translation: Vector,
        state: &mut dyn widget::operation::Scrollable,
    ) {
        if id != Some(self.body) {
            return;
        }
        let max_y = (content_bounds.height - bounds.height).max(0.0);
        let was_at_bottom = self
            .state
            .max_y
            .is_none_or(|previous| translation.y >= previous - 1.0);
        if self.follow && was_at_bottom {
            state.scroll_to(scrollable::AbsoluteOffset {
                x: None,
                y: Some(max_y),
            });
        }
        self.state.max_y = Some(max_y);
    }
}

/// A cell keeps its sortable value separate from its widgets (including tooltips).
pub struct Cell<'a, M> {
    value: SortValue,
    content: Element<'a, M>,
}

impl<'a, M> Cell<'a, M> {
    pub fn map(self, wrap: impl FnOnce(Element<'a, M>) -> Element<'a, M>) -> Self {
        Self {
            value: self.value,
            content: wrap(self.content),
        }
    }
}

pub struct CellText<'a> {
    value: SortValue,
    text: text::Text<'a>,
}

impl<'a> CellText<'a> {
    pub fn style(mut self, style: impl Fn(&Theme) -> text::Style + 'a) -> Self {
        self.text = self.text.style(style);
        self
    }

    pub fn font(mut self, font: iced::Font) -> Self {
        self.text = self.text.font(font);
        self
    }

    /// Use the underlying number for formatted sizes, percentages and dates.
    pub fn sort_number(mut self, number: f64) -> Self {
        self.value = SortValue::Number(number);
        self
    }
}

impl<'a, M: 'a> From<CellText<'a>> for Cell<'a, M> {
    fn from(cell: CellText<'a>) -> Self {
        Self {
            value: cell.value,
            content: cell.text.into(),
        }
    }
}

impl<'a, M: 'a> From<CellText<'a>> for Element<'a, M> {
    fn from(cell: CellText<'a>) -> Self {
        cell.text.into()
    }
}

/// Small, single-line text with a natural, case-insensitive sorting key.
pub fn cell<'a>(value: impl Into<String>) -> CellText<'a> {
    let value = value.into();
    CellText {
        text: text(value.clone())
            .size(theme::SMALL)
            .wrapping(text::Wrapping::None),
        value: value
            .parse::<f64>()
            .map(SortValue::Number)
            .unwrap_or_else(|_| SortValue::Text(value.to_lowercase())),
    }
}

enum SortValue {
    Text(String),
    Number(f64),
}

impl SortValue {
    fn compare(&self, other: &Self) -> Compare {
        match (self, other) {
            (Self::Number(a), Self::Number(b)) => a.total_cmp(b),
            (Self::Text(a), Self::Text(b)) => natural_cmp(a, b),
            (Self::Text(_), Self::Number(_)) => Compare::Less,
            (Self::Number(_), Self::Text(_)) => Compare::Greater,
        }
    }
}

fn natural_cmp(mut a: &str, mut b: &str) -> Compare {
    while !a.is_empty() && !b.is_empty() {
        if a.as_bytes()[0].is_ascii_digit() && b.as_bytes()[0].is_ascii_digit() {
            let an = a.bytes().take_while(u8::is_ascii_digit).count();
            let bn = b.bytes().take_while(u8::is_ascii_digit).count();
            let ad = a[..an].trim_start_matches('0');
            let bd = b[..bn].trim_start_matches('0');
            let order = ad.len().cmp(&bd.len()).then_with(|| ad.cmp(bd));
            if order != Compare::Equal {
                return order;
            }
            a = &a[an..];
            b = &b[bn..];
        } else {
            let ac = a.chars().next().unwrap();
            let bc = b.chars().next().unwrap();
            let order = ac.cmp(&bc);
            if order != Compare::Equal {
                return order;
            }
            a = &a[ac.len_utf8()..];
            b = &b[bc.len_utf8()..];
        }
    }
    a.len().cmp(&b.len())
}

pub use crate::theme::bold;

#[cfg(test)]
mod tests {
    use super::*;
    use iced::advanced::renderer::Headless;
    use iced::{Event as Input, Point};

    const COLUMNS: &[Col] = &[
        col("Name", Length::Fixed(100.0)),
        col("Value", Length::Fill),
    ];

    #[derive(Debug, Clone)]
    enum Message {
        Table(Event),
        Select(usize),
        Activate(usize),
    }

    fn renderer() -> Renderer {
        iced::futures::executor::block_on(Renderer::new(
            theme::FONT,
            theme::SMALL.into(),
            Some("tiny-skia"),
        ))
        .expect("software renderer")
    }

    fn view(tables: &Tables, name: &str) -> Element<'static, Message> {
        grid(
            tables.state(name),
            Message::Table,
            COLUMNS,
            vec![vec![
                cell("one").into(),
                cell("a long value to copy").into(),
            ]],
            Some(0),
            Message::Select,
            Message::Activate,
        )
    }

    fn layout(
        element: &mut Element<'_, Message>,
        tree: &mut Tree,
        renderer: &Renderer,
    ) -> layout::Node {
        element.as_widget_mut().layout(
            tree,
            renderer,
            &layout::Limits::new(Size::ZERO, Size::new(500.0, 200.0)),
        )
    }

    fn input(
        element: &mut Element<'_, Message>,
        tree: &mut Tree,
        renderer: &Renderer,
        node: &layout::Node,
        position: Point,
        event: mouse::Event,
    ) -> Vec<Message> {
        let mut messages = Vec::new();
        element.as_widget_mut().update(
            tree,
            &Input::Mouse(event),
            Layout::new(node),
            mouse::Cursor::Available(position),
            renderer,
            &mut iced::advanced::clipboard::Null,
            &mut Shell::new(&mut messages),
            &Rectangle::with_size(Size::new(500.0, 200.0)),
        );
        messages
    }

    #[test]
    fn dragging_commits_width_and_keeps_other_tables_independent() {
        let renderer = renderer();
        let mut tables = Tables::default();
        let other_device = Tables::default();
        let mut table = view(&tables, "first");
        let mut tree = Tree::new(&table);
        let node = layout(&mut table, &mut tree, &renderer);
        let origin = Point::new(99.0, 10.0);
        let messages = input(
            &mut table,
            &mut tree,
            &renderer,
            &node,
            origin,
            mouse::Event::ButtonPressed(mouse::Button::Left),
        );
        assert!(!messages
            .iter()
            .any(|message| matches!(message, Message::Table(Event::Sort(_)))));
        let end = Point::new(149.0, 10.0);
        let messages = input(
            &mut table,
            &mut tree,
            &renderer,
            &node,
            end,
            mouse::Event::CursorMoved { position: end },
        );
        assert!(messages.iter().any(|message| matches!(
            message,
            Message::Table(Event::Drag {
                column: 0,
                offset: 50.0,
                ..
            })
        )));
        for message in messages {
            if let Message::Table(event) = message {
                let _ = tables.update::<Message>("first", event);
            }
        }
        // A fresh view on every drag event must retain the divider's drag origin.
        table = view(&tables, "first");
        tree.diff(table.as_widget());
        let node = layout(&mut table, &mut tree, &renderer);
        let messages = input(
            &mut table,
            &mut tree,
            &renderer,
            &node,
            end,
            mouse::Event::ButtonReleased(mouse::Button::Left),
        );
        assert!(messages
            .iter()
            .any(|message| matches!(message, Message::Table(Event::Release))));
        for message in messages {
            if let Message::Table(event) = message {
                let _ = tables.update::<Message>("first", event);
            }
        }
        assert_eq!(
            tables.state("first").columns::<Message>(COLUMNS)[0].width,
            150.0
        );
        assert_eq!(
            tables.state("second").columns::<Message>(COLUMNS)[0].width,
            100.0
        );
        assert_ne!(tables.state("first").body, other_device.state("first").body);
        let _ = tables.update::<Message>(
            "first",
            Event::Drag {
                column: 0,
                width: 150.0,
                offset: -500.0,
            },
        );
        let _ = tables.update::<Message>("first", Event::Release);
        assert_eq!(
            tables.state("first").columns::<Message>(COLUMNS)[0].width,
            MIN_WIDTH
        );
    }

    #[test]
    fn cells_still_select_activate_and_open_copy_overlay() {
        let renderer = renderer();
        let mut table = view(&Tables::default(), "first");
        let mut tree = Tree::new(&table);
        let node = layout(&mut table, &mut tree, &renderer);
        let position = Point::new(30.0, 32.0);
        let click = mouse::Event::ButtonPressed(mouse::Button::Left);
        let messages = input(&mut table, &mut tree, &renderer, &node, position, click);
        assert!(messages
            .iter()
            .any(|message| matches!(message, Message::Select(0))));
        let messages = input(&mut table, &mut tree, &renderer, &node, position, click);
        assert!(messages
            .iter()
            .any(|message| matches!(message, Message::Activate(0))));
        let messages = input(
            &mut table,
            &mut tree,
            &renderer,
            &node,
            position,
            mouse::Event::ButtonPressed(mouse::Button::Right),
        );
        assert!(messages
            .iter()
            .any(|message| matches!(message, Message::Select(0))));
        assert!(table
            .as_widget_mut()
            .overlay(
                &mut tree,
                Layout::new(&node),
                &renderer,
                &Rectangle::with_size(Size::new(500.0, 200.0)),
                Vector::ZERO
            )
            .is_some());
    }

    #[derive(Default)]
    struct ScrollPositions(Vec<(widget::Id, Vector, f32)>);

    impl Operation for ScrollPositions {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }
        fn scrollable(
            &mut self,
            id: Option<&widget::Id>,
            bounds: Rectangle,
            content_bounds: Rectangle,
            translation: Vector,
            _: &mut dyn widget::operation::Scrollable,
        ) {
            if let Some(id) = id {
                self.0.push((
                    id.clone(),
                    translation,
                    (content_bounds.height - bounds.height).max(0.0),
                ));
            }
        }
    }

    #[test]
    fn journal_follows_new_rows_but_leaves_scrolled_back_content_alone() {
        let renderer = renderer();
        let tables = Tables::default();
        let body = tables.state("journal").body;
        let view = |count, follow| {
            grid_following(
                tables.state("journal"),
                Message::Table,
                COLUMNS,
                (0..count)
                    .map(|at| vec![cell(at.to_string()).into(), cell("entry").into()])
                    .collect(),
                follow,
            )
        };
        let position = |table: &mut Element<'_, Message>, tree: &mut Tree, node: &layout::Node| {
            let mut positions = ScrollPositions::default();
            table
                .as_widget_mut()
                .operate(tree, Layout::new(node), &renderer, &mut positions);
            let (_, translation, max) = positions
                .0
                .into_iter()
                .find(|(id, _, _)| id == &body)
                .unwrap();
            (translation.y, max)
        };
        let mut table = view(30, true);
        let mut tree = Tree::new(&table);
        let node = layout(&mut table, &mut tree, &renderer);
        let (y, max) = position(&mut table, &mut tree, &node);
        assert!(max > 0.0);
        assert!((y - max).abs() <= 1.0);
        table = view(40, true);
        tree.diff(table.as_widget());
        let node = layout(&mut table, &mut tree, &renderer);
        let (next_y, next_max) = position(&mut table, &mut tree, &node);
        assert!(next_max > max);
        assert!((next_y - next_max).abs() <= 1.0);
        let mut scroll = widget::operation::scrollable::scroll_to::<()>(
            body.clone(),
            scrollable::AbsoluteOffset {
                x: None,
                y: Some(0.0),
            },
        );
        table
            .as_widget_mut()
            .operate(&mut tree, Layout::new(&node), &renderer, &mut scroll);
        table = view(50, true);
        tree.diff(table.as_widget());
        let node = layout(&mut table, &mut tree, &renderer);
        assert_eq!(position(&mut table, &mut tree, &node).0, 0.0);
        // Reusing a widget position for another table must reset its scroll state.
        table = grid_following(
            tables.state("another"),
            Message::Table,
            COLUMNS,
            vec![],
            false,
        );
        tree.diff(table.as_widget());
        assert_eq!(tree.state.downcast_ref::<Follow>().max_y, None);
    }

    #[test]
    fn header_sorting_keeps_callbacks_and_keyboard_order_with_the_displayed_rows() {
        let renderer = renderer();
        let mut tables = Tables::default();
        let values = ["Node 10", "node 2", "node 1"];
        let view = |tables: &Tables| {
            grid(
                tables.state("nodes"),
                Message::Table,
                COLUMNS,
                values
                    .iter()
                    .map(|value| vec![cell(*value).into(), cell("value").into()])
                    .collect(),
                Some(0),
                Message::Select,
                Message::Activate,
            )
        };
        let mut table = view(&tables);
        let mut tree = Tree::new(&table);
        for expected in [[2, 1, 0], [0, 1, 2], [0, 1, 2]] {
            let node = layout(&mut table, &mut tree, &renderer);
            let messages = input(
                &mut table,
                &mut tree,
                &renderer,
                &node,
                Point::new(30.0, 10.0),
                mouse::Event::ButtonPressed(mouse::Button::Left),
            );
            assert!(messages
                .iter()
                .any(|message| matches!(message, Message::Table(Event::Sort(0)))));
            for message in messages {
                if let Message::Table(event) = message {
                    let _ = tables.update::<Message>("nodes", event);
                }
            }
            table = view(&tables);
            tree.diff(table.as_widget());
            let node = layout(&mut table, &mut tree, &renderer);
            assert_eq!(tables.ordered("nodes", &[0, 1, 2]), expected);
            let messages = input(
                &mut table,
                &mut tree,
                &renderer,
                &node,
                Point::new(30.0, 32.0),
                mouse::Event::ButtonPressed(mouse::Button::Right),
            );
            assert!(messages
                .iter()
                .any(|message| matches!(message, Message::Select(at) if *at == expected[0])));
            // Recreate the table owner as on reopening the window or app.
            let saved = serde_json::to_vec(&tables.preferences()).unwrap();
            tables = Tables::new(serde_json::from_slice(&saved).unwrap());
            table = view(&tables);
            tree.diff(table.as_widget());
            assert_eq!(tables.ordered("nodes", &[0, 1, 2]), expected);
        }
        assert_eq!(tables.state("nodes").sort, None);
        assert_eq!(natural_cmp("item2", "item10"), Compare::Less);
        assert_eq!(cell("-20").value.compare(&cell("-3").value), Compare::Less);
        // Formatted sizes use bytes, not the magnitude of their displayed number.
        let small = cell("900 MiB").sort_number(900.0 * 1024.0 * 1024.0);
        let big = cell("2 GiB").sort_number(2.0 * 1024.0 * 1024.0 * 1024.0);
        assert_eq!(small.value.compare(&big.value), Compare::Less);
    }

    #[test]
    fn only_headers_have_visible_column_dividers() {
        let mut renderer = renderer();
        let tables = Tables::default();
        let mut table = grid(
            tables.state("nodes"),
            Message::Table,
            COLUMNS,
            (0..3)
                .map(|_| vec![cell("one").into(), cell("two").into()])
                .collect(),
            Some(0),
            Message::Select,
            Message::Activate,
        );
        let mut tree = Tree::new(&table);
        let node = layout(&mut table, &mut tree, &renderer);
        table.as_widget().draw(
            &tree,
            &mut renderer,
            &theme::theme(),
            &renderer::Style {
                text_color: theme::TEXT_COLOR,
            },
            Layout::new(&node),
            mouse::Cursor::Unavailable,
            &Rectangle::with_size(Size::new(500.0, 200.0)),
        );
        let pixels = renderer.screenshot(Size::new(500, 200), 1.0, theme::BACKGROUND);
        let pixel = |x: usize, y: usize| &pixels[(y * 500 + x) * 4..(y * 500 + x + 1) * 4];
        assert_ne!(
            pixel(90, 10),
            pixel(99, 10),
            "header separator stays visible without hovering"
        );
        for y in [36, 55, 73] {
            for x in [98, 99, 100, 101] {
                assert_eq!(pixel(90, y), pixel(x, y), "body divider at {x},{y}");
            }
        }
    }

    #[test]
    fn fill_columns_preview_the_same_width_they_commit() {
        let mut tables = Tables::default();
        let _ = tables.update::<Message>("first", Event::Size(Size::new(500.0, 200.0)));
        let _ = tables.update::<Message>(
            "first",
            Event::Drag {
                column: 0,
                width: 100.0,
                offset: 50.0,
            },
        );
        let preview = tables.state("first").columns::<Message>(COLUMNS)[1].width;
        let _ = tables.update::<Message>("first", Event::Release);
        assert_eq!(
            tables.state("first").columns::<Message>(COLUMNS)[1].width,
            preview
        );
        let _ = tables.update::<Message>(
            "first",
            Event::Drag {
                column: 1,
                width: preview,
                offset: 30.0,
            },
        );
        let _ = tables.update::<Message>("first", Event::Release);
        let _ = tables.update::<Message>("first", Event::Size(Size::new(700.0, 200.0)));
        assert_eq!(
            tables.state("first").columns::<Message>(COLUMNS)[1].width,
            preview + 30.0
        );
    }
}
