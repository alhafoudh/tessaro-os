//! Playlists and the timetable: `tessaro-ctl playlist`, the playlist row of
//! `device status`, and the GUI's Playlists page.

use protocol::playlist::{
    format_days, ActiveReason, Fit, ItemKind, PlaylistInfo, PlaylistItem, PlaylistStatus,
    TimetableInfo, Transition,
};

use crate::playlist::{format_ms, format_position, format_seconds, short_id};
use crate::schedule::relative;
use crate::text::{Fact, Line, Tone};

/// Longest a source is shown in a listing, characters.
const SRC_SHOWN: usize = 48;

/// A source short enough for a listing: no scheme, the device's own files
/// by their path, a long one cut in the middle.
pub fn shorten(src: &str) -> String {
    let bare = if protocol::playlist::is_local(src) {
        let rest = src
            .strip_prefix("http://127.0.0.1")
            .or_else(|| src.strip_prefix("http://localhost"))
            .unwrap_or(src);
        if rest.is_empty() {
            "/"
        } else {
            rest
        }
    } else {
        src.strip_prefix("https://")
            .or_else(|| src.strip_prefix("http://"))
            .unwrap_or(src)
    };
    let chars: Vec<char> = bare.chars().collect();
    if chars.len() <= SRC_SHOWN {
        return bare.to_string();
    }
    let head = (SRC_SHOWN - 3) / 2;
    let tail = SRC_SHOWN - 3 - head;
    let start: String = chars[..head].iter().collect();
    let end: String = chars[chars.len() - tail..].iter().collect();
    format!("{start}...{end}")
}

/// How long an item plays: its duration, or for a video the part of it
/// that plays.
pub fn timing(item: &PlaylistItem) -> String {
    match item.kind {
        ItemKind::Url | ItemKind::Image => match item.duration_s {
            Some(seconds) => format_seconds(seconds),
            None => "-".to_string(),
        },
        ItemKind::Video => match (item.trim_start_ms, item.trim_end_ms) {
            (None, None) => "whole".to_string(),
            (start, None) => format!("{}-end", format_position(start.unwrap_or(0))),
            (start, Some(end)) => format!(
                "{}-{}",
                format_position(start.unwrap_or(0)),
                format_position(end)
            ),
        },
    }
}

/// A transition in words: `fade 800 ms`, `cut`.
pub fn transition(transition: Transition, ms: u32) -> String {
    match transition {
        Transition::Cut => "cut".to_string(),
        other => format!("{} {ms} ms", other.name()),
    }
}

/// What sets an item apart from one with every option at its default:
/// `sound 80%`, `cover`, `interactive, idle 45s`, `bridge`.
pub fn options(item: &PlaylistItem) -> Vec<String> {
    let mut words = Vec::new();
    if item.kind == ItemKind::Video {
        words.push(match (item.sound, item.volume) {
            (true, Some(volume)) => format!("sound {volume}%"),
            (true, None) => "sound".to_string(),
            (false, _) => "muted".to_string(),
        });
    }
    if item.kind != ItemKind::Url && item.fit != Fit::Contain {
        words.push(item.fit.name().to_string());
    }
    if let Some(background) = &item.background {
        words.push(format!("on {background}"));
    }
    match (item.transition, item.transition_ms) {
        (Some(Transition::Cut), _) => words.push("cut in".to_string()),
        (Some(kind), Some(ms)) => words.push(format!("{} in {ms} ms", kind.name())),
        (Some(kind), None) => words.push(format!("{} in", kind.name())),
        (None, Some(ms)) => words.push(format!("in {ms} ms")),
        (None, None) => {}
    }
    if item.interactive {
        words.push(match item.idle_s {
            Some(idle) => format!("interactive, idle {}", format_seconds(idle)),
            None => "interactive".to_string(),
        });
    }
    if let Some(delay) = item.ready_delay_ms {
        words.push(format!("ready {} after load", format_ms(u64::from(delay))));
    }
    if item.bridge {
        words.push("bridge".to_string());
    }
    words
}

/// One item: its place, kind, source, how long it plays and its options.
pub fn item(item: &PlaylistItem, position: u32) -> Line {
    let options = options(item);
    let line = Line::new()
        .pad(Tone::Muted, format!("{position:>3}"), 3)
        .text(" ")
        .pad(Tone::Label, item.kind.name(), 5)
        .text(" ")
        .pad(Tone::Plain, shorten(&item.src), 40)
        .text(" ");
    if options.is_empty() {
        line.add(Tone::Plain, timing(item))
    } else {
        line.pad(Tone::Plain, timing(item), 11)
            .text(" ")
            .add(Tone::Muted, options.join(", "))
    }
}

