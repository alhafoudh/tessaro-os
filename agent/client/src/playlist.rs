//! Playlists, what both clients do around their requests: an item built or
//! changed from what was typed, the item change that has to send the whole
//! item, a playlist read from a file, timetable entries from typed days and
//! times, and the spans of time an item takes, read and written the one
//! way. The words are in `describe::playlist`; the types and the checks the
//! device makes are `protocol::playlist`.

use std::path::Path;

use protocol::api::{
    self, PlaylistChange, PlaylistItemAddBody, PlaylistItemRef, PlaylistRef, TimetableEntryChange,
};
use protocol::playlist::{
    check_color, check_item, check_playlist, check_playlist_name, check_timetable, format_clock,
    parse_clock, parse_days, ItemKind, PlaylistInfo, PlaylistItem, PlaylistSpec, TimetableSpec,
    Transition, PLAYLIST_ITEMS_MAX, TRANSITION_MS_DEFAULT,
};
use serde::Deserialize;

use crate::connect::Session;

/// How long a URL or image item stays on screen when no duration is given,
/// seconds.
pub const DURATION_DEFAULT_S: u32 = 10;

/// A span as typed, in milliseconds: a bare number is seconds (`90`,
/// `1.5`), or numbers with units (`500ms`, `10s`, `1m30s`, `2h`, `1d`), or a
/// clock reading (`0:05`, `1:02.5`, `1:00:00`).
pub fn parse_ms(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let refuse = || {
        format!(
            "{text:?} is not a time: seconds, or like 500ms, 1.5s, 1m30s, or a clock reading like 0:05"
        )
    };
    if text.is_empty() {
        return Err(refuse());
    }
    if text.contains(':') {
        return parse_clock_reading(text).ok_or_else(refuse);
    }
    let mut total: u64 = 0;
    let mut rest = text;
    while !rest.is_empty() {
        let number_len = rest
            .find(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
            .unwrap_or(rest.len());
        let (number, after) = rest.split_at(number_len);
        let unit_len = after
            .find(|ch: char| !ch.is_ascii_alphabetic())
            .unwrap_or(after.len());
        let (unit, after) = after.split_at(unit_len);
        let unit_ms = match unit {
            "ms" => 1,
            "s" => 1000,
            "m" => 60_000,
            "h" => 3_600_000,
            "d" => 86_400_000,
            "" if after.is_empty() => 1000,
            _ => return Err(refuse()),
        };
        let ms = decimal_times(number, unit_ms).ok_or_else(refuse)?;
        total = total.checked_add(ms).ok_or_else(refuse)?;
        rest = after;
    }
    Ok(total)
}

/// `number` (digits, at most one `.`) times `unit_ms`, rounded to the
/// millisecond.
fn decimal_times(number: &str, unit_ms: u64) -> Option<u64> {
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    if fraction.contains('.') || fraction.len() > 9 {
        return None;
    }
    let whole: u64 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let mut ms = whole.checked_mul(unit_ms)?;
    if !fraction.is_empty() {
        let scale = 10u128.pow(fraction.len() as u32);
        let digits: u128 = fraction.parse().ok()?;
        let part = (digits * u128::from(unit_ms) + scale / 2) / scale;
        ms = ms.checked_add(u64::try_from(part).ok()?)?;
    }
    Some(ms)
}

/// `M:SS`, `H:MM:SS`, the seconds with a fraction or not.
fn parse_clock_reading(text: &str) -> Option<u64> {
    let parts: Vec<&str> = text.split(':').collect();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [minutes, seconds] => ("0", *minutes, *seconds),
        [hours, minutes, seconds] => (*hours, *minutes, *seconds),
        _ => return None,
    };
    let number = |part: &str| -> Option<u64> {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        part.parse().ok()
    };
    let hours = number(hours)?;
    let minutes = number(minutes)?;
    if parts.len() == 3 && minutes > 59 {
        return None;
    }
    let whole_seconds = seconds.split('.').next().unwrap_or("");
    if whole_seconds.len() != 2 {
        return None;
    }
    let seconds = decimal_times(seconds, 1000)?;
    if seconds >= 60_000 {
        return None;
    }
    hours
        .checked_mul(3_600_000)?
        .checked_add(minutes.checked_mul(60_000)?)?
        .checked_add(seconds)
}

