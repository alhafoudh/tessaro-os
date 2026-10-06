//! Playlists and the timetable: what the player page shows instead of one
//! URL, and which playlist plays when.
//!
//! The `playlists` and `timetable` tables of `tessaro.db` are the only
//! truth. The player is on screen while `playlist.default` is set or the
//! timetable has an entry - decided by the configuration, never the clock,
//! so a device only ever switches between browser.url shown directly and the
//! player when someone edits it. Which playlist plays is picked here
//! (`pick`) and written for the page as `/run/tessaro-kiosk/playlist.json`
//! (`player_doc`); `control/playlists.rs` does that once a minute and after
//! every change. docs/playlists.md has the whole picture.
//!
//! What the rest of the agent needs of the store - whether the timetable has
//! an entry, the origins of the interactive items - is `Shared`, read by
//! `render::live` into `state::Live`, the same way the device's addresses
//! reach `state::Effective`.

use protocol::playlist::{
    self as wire, ActiveReason, Day, ItemKind, PlaylistItem, PlaylistSpec, TimetableSpec,
    Transition,
};
use serde::Serialize;
use tessaro_db::rusqlite::{self, params, types::Type, Connection};

use crate::db::{Db, Stored};

#[derive(Debug, Default)]
pub struct Playlists {
    pub playlists: Vec<Playlist>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playlist {
    pub id: String,
    pub spec: PlaylistSpec,
}

#[derive(Debug, Default)]
pub struct Timetable {
    pub entries: Vec<Entry>,
}

/// A timetable entry as stored: `spec.playlist` is the playlist's id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub spec: TimetableSpec,
}

fn json_column<T: serde::de::DeserializeOwned>(text: String, column: usize) -> rusqlite::Result<T> {
    serde_json::from_str(&text)
        .map_err(|err| rusqlite::Error::FromSqlConversionFailure(column, Type::Text, Box::new(err)))
}

impl Stored for Playlists {
    const WHAT: &'static str = "the playlists";

