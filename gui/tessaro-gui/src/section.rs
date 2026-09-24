//! What every list looks like: a toolbar over a table. The toolbar holds
//! the actions on the whole list first, then the actions on the selected
//! record (disabled while nothing is selected), then the filter. The node
//! list and each settings section of a device are drawn with it, and so is
//! anything that becomes a section later (tokens, SSH keys, profiles):
//! build the `Action`s and the grid, and it looks and works like the rest.

use iced::widget::{column, row, rule, space, text_input};
use iced::{Element, Length};

use crate::theme;

pub struct Action<M> {
    pub label: &'static str,
    /// `None` shows the action disabled.
    pub message: Option<M>,
}

pub fn action<M>(label: &'static str, message: Option<M>) -> Action<M> {
    Action { label, message }
}

pub fn view<'a, M: Clone + 'a>(
    list: Vec<Action<M>>,
    record: Vec<Action<M>>,
    filter: &str,
    on_filter: impl Fn(String) -> M + 'a,
    table: Element<'a, M>,
) -> Element<'a, M> {
    let buttons = |actions: Vec<Action<M>>| {
        row(actions
            .into_iter()
            .map(|action| theme::tool(action.label, action.message)))
        .spacing(4)
    };
    let both = !list.is_empty() && !record.is_empty();
    let mut toolbar = row![buttons(list)].spacing(8);
    if both {
        toolbar = toolbar.push(rule::vertical(1));
    }
    toolbar = toolbar.push(buttons(record));
    let toolbar = toolbar
        .push(space::horizontal())
        .push(
            text_input("Find", filter)
                .on_input(on_filter)
                .size(theme::SMALL)
                .padding([2, 6])
                .width(180),
        )
        .height(24)
        .align_y(iced::alignment::Vertical::Center);

    column![toolbar, table]
        .spacing(4)
        .height(Length::Fill)
        .into()
}

/// The key `by` rows away from `selected` in `keys`, for Up and Down: the
/// first or last row when nothing is selected, and it stops at the ends.
pub fn step<K: Clone + PartialEq>(keys: &[K], selected: Option<&K>, by: i32) -> Option<K> {
    if keys.is_empty() {
        return None;
    }
    let last = keys.len() as i32 - 1;
    let at = match selected.and_then(|selected| keys.iter().position(|key| key == selected)) {
        Some(at) => (at as i32 + by).clamp(0, last),
        None if by < 0 => last,
        None => 0,
    };
    Some(keys[at as usize].clone())
}

/// Whether a row with these cells passes the filter: any cell contains it,
/// ignoring case.
pub fn matches(filter: &str, cells: &[&str]) -> bool {
    let filter = filter.trim().to_lowercase();
    filter.is_empty()
        || cells
            .iter()
            .any(|cell| cell.to_lowercase().contains(&filter))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn up_and_down_stop_at_the_ends() {
        let keys = ["a", "b", "c"];
        assert_eq!(step(&keys, None, 1), Some("a"));
        assert_eq!(step(&keys, None, -1), Some("c"));
        assert_eq!(step(&keys, Some(&"b"), 1), Some("c"));
        assert_eq!(step(&keys, Some(&"c"), 1), Some("c"));
        assert_eq!(step(&keys, Some(&"a"), -1), Some("a"));
        assert_eq!(step::<&str>(&[], None, 1), None);
    }

    #[test]
    fn a_filter_matches_any_cell_ignoring_case() {
        assert!(matches("", &["anything"]));
        assert!(matches("URL", &["browser.url", "https://a"]));
        assert!(matches("a.test", &["x", "https://a.test/"]));
        assert!(!matches("nope", &["browser.url", "https://a"]));
    }
}