/// A span in whole seconds, as `parse_ms` reads it: `10s`, `1m30s`, `90`.
pub fn parse_seconds(text: &str) -> Result<u32, String> {
    let ms = parse_ms(text)?;
    if !ms.is_multiple_of(1000) {
        return Err(format!("{:?} is not whole seconds", text.trim()));
    }
    u32::try_from(ms / 1000).map_err(|_| format!("{:?} is too long", text.trim()))
}

/// Seconds the way `parse_seconds` reads them back: `10s`, `1m30s`, `2h`.
pub fn format_seconds(seconds: u32) -> String {
    if seconds == 0 {
        return "0s".to_string();
    }
    crate::schedule::format_timeout(u64::from(seconds))
}

/// Milliseconds as a span `parse_ms` reads back: `500ms`, `1.5s`, `10s`,
/// `1m2.5s`.
pub fn format_ms(ms: u64) -> String {
    if ms < 1000 {
        return format!("{ms}ms");
    }
    if ms.is_multiple_of(1000) {
        return match u32::try_from(ms / 1000) {
            Ok(seconds) => format_seconds(seconds),
            Err(_) => format!("{}s", ms / 1000),
        };
    }
    let minutes = ms / 60_000;
    let seconds = seconds_with_fraction(ms % 60_000);
    if minutes == 0 {
        format!("{seconds}s")
    } else {
        format!("{}{seconds}s", format_seconds((minutes * 60) as u32))
    }
}

/// A point in a video as a clock reading `parse_ms` reads back: `0:05`,
/// `1:02.5`, `1:00:00`.
pub fn format_position(ms: u64) -> String {
    let hours = ms / 3_600_000;
    let minutes = ms / 60_000 % 60;
    let seconds = seconds_with_fraction(ms % 60_000);
    let seconds = if seconds.split('.').next().unwrap_or("").len() < 2 {
        format!("0{seconds}")
    } else {
        seconds
    };
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds}")
    } else {
        format!("{minutes}:{seconds}")
    }
}

/// Milliseconds below a minute as seconds, the fraction only when there is
/// one: `5`, `2.5`, `0.25`.
fn seconds_with_fraction(ms: u64) -> String {
    let whole = ms / 1000;
    let fraction = ms % 1000;
    if fraction == 0 {
        return whole.to_string();
    }
    let digits = format!("{fraction:03}");
    format!("{whole}.{}", digits.trim_end_matches('0'))
}

/// How long a transition takes, as typed: milliseconds, a plain number.
pub fn parse_transition_ms(text: &str) -> Result<u32, String> {
    let text = text.trim();
    text.parse::<u32>()
        .map_err(|_| format!("{text:?} is not a transition: milliseconds"))
}

/// What was typed for an item, each field as text: `None` leaves it as it
/// is, an empty string puts it back to its default. Flags are `Some(true)`
/// or `Some(false)` when given.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ItemFields {
    /// `url`, `image` or `video`.
    pub kind: Option<String>,
    pub src: Option<String>,
    /// How long it stays on screen, as `parse_seconds` reads it.
    pub duration: Option<String>,
    /// Where a video starts, as `parse_ms` reads it.
    pub from: Option<String>,
    /// Where a video stops.
    pub to: Option<String>,
    pub sound: Option<bool>,
    /// 0-100; a volume turns the sound on unless `sound` says otherwise.
    pub volume: Option<String>,
    /// `contain`, `cover` or `stretch`.
    pub fit: Option<String>,
    /// `#rrggbb`.
    pub background: Option<String>,
    /// `cut`, `fade` or `slide`; empty for the playlist's.
    pub transition: Option<String>,
    /// Milliseconds, a plain number; empty for the playlist's.
    pub transition_ms: Option<String>,
    pub interactive: Option<bool>,
    /// Seconds without input before an interactive item moves on; one
    /// makes the item interactive unless `interactive` says otherwise.
    pub idle: Option<String>,
    /// How long a URL item waits after it loads, as `parse_ms` reads it.
    pub ready_delay: Option<String>,
    pub bridge: Option<bool>,
}