/// A playlist's items, one line each, or how to add the first.
pub fn items(info: &PlaylistInfo) -> Vec<Line> {
    if info.spec.items.is_empty() {
        return vec![Line::of(Tone::Warn, "no items; add one with")
            .text(" ")
            .add(
                Tone::Cmd,
                format!(
                    "tessaro-ctl playlist items add {} --url URL",
                    info.spec.name
                ),
            )];
    }
    info.spec
        .items
        .iter()
        .zip(1..)
        .map(|(one, position)| item(one, position))
        .collect()
}

/// `playlist list`: a line per playlist, the one on screen marked, and
/// where each is used.
pub fn list(playlists: &[PlaylistInfo]) -> Vec<Line> {
    if playlists.is_empty() {
        return vec![Line::of(Tone::Muted, "no playlists; add one with")
            .text(" ")
            .add(Tone::Cmd, "tessaro-ctl playlist create NAME")];
    }
    let mut lines = Vec::new();
    for info in playlists {
        let marker = if info.playing {
            Line::of(Tone::Ok, "*")
        } else {
            Line::plain(" ")
        };
        let count = match info.spec.items.len() {
            1 => "1 item".to_string(),
            count => format!("{count} items"),
        };
        let mut used = Vec::new();
        if info.default {
            used.push("playlist.default".to_string());
        }
        match info.timetable.len() {
            0 => {}
            1 => used.push("1 timetable entry".to_string()),
            entries => used.push(format!("{entries} timetable entries")),
        }
        let line = marker
            .text(" ")
            .pad(Tone::Heading, &info.spec.name, 20)
            .text(" ");
        lines.push(if used.is_empty() {
            line.add(Tone::Plain, count)
        } else {
            line.pad(Tone::Plain, count, 10)
                .text(" ")
                .add(Tone::Muted, used.join(", "))
        });
    }
    lines.push(Line::new());
    lines.push(
        Line::of(Tone::Muted, "* on screen now; its items with")
            .text(" ")
            .add(Tone::Cmd, "tessaro-ctl playlist show NAME"),
    );
    lines
}

/// `playlist show`: one playlist, before its items.
pub fn show(info: &PlaylistInfo) -> Vec<Fact> {
    let mut facts = vec![
        Fact::new("name", Line::of(Tone::Heading, &info.spec.name)),
        Fact::new("id", Line::of(Tone::Muted, &info.id)),
        Fact::new(
            "transition",
            transition(info.spec.transition, info.spec.transition_ms),
        ),
        Fact::new(
            "default",
            if info.default {
                Line::of(Tone::Ok, "yes")
                    .text(" ")
                    .add(Tone::Muted, "(playlist.default)")
            } else {
                Line::of(Tone::Muted, "no")
            },
        ),
    ];
    facts.push(Fact::new(
        "timetable",
        if info.timetable.is_empty() {
            Line::of(Tone::Muted, "not in it")
        } else {
            let ids: Vec<&str> = info.timetable.iter().map(|id| short_id(id)).collect();
            Line::plain(format!("entries {}", ids.join(", ")))
        },
    ));
    facts.push(Fact::new(
        "playing",
        if info.playing {
            Line::of(Tone::Ok, "yes")
        } else {
            Line::of(Tone::Muted, "no")
        },
    ));
    facts
}

/// One timetable entry: `mon-fri 11:30-14:00 -> lunch, priority 0`, the
/// active one marked, a disabled one muted.
pub fn entry(entry: &TimetableInfo) -> Line {
    let spec = &entry.spec;
    let on = |tone: Tone| if spec.enabled { tone } else { Tone::Muted };
    let marker = if entry.active {
        Line::of(Tone::Ok, "*")
    } else {
        Line::plain(" ")
    };
    let line = marker
        .text(" ")
        .pad(Tone::Muted, short_id(&entry.id), 8)
        .text(" ")
        .pad(on(Tone::Plain), format_days(&spec.days), 13)
        .text(" ")
        .add(on(Tone::Plain), format!("{}-{}", spec.from, spec.to))
        .text(" ")
        .add(Tone::Muted, "->")
        .text(" ")
        .add(on(Tone::Heading), &entry.playlist_name)
        .add(on(Tone::Plain), format!(", priority {}", spec.priority));
    if spec.enabled {
        line
    } else {
        line.text(" ").add(Tone::Muted, "(off)")
    }
}