    fn load(db: &Connection) -> rusqlite::Result<Self> {
        let mut rows = db.prepare(
            "SELECT id, name, transition, transition_ms, items \
             FROM playlists ORDER BY position",
        )?;
        let playlists = rows
            .query_map([], |row| {
                let transition: String = row.get(2)?;
                Ok(Playlist {
                    id: row.get(0)?,
                    spec: PlaylistSpec {
                        name: row.get(1)?,
                        transition: transition.parse().unwrap_or_default(),
                        transition_ms: row.get(3)?,
                        items: json_column(row.get(4)?, 4)?,
                    },
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Self { playlists })
    }

    fn save(&self, db: &Connection) -> rusqlite::Result<()> {
        db.execute("DELETE FROM playlists", [])?;
        let mut insert = db.prepare(
            "INSERT INTO playlists (id, position, name, transition, transition_ms, items) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for (position, playlist) in self.playlists.iter().enumerate() {
            let spec = &playlist.spec;
            insert.execute(params![
                playlist.id,
                position as i64,
                spec.name,
                spec.transition.name(),
                spec.transition_ms,
                serde_json::to_string(&spec.items).unwrap_or_default(),
            ])?;
        }
        Ok(())
    }

    fn clear(db: &Connection) -> rusqlite::Result<()> {
        db.execute("DELETE FROM playlists", []).map(drop)
    }
}

impl Stored for Timetable {
    const WHAT: &'static str = "the timetable";

    fn load(db: &Connection) -> rusqlite::Result<Self> {
        let mut rows = db.prepare(
            "SELECT id, playlist_id, days, time_from, time_to, priority, enabled \
             FROM timetable ORDER BY position",
        )?;
        let entries = rows
            .query_map([], |row| {
                Ok(Entry {
                    id: row.get(0)?,
                    spec: TimetableSpec {
                        playlist: row.get(1)?,
                        days: json_column(row.get(2)?, 2)?,
                        from: row.get(3)?,
                        to: row.get(4)?,
                        priority: row.get(5)?,
                        enabled: row.get(6)?,
                    },
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Self { entries })
    }

    fn save(&self, db: &Connection) -> rusqlite::Result<()> {
        db.execute("DELETE FROM timetable", [])?;
        let mut insert = db.prepare(
            "INSERT INTO timetable \
             (id, position, playlist_id, days, time_from, time_to, priority, enabled) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for (position, entry) in self.entries.iter().enumerate() {
            let spec = &entry.spec;
            insert.execute(params![
                entry.id,
                position as i64,
                spec.playlist,
                serde_json::to_string(&spec.days).unwrap_or_default(),
                spec.from,
                spec.to,
                spec.priority,
                spec.enabled,
            ])?;
        }
        Ok(())
    }

    fn clear(db: &Connection) -> rusqlite::Result<()> {
        db.execute("DELETE FROM timetable", []).map(drop)
    }
}

/// The playlist named or numbered `query`, by id or name.
pub fn find(playlists: &[Playlist], query: &str) -> Result<usize, String> {
    let query = query.trim();
    playlists
        .iter()
        .position(|playlist| playlist.id == query || playlist.spec.name == query)
        .ok_or_else(|| format!("no playlist {query}; `tessaro-ctl playlist list` shows them"))
}

/// The timetable entry `query`: its id, or a prefix of it only one entry
/// has.
pub fn find_entry(entries: &[Entry], query: &str) -> Result<usize, String> {
    let query = query.trim();
    if let Some(at) = entries.iter().position(|entry| entry.id == query) {
        return Ok(at);
    }
    let matching: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| !query.is_empty() && entry.id.starts_with(query))
        .map(|(at, _)| at)
        .collect();
    match matching.as_slice() {
        [at] => Ok(*at),
        [] => Err(format!(
            "no timetable entry {query}; `tessaro-ctl playlist timetable list` shows them"
        )),
        _ => Err(format!(
            "{query} is the start of more than one timetable entry"
        )),
    }
}

/// `spec` made ready to save: checked, and its name not one of `others`'.
pub fn validate(spec: PlaylistSpec, others: &[&Playlist]) -> Result<PlaylistSpec, String> {
    let spec = wire::check_playlist(spec)?;
    if others.iter().any(|other| other.spec.name == spec.name) {
        return Err(format!("a playlist named {} exists already", spec.name));
    }
    Ok(spec)
}

/// `spec` made ready to save, its playlist turned into the id of one of
/// `playlists`.
pub fn validate_entry(
    spec: TimetableSpec,
    playlists: &[Playlist],
) -> Result<TimetableSpec, String> {
    let mut spec = wire::check_timetable(spec)?;
    spec.playlist = playlists[find(playlists, &spec.playlist)?].id.clone();
    Ok(spec)
}

/// A position from 1, as `tessaro-ctl` and the API give it, as an index
/// into `len` items. `end` allows one past the last, for an insert.
pub fn index(position: u32, len: usize, end: bool) -> Result<usize, String> {
    let last = if end { len + 1 } else { len };
    match position as usize {
        0 => Err("positions start at 1".to_string()),
        at if at > last && last == 0 => Err("the playlist has no items".to_string()),
        at if at > last => Err(format!("there is no item {at}; the playlist has {len}")),
        at => Ok(at - 1),
    }
}

/// The origins of the interactive URL items of every playlist: they get the
/// device grants, like browser.url's origin. Every playlist, not the one
/// playing, so the timetable switching playlists never rewrites the policy
/// (which restarts the browser).
pub fn interactive_origins(playlists: &[Playlist]) -> Vec<String> {
    origins_of(playlists, |item| item.interactive)
}

/// The origins of the URL items that get `window.tessaro`, of every
/// playlist: the bridge answers a frame of the player only from these.
pub fn bridge_origins(playlists: &[Playlist]) -> Vec<String> {
    origins_of(playlists, |item| item.bridge)
}

fn origins_of(playlists: &[Playlist], wanted: impl Fn(&PlaylistItem) -> bool) -> Vec<String> {
    let mut origins: Vec<String> = playlists
        .iter()
        .flat_map(|playlist| &playlist.spec.items)
        .filter(|item| item.kind == ItemKind::Url && wanted(item))
        .filter_map(|item| origin(&item.src))
        .collect();
    origins.sort();
    origins.dedup();
    origins
}

/// `scheme://host[:port]` of an http(s) URL.
pub fn origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit_once('@').map_or(host, |(_, host)| host);
    (!host.is_empty()).then(|| format!("{scheme}://{}", host.to_ascii_lowercase()))
}

/// What the rest of the agent needs of the store: read by `render::live`
/// with what the device reports, so `state::Effective` sees it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shared {
    /// The timetable has an entry, enabled or not: with `playlist.default`,
    /// what decides that the player is on screen.
    pub timetable: bool,
    /// `interactive_origins`.
    pub origins: Vec<String>,
    /// `bridge_origins`.
    pub bridge: Vec<String>,
}

impl Shared {
    pub fn of(playlists: &Playlists, timetable: &Timetable) -> Self {
        Shared {
            timetable: !timetable.entries.is_empty(),
            origins: interactive_origins(&playlists.playlists),
            bridge: bridge_origins(&playlists.playlists),
        }
    }
}

/// `Shared` as the store in `state_dir` holds it now; nothing when there is
/// no store. A read that fails is the defaults, like every read.
pub fn shared(state_dir: &std::path::Path) -> Shared {
    let db = Db::at(state_dir);
    if !db.exists() {
        return Shared::default();
    }
    let log = crate::log::Log::new(false);
    Shared::of(&db.read::<Playlists>(&log), &db.read::<Timetable>(&log))
}

/// Empty the playlists and the timetable: a factory reset.
pub fn clear(db: &Db) -> Result<(), String> {
    db.transaction(|tx| {
        Playlists::clear(tx).map_err(|err| err.to_string())?;
        Timetable::clear(tx).map_err(|err| err.to_string())
    })
}

/// What plays: the playlist and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick<'a> {
    pub playlist: Option<&'a Playlist>,
    pub reason: ActiveReason,
    pub entry: Option<String>,
}

/// What plays at `minute` of `day`: the active timetable entry's playlist,
/// else the playlist `default` names, else browser.url. An entry or a
/// default that names no playlist is passed over.
pub fn pick<'a>(
    playlists: &'a [Playlist],
    timetable: &[Entry],
    default: Option<&str>,
    day: Day,
    minute: u32,
) -> Pick<'a> {
    let playable: Vec<&Entry> = timetable
        .iter()
        .filter(|entry| find(playlists, &entry.spec.playlist).is_ok())
        .collect();
    if let Some(at) = wire::active(playable.iter().map(|entry| &entry.spec), day, minute) {
        let entry = playable[at];
        if let Ok(found) = find(playlists, &entry.spec.playlist) {
            return Pick {
                playlist: Some(&playlists[found]),
                reason: ActiveReason::Timetable,
                entry: Some(entry.id.clone()),
            };
        }
    }
    if let Some(found) = default
        .filter(|name| !name.is_empty())
        .and_then(|name| find(playlists, name).ok())
    {
        return Pick {
            playlist: Some(&playlists[found]),
            reason: ActiveReason::Default,
            entry: None,
        };
    }
    Pick {
        playlist: None,
        reason: ActiveReason::Url,
        entry: None,
    }
}