/// `text`, or `None` when it is empty.
fn optional<T>(
    text: &str,
    parse: impl FnOnce(&str) -> Result<T, String>,
) -> Result<Option<T>, String> {
    if text.trim().is_empty() {
        Ok(None)
    } else {
        parse(text.trim()).map(Some)
    }
}

/// Drop what an item of `kind` cannot carry, so changing an item's kind
/// keeps only what still applies.
fn switch_kind(item: &mut PlaylistItem, kind: ItemKind) {
    item.kind = kind;
    if kind == ItemKind::Video {
        item.duration_s = None;
        item.interactive = false;
        item.idle_s = None;
    } else {
        item.trim_start_ms = None;
        item.trim_end_ms = None;
        item.sound = false;
        item.volume = None;
    }
    if kind != ItemKind::Url {
        item.bridge = false;
        item.ready_delay_ms = None;
    }
}

/// Apply the fields that were given to `item`, leaving the rest. Not
/// checked: `edit_item` and `item_from_fields` check the result.
pub fn apply(item: &mut PlaylistItem, fields: &ItemFields) -> Result<(), String> {
    if let Some(kind) = &fields.kind {
        let kind: ItemKind = kind.trim().parse()?;
        if kind != item.kind {
            switch_kind(item, kind);
        }
    }
    if let Some(src) = &fields.src {
        item.src = src.trim().to_string();
    }
    if let Some(text) = &fields.duration {
        item.duration_s = optional(text, parse_seconds)?;
    }
    if let Some(text) = &fields.from {
        item.trim_start_ms = optional(text, parse_ms)?;
    }
    if let Some(text) = &fields.to {
        item.trim_end_ms = optional(text, parse_ms)?;
    }
    if let Some(text) = &fields.volume {
        item.volume = optional(text, |text| {
            text.parse::<u8>()
                .ok()
                .filter(|volume| *volume <= 100)
                .ok_or_else(|| format!("{text:?} is not a volume: 0 to 100"))
        })?;
        if item.volume.is_some() && fields.sound.is_none() {
            item.sound = true;
        }
    }
    if let Some(sound) = fields.sound {
        item.sound = sound;
        if !sound {
            item.volume = None;
        }
    }
    if let Some(text) = &fields.fit {
        item.fit = optional(text, str::parse)?.unwrap_or_default();
    }
    if let Some(text) = &fields.background {
        item.background = optional(text, |text| {
            check_color(text)?;
            Ok(text.to_ascii_lowercase())
        })?;
    }
    if let Some(text) = &fields.transition {
        item.transition = optional(text, str::parse::<Transition>)?;
    }
    if let Some(text) = &fields.transition_ms {
        item.transition_ms = optional(text, parse_transition_ms)?;
    }
    if let Some(text) = &fields.idle {
        item.idle_s = optional(text, parse_seconds)?;
        if item.idle_s.is_some() && fields.interactive.is_none() {
            item.interactive = true;
        }
    }
    if let Some(interactive) = fields.interactive {
        item.interactive = interactive;
        if !interactive {
            item.idle_s = None;
        }
    }
    if let Some(text) = &fields.ready_delay {
        item.ready_delay_ms = optional(text, |text| {
            let ms = parse_ms(text)?;
            u32::try_from(ms).map_err(|_| format!("{text:?} is too long"))
        })?;
    }
    if let Some(bridge) = fields.bridge {
        item.bridge = bridge;
    }
    if item.kind != ItemKind::Video && item.duration_s.is_none() {
        item.duration_s = Some(DURATION_DEFAULT_S);
    }
    Ok(())
}

/// `item` checked as the device will, `position` from 1 naming it in the
/// message; `None` for one not placed yet.
fn checked(item: PlaylistItem, position: Option<u32>) -> Result<PlaylistItem, String> {
    match position {
        Some(position) => check_item(item, position as usize),
        None => check_item(item, 0).map_err(|err| {
            err.strip_prefix("item 0: ")
                .map_or_else(|| err.clone(), |rest| format!("the new item: {rest}"))
        }),
    }
}