/// `playlist timetable list`: every entry in order, and how they decide.
pub fn timetable(entries: &[TimetableInfo]) -> Vec<Line> {
    if entries.is_empty() {
        return vec![Line::of(Tone::Muted, "no timetable entries; add one with")
            .text(" ")
            .add(
                Tone::Cmd,
                "tessaro-ctl playlist timetable add PLAYLIST --from HH:MM --to HH:MM",
            )];
    }
    let mut lines: Vec<Line> = entries.iter().map(entry).collect();
    lines.push(Line::new());
    lines.push(Line::of(
        Tone::Muted,
        "* decides what plays now; where entries overlap the highest priority wins, then the one listed first",
    ));
    lines
}

/// Why the player shows what it shows, in a few words.
fn reason(status: &PlaylistStatus) -> Line {
    match status.reason {
        ActiveReason::Timetable => Line::of(
            Tone::Muted,
            match &status.entry {
                Some(entry) => format!("(timetable entry {})", short_id(entry)),
                None => "(timetable)".to_string(),
            },
        ),
        ActiveReason::Default => Line::of(Tone::Muted, "(playlist.default)"),
        ActiveReason::Url => Line::of(
            Tone::Muted,
            "(no timetable entry covers now and playlist.default is empty)",
        ),
    }
}

/// The playlist playing, or browser.url in its place.
fn playing(status: &PlaylistStatus) -> Line {
    match &status.playlist {
        Some(name) => Line::of(Tone::Heading, name),
        None => Line::plain("browser.url"),
    }
}

/// How far the copies of the media are.
fn cache(status: &PlaylistStatus) -> Line {
    let cache = &status.cache;
    if cache.ready + cache.pending + cache.failed == 0 {
        return Line::of(Tone::Muted, "nothing to keep a copy of");
    }
    let mut line = Line::of(Tone::Ok, format!("{} ready", cache.ready));
    if cache.pending > 0 {
        line = line
            .text(", ")
            .add(Tone::Warn, format!("{} pending", cache.pending));
    }
    if cache.failed > 0 {
        line = line
            .text(", ")
            .add(Tone::Bad, format!("{} failed", cache.failed));
    }
    line
}

/// `playlist status`: whether the player is on screen, what it plays and
/// why, the item on screen, what it went past, how far the media copies
/// are. `now` is seconds since the epoch.
pub fn status(status: &PlaylistStatus, now: i64) -> Vec<Fact> {
    if !status.player {
        return vec![Fact::new(
            "player",
            Line::of(Tone::Muted, "off")
                .text(" ")
                .add(Tone::Muted, "- browser.url shows; set")
                .text(" ")
                .add(Tone::Cmd, "tessaro-ctl config set playlist.default=NAME")
                .text(" ")
                .add(Tone::Muted, "or add a timetable entry"),
        )];
    }
    let mut facts = vec![Fact::new("player", Line::of(Tone::Ok, "on"))];
    if status.nothing_playable {
        facts.push(Fact::new(
            "showing",
            Line::of(Tone::Bad, "nothing playable")
                .text(" ")
                .add(Tone::Muted, "- the offline page is up"),
        ));
    }
    facts.push(Fact::new(
        "playlist",
        playing(status).text(" ").join(reason(status)),
    ));
    facts.push(Fact::new(
        "item",
        match &status.item {
            Some(item) => Line::of(Tone::Muted, item.position.to_string())
                .text(" ")
                .add(Tone::Label, item.kind.name())
                .text(" ")
                .add(Tone::Plain, shorten(&item.src))
                .text(" ")
                .add(
                    Tone::Muted,
                    match relative(item.since.unix, now).as_str() {
                        "now" => "(came on just now)".to_string(),
                        when => format!("(came on {when})"),
                    },
                ),
            None => Line::of(Tone::Muted, "(the player has not said yet)"),
        },
    ));
    for (at, skipped) in status.skipped.iter().enumerate() {
        facts.push(Fact::new(
            if at == 0 { "skipped" } else { "" },
            Line::of(
                Tone::Warn,
                format!("{} {}", skipped.position, shorten(&skipped.src)),
            )
            .text(": ")
            .add(Tone::Plain, &skipped.reason)
            .text(" ")
            .add(Tone::Muted, format!("({})", relative(skipped.at.unix, now))),
        ));
    }
    facts.push(Fact::new("media", cache(status)));
    facts
}