/// The id the player gets for browser.url played as a playlist of one.
pub const URL_PLAYLIST: &str = "url";

/// `/run/tessaro-kiosk/playlist.json`: the playlist that plays, every
/// option the player needs filled in, so the page decides nothing the
/// agent already knows.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlayerDoc {
    pub id: String,
    pub name: String,
    pub items: Vec<PlayerItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlayerItem {
    /// From 1, as `tessaro-ctl` numbers it; the player reports it back.
    pub position: u32,
    pub kind: ItemKind,
    /// Where the player loads it from: the cached copy when there is one.
    pub src: String,
    /// The source as configured, for the reports.
    pub source: String,
    pub duration_s: Option<u32>,
    pub trim_start_ms: u64,
    pub trim_end_ms: Option<u64>,
    pub sound: bool,
    pub volume: u8,
    pub fit: wire::Fit,
    pub background: String,
    pub transition: Transition,
    pub transition_ms: u32,
    pub interactive: bool,
    pub idle_s: u32,
    pub ready_delay_ms: u32,
    pub bridge: bool,
}

/// The longest a playlist of one stays put, for browser.url: it never moves
/// on, a playlist of one item is never shown again.
const URL_DURATION_S: u32 = wire::PLAYLIST_DURATION_MAX;

/// What the player plays for `pick`. `expand` fills a URL item's `{key}`
/// placeholders as browser.url's are; `cached` gives the local address of a
/// source's copy when the device has one. `url` is browser.url expanded, for
/// a pick of none.
pub fn player_doc(
    pick: &Pick,
    url: &str,
    expand: &dyn Fn(&str) -> String,
    cached: &dyn Fn(&str) -> Option<String>,
) -> PlayerDoc {
    let Some(playlist) = pick.playlist else {
        let mut item = PlaylistItem::new(ItemKind::Url, url);
        item.duration_s = Some(URL_DURATION_S);
        item.interactive = true;
        item.bridge = true;
        return PlayerDoc {
            id: URL_PLAYLIST.to_string(),
            name: String::new(),
            items: vec![player_item(1, &item, Transition::Cut, 0, url.to_string())],
        };
    };
    let spec = &playlist.spec;
    PlayerDoc {
        id: playlist.id.clone(),
        name: spec.name.clone(),
        items: spec
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let src = if item.kind == ItemKind::Url {
                    expand(&item.src)
                } else if item.cached() {
                    cached(&item.src).unwrap_or_else(|| item.src.clone())
                } else {
                    item.src.clone()
                };
                player_item(
                    index as u32 + 1,
                    item,
                    spec.transition,
                    spec.transition_ms,
                    src,
                )
            })
            .collect(),
    }
}