/// A new item from what was typed, checked; it needs a kind and a src.
/// `position` names it in a refusal.
pub fn item_from_fields(
    fields: &ItemFields,
    position: Option<u32>,
) -> Result<PlaylistItem, String> {
    let kind: ItemKind = fields
        .kind
        .as_deref()
        .ok_or("an item is a url, an image or a video")?
        .trim()
        .parse()?;
    if fields
        .src
        .as_deref()
        .is_none_or(|src| src.trim().is_empty())
    {
        return Err(format!("a {} item needs its URL", kind.name()));
    }
    let mut item = PlaylistItem::new(kind, "");
    apply(&mut item, fields)?;
    checked(item, position)
}

/// `item` with the given fields changed, checked; `position` is its place,
/// from 1.
pub fn edit_item(
    item: &PlaylistItem,
    fields: &ItemFields,
    position: u32,
) -> Result<PlaylistItem, String> {
    let mut item = item.clone();
    apply(&mut item, fields)?;
    checked(item, Some(position))
}

/// The item at `position` (from 1) of `info`.
pub fn item_at(info: &PlaylistInfo, position: u32) -> Result<&PlaylistItem, String> {
    let index = (position as usize).checked_sub(1);
    index
        .and_then(|index| info.spec.items.get(index))
        .ok_or_else(|| {
            format!(
                "{} has no item {position}; `tessaro-ctl playlist show {}` lists them",
                info.spec.name, info.spec.name
            )
        })
}

/// Add an item from what was typed, at `at` (from 1) or at the end.
pub fn add_item(
    session: &mut Session,
    playlist: &str,
    fields: &ItemFields,
    at: Option<u32>,
) -> Result<PlaylistInfo, String> {
    let item = item_from_fields(fields, at)?;
    session.call::<api::playlist::ItemAdd>(
        PlaylistRef {
            playlist: playlist.to_string(),
        },
        PlaylistItemAddBody { item, at },
    )
}

/// Change only the given fields of the item at `position`: the device
/// replaces a whole item, so it is read first and sent back in full.
pub fn set_item(
    session: &mut Session,
    playlist: &str,
    position: u32,
    fields: &ItemFields,
) -> Result<PlaylistInfo, String> {
    let info = session.call::<api::playlist::Show>(
        PlaylistRef {
            playlist: playlist.to_string(),
        },
        (),
    )?;
    let item = edit_item(item_at(&info, position)?, fields, position)?;
    session.call::<api::playlist::ItemSet>(
        PlaylistItemRef {
            playlist: info.id.clone(),
            position,
        },
        item,
    )
}

/// What a playlist file gives: the JSON `tessaro-ctl playlist show --json`
/// prints (a `PlaylistInfo`), a `PlaylistSpec`, or just a list of items.
/// Its name and id are not read: the command names the playlist.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct PlaylistFile {
    #[serde(default)]
    pub transition: Option<Transition>,
    #[serde(default)]
    pub transition_ms: Option<u32>,
    #[serde(default)]
    pub items: Vec<PlaylistItem>,
}

/// A playlist file's text; `origin` names it in a refusal.
pub fn parse_file(text: &str, origin: &str) -> Result<PlaylistFile, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|err| format!("{origin}: not JSON: {err}"))?;
    let file = if value.is_array() {
        PlaylistFile {
            items: serde_json::from_value(value).map_err(|err| format!("{origin}: {err}"))?,
            ..PlaylistFile::default()
        }
    } else {
        serde_json::from_value(value).map_err(|err| format!("{origin}: {err}"))?
    };
    if file.items.len() > PLAYLIST_ITEMS_MAX {
        return Err(format!(
            "{origin}: a playlist takes at most {PLAYLIST_ITEMS_MAX} items"
        ));
    }
    for (index, item) in file.items.iter().enumerate() {
        check_item(item.clone(), index + 1).map_err(|err| format!("{origin}: {err}"))?;
    }
    Ok(file)
}

/// A playlist file read from this machine.
pub fn read_file(path: &Path) -> Result<PlaylistFile, String> {
    let text = std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    parse_file(&text, &path.display().to_string())
}

/// A new playlist named `name`: the file's items and transition if there is
/// one, a given transition before the file's. Checked.
pub fn create_spec(
    name: &str,
    file: Option<PlaylistFile>,
    transition: Option<Transition>,
    transition_ms: Option<u32>,
) -> Result<PlaylistSpec, String> {
    let file = file.unwrap_or_default();
    check_playlist(PlaylistSpec {
        name: name.to_string(),
        transition: transition.or(file.transition).unwrap_or_default(),
        transition_ms: transition_ms
            .or(file.transition_ms)
            .unwrap_or(TRANSITION_MS_DEFAULT),
        items: file.items,
    })
}