/// The player in one line, for `device status`: what plays and why, or
/// that nothing can.
pub fn summary(status: &PlaylistStatus) -> Line {
    if !status.player {
        return Line::of(Tone::Muted, "off - browser.url shows directly");
    }
    if status.nothing_playable {
        return Line::of(Tone::Bad, "nothing playable")
            .text(" ")
            .add(Tone::Muted, "- the offline page is up");
    }
    let mut line = playing(status).text(" ").join(reason(status));
    if let Some(item) = &status.item {
        line = line.text(format!(", item {} {}", item.position, item.kind.name()));
    }
    match status.cache.failed {
        0 => line,
        1 => line.text(", ").add(Tone::Warn, "1 media copy failed"),
        failed => line
            .text(", ")
            .add(Tone::Warn, format!("{failed} media copies failed")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::playlist::{CacheStatus, PlaylistSpec, TimetableSpec};

    fn video() -> PlaylistItem {
        let mut item = PlaylistItem::new(ItemKind::Video, "https://cdn.test/promo.mp4");
        item.trim_start_ms = Some(5000);
        item.trim_end_ms = Some(10_000);
        item.sound = true;
        item.volume = Some(80);
        item
    }

    #[test]
    fn sources_lose_their_scheme_and_the_device_its_host() {
        assert_eq!(shorten("https://cdn.test/a.mp4"), "cdn.test/a.mp4");
        assert_eq!(shorten("http://127.0.0.1/files/a.png"), "/files/a.png");
        let long = format!("https://cdn.test/{}", "x".repeat(100));
        let short = shorten(&long);
        assert_eq!(short.chars().count(), SRC_SHOWN);
        assert!(short.contains("..."));
    }

    #[test]
    fn an_item_line_says_how_long_and_how() {
        let line = item(&video(), 2).to_string();
        assert!(line.contains("0:05-0:10"), "{line}");
        assert!(line.contains("sound 80%"), "{line}");
        let mut page = PlaylistItem::new(ItemKind::Url, "https://menu.test/");
        page.duration_s = Some(90);
        assert!(item(&page, 1).to_string().ends_with("1m30s"));
    }

    #[test]
    fn timings_of_a_video_name_the_part_that_plays() {
        let mut clip = video();
        assert_eq!(timing(&clip), "0:05-0:10");
        clip.trim_end_ms = None;
        assert_eq!(timing(&clip), "0:05-end");
        clip.trim_start_ms = None;
        assert_eq!(timing(&clip), "whole");
    }

    #[test]
    fn a_disabled_entry_is_muted() {
        let mut one = TimetableInfo {
            id: "0123456789".to_string(),
            spec: TimetableSpec {
                playlist: "p1".to_string(),
                days: protocol::playlist::parse_days("mon-fri").unwrap(),
                from: "11:30".to_string(),
                to: "14:00".to_string(),
                priority: 0,
                enabled: true,
            },
            playlist_name: "lunch".to_string(),
            active: false,
        };
        assert!(entry(&one)
            .to_string()
            .contains("mon-fri       11:30-14:00 -> lunch, priority 0"));
        one.spec.enabled = false;
        let line = entry(&one);
        assert!(line.to_string().ends_with("(off)"));
        assert_eq!(line.tone(), Tone::Plain);
    }

    #[test]
    fn nothing_playable_is_bad() {
        let status = PlaylistStatus {
            player: true,
            playlist: Some("lobby".to_string()),
            reason: ActiveReason::Default,
            entry: None,
            item: None,
            skipped: Vec::new(),
            nothing_playable: true,
            cache: CacheStatus::default(),
        };
        assert_eq!(summary(&status).tone(), Tone::Bad);
        assert_eq!(super::status(&status, 0)[1].value.tone(), Tone::Bad);
    }

    #[test]
    fn an_empty_playlist_says_how_to_fill_it() {
        let info = PlaylistInfo {
            id: "p1".to_string(),
            spec: PlaylistSpec {
                name: "lobby".to_string(),
                transition: Transition::Fade,
                transition_ms: 800,
                items: Vec::new(),
            },
            default: false,
            timetable: Vec::new(),
            playing: false,
        };
        assert!(items(&info)[0]
            .to_string()
            .contains("tessaro-ctl playlist items add lobby"));
    }
}