fn player_item(
    position: u32,
    item: &PlaylistItem,
    transition: Transition,
    transition_ms: u32,
    src: String,
) -> PlayerItem {
    PlayerItem {
        position,
        kind: item.kind,
        src,
        source: item.src.clone(),
        duration_s: item.duration_s,
        trim_start_ms: item.trim_start_ms.unwrap_or(0),
        trim_end_ms: item.trim_end_ms,
        sound: item.sound,
        volume: item.volume.unwrap_or(100),
        fit: item.fit,
        background: item
            .background
            .clone()
            .unwrap_or_else(|| "#000000".to_string()),
        transition: item.transition.unwrap_or(transition),
        transition_ms: item.transition_ms.unwrap_or(transition_ms),
        interactive: item.interactive,
        idle_s: item.idle_s.unwrap_or(wire::IDLE_S_DEFAULT),
        ready_delay_ms: item.ready_delay_ms.unwrap_or(0),
        bridge: item.bridge,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playlist(id: &str, name: &str, items: Vec<PlaylistItem>) -> Playlist {
        Playlist {
            id: id.to_string(),
            spec: PlaylistSpec {
                name: name.to_string(),
                transition: Transition::Fade,
                transition_ms: 800,
                items,
            },
        }
    }

    fn entry(id: &str, playlist: &str, from: &str, to: &str, priority: i32) -> Entry {
        Entry {
            id: id.to_string(),
            spec: TimetableSpec {
                playlist: playlist.to_string(),
                days: Vec::new(),
                from: from.to_string(),
                to: to.to_string(),
                priority,
                enabled: true,
            },
        }
    }

    fn image(src: &str) -> PlaylistItem {
        let mut item = PlaylistItem::new(ItemKind::Image, src);
        item.duration_s = Some(10);
        item
    }

    #[test]
    fn the_timetable_wins_over_the_default_and_the_default_over_the_url() {
        let all = [
            playlist("a1", "lobby", vec![]),
            playlist("b2", "lunch", vec![]),
        ];
        let timetable = [entry("e1", "b2", "11:30", "14:00", 0)];
        let noon = pick(&all, &timetable, Some("lobby"), Day::Mon, 12 * 60);
        assert_eq!(noon.playlist.map(|p| p.id.as_str()), Some("b2"));
        assert_eq!(noon.reason, ActiveReason::Timetable);
        assert_eq!(noon.entry.as_deref(), Some("e1"));

        let evening = pick(&all, &timetable, Some("lobby"), Day::Mon, 20 * 60);
        assert_eq!(evening.playlist.map(|p| p.id.as_str()), Some("a1"));
        assert_eq!(evening.reason, ActiveReason::Default);

        let none = pick(&all, &timetable, None, Day::Mon, 20 * 60);
        assert_eq!(none.playlist, None);
        assert_eq!(none.reason, ActiveReason::Url);
    }

    #[test]
    fn an_entry_naming_no_playlist_is_passed_over() {
        let all = [playlist("a1", "lobby", vec![])];
        let timetable = [
            entry("e1", "gone", "00:00", "00:00", 9),
            entry("e2", "a1", "00:00", "00:00", 0),
        ];
        let picked = pick(&all, &timetable, None, Day::Sun, 0);
        assert_eq!(picked.entry.as_deref(), Some("e2"));
    }

    #[test]
    fn the_doc_fills_in_every_option_and_uses_the_copies() {
        let mut page = PlaylistItem::new(ItemKind::Url, "https://menu.test/?t={data.table}");
        page.duration_s = Some(30);
        page.transition = Some(Transition::Slide);
        let all = [playlist(
            "a1",
            "lobby",
            vec![
                image("https://cdn.test/a.png"),
                page,
                image("http://127.0.0.1/files/b.png"),
            ],
        )];
        let picked = pick(&all, &[], Some("lobby"), Day::Mon, 0);
        let doc = player_doc(
            &picked,
            "https://site.test/",
            &|url| url.replace("{data.table}", "12"),
            &|src| (src == "https://cdn.test/a.png").then(|| "/media-cache/x.png".to_string()),
        );
        assert_eq!(doc.id, "a1");
        assert_eq!(doc.items[0].src, "/media-cache/x.png");
        assert_eq!(doc.items[0].source, "https://cdn.test/a.png");
        assert_eq!(doc.items[0].transition, Transition::Fade);
        assert_eq!(doc.items[1].src, "https://menu.test/?t=12");
        assert_eq!(doc.items[1].transition, Transition::Slide);
        assert_eq!(doc.items[2].src, "http://127.0.0.1/files/b.png");
        assert_eq!(doc.items[2].position, 3);
    }

    #[test]
    fn no_playlist_plays_browser_url_as_one_interactive_item() {
        let doc = player_doc(
            &pick(&[], &[], None, Day::Mon, 0),
            "https://site.test/",
            &|url| url.to_string(),
            &|_| None,
        );
        assert_eq!(doc.id, URL_PLAYLIST);
        assert_eq!(doc.items.len(), 1);
        assert!(doc.items[0].interactive);
        assert!(doc.items[0].bridge);
    }

    #[test]
    fn interactive_origins_cover_every_playlist() {
        let mut menu = PlaylistItem::new(ItemKind::Url, "https://Menu.test:8443/a?b");
        menu.duration_s = Some(5);
        menu.interactive = true;
        let mut shown = PlaylistItem::new(ItemKind::Url, "https://news.test/");
        shown.duration_s = Some(5);
        let all = [
            playlist("a1", "a", vec![menu.clone(), shown]),
            playlist("b2", "b", vec![menu]),
        ];
        assert_eq!(
            interactive_origins(&all),
            vec!["https://menu.test:8443".to_string()]
        );
    }

    #[test]
    fn positions_start_at_one() {
        assert_eq!(index(1, 3, false), Ok(0));
        assert_eq!(index(4, 3, true), Ok(3));
        assert!(index(4, 3, false).is_err());
        assert!(index(0, 3, false).is_err());
    }

    #[test]
    fn entries_are_found_by_a_unique_prefix() {
        let entries = [
            entry("ab12", "x", "1:00", "2:00", 0),
            entry("ab34", "x", "1:00", "2:00", 0),
        ];
        assert_eq!(find_entry(&entries, "ab3"), Ok(1));
        assert!(find_entry(&entries, "ab").is_err());
        assert!(find_entry(&entries, "zz").is_err());
    }
}