/// A change of a playlist: a new name, the file's items in place of all of
/// them and its transition, a given transition before the file's. Checked.
pub fn change(
    name: Option<String>,
    file: Option<PlaylistFile>,
    transition: Option<Transition>,
    transition_ms: Option<u32>,
) -> Result<PlaylistChange, String> {
    let name = name.map(|name| name.trim().to_string());
    if let Some(name) = &name {
        check_playlist_name(name)?;
    }
    let (file_transition, file_ms, items) = match file {
        Some(file) => (file.transition, file.transition_ms, Some(file.items)),
        None => (None, None, None),
    };
    let change = PlaylistChange {
        name,
        transition: transition.or(file_transition),
        transition_ms: transition_ms.or(file_ms),
        items,
    };
    if change == PlaylistChange::default() {
        return Err(
            "nothing to change: give --name, --transition, --transition-ms or --file".to_string(),
        );
    }
    Ok(change)
}

/// A new timetable entry from what was typed: days like `mon-fri` (none or
/// `all` for every day), times `HH:MM`. Checked.
pub fn entry_spec(
    playlist: &str,
    days: Option<&str>,
    from: &str,
    to: &str,
    priority: i32,
    enabled: bool,
) -> Result<TimetableSpec, String> {
    check_timetable(TimetableSpec {
        playlist: playlist.to_string(),
        days: parse_days(days.unwrap_or(""))?,
        from: from.to_string(),
        to: to.to_string(),
        priority,
        enabled,
    })
}

/// What was typed for a change of a timetable entry; `None` leaves a field.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntryFields {
    pub playlist: Option<String>,
    /// Like `mon-fri`; empty or `all` for every day.
    pub days: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub priority: Option<i32>,
    pub enabled: Option<bool>,
}

/// The change `fields` make, the days and times read and written the one
/// way.
pub fn entry_change(fields: &EntryFields) -> Result<TimetableEntryChange, String> {
    let clock = |text: &Option<String>| -> Result<Option<String>, String> {
        text.as_deref()
            .map(|text| parse_clock(text).map(format_clock))
            .transpose()
    };
    let playlist = fields.playlist.as_ref().map(|name| name.trim().to_string());
    if playlist.as_deref() == Some("") {
        return Err("a timetable entry needs a playlist".to_string());
    }
    let change = TimetableEntryChange {
        playlist,
        days: fields.days.as_deref().map(parse_days).transpose()?,
        from: clock(&fields.from)?,
        to: clock(&fields.to)?,
        priority: fields.priority,
        enabled: fields.enabled,
    };
    if change.from.as_deref() == Some("24:00") {
        return Err("an entry cannot start at 24:00".to_string());
    }
    if change == TimetableEntryChange::default() {
        return Err(
            "nothing to change: give --playlist, --days, --from, --to, --priority, --enable or --disable"
                .to_string(),
        );
    }
    Ok(change)
}

/// The start of an id that is enough to name it: the device takes any
/// start of a timetable entry's id only one entry has.
pub fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::playlist::{Day, Fit};

    fn typed(kind: &str, src: &str) -> ItemFields {
        ItemFields {
            kind: Some(kind.to_string()),
            src: Some(src.to_string()),
            ..ItemFields::default()
        }
    }

    #[test]
    fn spans_read_like_they_are_typed() {
        assert_eq!(parse_ms("90"), Ok(90_000));
        assert_eq!(parse_ms("1.5"), Ok(1500));
        assert_eq!(parse_ms("1.5s"), Ok(1500));
        assert_eq!(parse_ms("500ms"), Ok(500));
        assert_eq!(parse_ms("1m30s"), Ok(90_000));
        assert_eq!(parse_ms("2h"), Ok(7_200_000));
        assert_eq!(parse_ms("0:05"), Ok(5000));
        assert_eq!(parse_ms("1:02.5"), Ok(62_500));
        assert_eq!(parse_ms("1:00:00"), Ok(3_600_000));
        assert_eq!(parse_ms(" 10s "), Ok(10_000));
        for wrong in [
            "", "s", "10x", "1:5", "1:60", "1:61:00", "1.2.3", "5x10", "a:05",
        ] {
            assert!(parse_ms(wrong).is_err(), "{wrong}");
        }
    }

    #[test]
    fn durations_are_whole_seconds() {
        assert_eq!(parse_seconds("10s"), Ok(10));
        assert_eq!(parse_seconds("1m30s"), Ok(90));
        assert_eq!(parse_seconds("90"), Ok(90));
        assert!(parse_seconds("1.5s").is_err());
    }

    #[test]
    fn written_spans_read_back_the_same() {
        for ms in [
            0, 250, 999, 1000, 1500, 5000, 62_500, 90_000, 3_600_000, 3_723_250,
        ] {
            assert_eq!(parse_ms(&format_ms(ms)), Ok(ms), "{}", format_ms(ms));
            assert_eq!(
                parse_ms(&format_position(ms)),
                Ok(ms),
                "{}",
                format_position(ms)
            );
        }
        assert_eq!(format_ms(1500), "1.5s");
        assert_eq!(format_ms(62_500), "1m2.5s");
        assert_eq!(format_position(5000), "0:05");
        assert_eq!(format_position(62_500), "1:02.5");
        assert_eq!(format_position(3_600_000), "1:00:00");
        assert_eq!(format_seconds(90), "1m30s");
    }

    #[test]
    fn a_transition_is_plain_milliseconds() {
        assert_eq!(parse_transition_ms(" 800 "), Ok(800));
        assert_eq!(parse_transition_ms("0"), Ok(0));
        for wrong in ["", "0.8s", "-1", "fast"] {
            assert!(parse_transition_ms(wrong).is_err(), "{wrong}");
        }
    }

    #[test]
    fn a_new_item_takes_what_was_typed() {
        let mut fields = typed("video", "https://cdn.test/promo.mp4");
        fields.from = Some("5s".to_string());
        fields.to = Some("0:10".to_string());
        fields.volume = Some("80".to_string());
        fields.fit = Some("cover".to_string());
        let item = item_from_fields(&fields, None).unwrap();
        assert_eq!(item.trim_start_ms, Some(5000));
        assert_eq!(item.trim_end_ms, Some(10_000));
        assert!(item.sound, "a volume turns the sound on");
        assert_eq!(item.volume, Some(80));
        assert_eq!(item.fit, Fit::Cover);
        assert_eq!(item.duration_s, None);
    }

    #[test]
    fn a_page_or_image_gets_the_default_duration() {
        let item = item_from_fields(&typed("url", "https://menu.test/"), Some(1)).unwrap();
        assert_eq!(item.duration_s, Some(DURATION_DEFAULT_S));
    }

    #[test]
    fn a_new_item_is_refused_as_the_device_would() {
        let mut fields = typed("image", "https://cdn.test/a.png");
        fields.sound = Some(true);
        let err = item_from_fields(&fields, None).unwrap_err();
        assert!(err.starts_with("the new item: "), "{err}");
        let err = item_from_fields(&fields, Some(3)).unwrap_err();
        assert!(err.starts_with("item 3: "), "{err}");
        assert!(item_from_fields(&typed("gif", "https://x.test/"), None).is_err());
        assert!(item_from_fields(&typed("url", " "), None).is_err());
    }

    #[test]
    fn an_edit_changes_only_what_was_given() {
        let mut fields = typed("url", "https://menu.test/");
        fields.duration = Some("30s".to_string());
        fields.idle = Some("45s".to_string());
        fields.bridge = Some(true);
        let item = item_from_fields(&fields, Some(1)).unwrap();
        assert!(item.interactive, "an idle time makes it interactive");

        let edit = ItemFields {
            src: Some("https://menu.test/today".to_string()),
            interactive: Some(false),
            ..ItemFields::default()
        };
        let edited = edit_item(&item, &edit, 1).unwrap();
        assert_eq!(edited.src, "https://menu.test/today");
        assert_eq!(edited.duration_s, Some(30));
        assert!(!edited.interactive);
        assert_eq!(edited.idle_s, None, "the idle time goes with it");
        assert!(edited.bridge);

        let back = ItemFields {
            duration: Some(String::new()),
            ..ItemFields::default()
        };
        assert_eq!(
            edit_item(&item, &back, 1).unwrap().duration_s,
            Some(DURATION_DEFAULT_S),
            "empty is the default"
        );
    }

    #[test]
    fn changing_the_kind_drops_what_no_longer_applies() {
        let mut fields = typed("url", "https://menu.test/");
        fields.bridge = Some(true);
        fields.ready_delay = Some("500ms".to_string());
        let page = item_from_fields(&fields, Some(1)).unwrap();
        let video = edit_item(
            &page,
            &ItemFields {
                kind: Some("video".to_string()),
                src: Some("https://cdn.test/a.mp4".to_string()),
                ..ItemFields::default()
            },
            1,
        )
        .unwrap();
        assert_eq!(video.duration_s, None);
        assert!(!video.bridge);
        assert_eq!(video.ready_delay_ms, None);
    }

    #[test]
    fn a_file_is_what_show_prints_or_a_spec_or_items() {
        let info = r#"{"id":"p1","name":"lobby","transition":"cut","transition_ms":0,
            "items":[{"kind":"image","src":"http://127.0.0.1/files/a.png","duration_s":5}],
            "default":true,"timetable":[],"playing":false}"#;
        let file = parse_file(info, "lobby.json").unwrap();
        assert_eq!(file.transition, Some(Transition::Cut));
        assert_eq!(file.transition_ms, Some(0));
        assert_eq!(file.items.len(), 1);

        let items = r#"[{"kind":"url","src":"https://menu.test/","duration_s":20}]"#;
        let file = parse_file(items, "x").unwrap();
        assert_eq!(file.transition, None);
        assert_eq!(file.items[0].kind, ItemKind::Url);

        let wrong = r#"[{"kind":"url","src":"https://menu.test/"}]"#;
        assert!(parse_file(wrong, "x").unwrap_err().contains("item 1"));
        assert!(parse_file("{", "x").is_err());
    }

    #[test]
    fn given_transitions_come_before_the_files() {
        let file = PlaylistFile {
            transition: Some(Transition::Cut),
            transition_ms: Some(0),
            items: Vec::new(),
        };
        let spec = create_spec("lobby", Some(file.clone()), Some(Transition::Slide), None).unwrap();
        assert_eq!(spec.transition, Transition::Slide);
        assert_eq!(spec.transition_ms, 0);
        let spec = create_spec("lobby", None, None, None).unwrap();
        assert_eq!(spec.transition_ms, TRANSITION_MS_DEFAULT);
        assert!(create_spec("Lobby", None, None, None).is_err());

        let changed = change(None, Some(file), None, Some(300)).unwrap();
        assert_eq!(changed.items, Some(Vec::new()));
        assert_eq!(changed.transition_ms, Some(300));
        assert!(change(None, None, None, None).is_err());
        assert!(change(Some("status".to_string()), None, None, None).is_err());
    }

    #[test]
    fn timetable_entries_from_typed_days_and_times() {
        let spec = entry_spec("lunch", Some("mon-fri"), "11:30", "14:00", 0, true).unwrap();
        assert_eq!(
            spec.days,
            vec![Day::Mon, Day::Tue, Day::Wed, Day::Thu, Day::Fri]
        );
        assert!(entry_spec("lunch", None, "24:00", "01:00", 0, true).is_err());
        assert!(entry_spec("lunch", Some("someday"), "11:30", "14:00", 0, true).is_err());

        let change = entry_change(&EntryFields {
            from: Some("9:05".to_string()),
            days: Some("all".to_string()),
            ..EntryFields::default()
        })
        .unwrap();
        assert_eq!(change.from.as_deref(), Some("09:05"));
        assert_eq!(change.days, Some(Vec::new()));
        assert!(entry_change(&EntryFields::default()).is_err());
        assert!(entry_change(&EntryFields {
            to: Some("25:00".to_string()),
            ..EntryFields::default()
        })
        .is_err());
    }

    #[test]
    fn short_ids_cut_long_ones_only() {
        assert_eq!(short_id("0123456789abcdef"), "01234567");
        assert_eq!(short_id("abc"), "abc");
    }
}
