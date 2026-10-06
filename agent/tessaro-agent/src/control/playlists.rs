//! `tessaro-ctl playlist`: the playlists and the timetable in `tessaro.db`
//! (`crate::playlists`), the player page's reports, and the two watchers that
//! keep the page fed.
//!
//! `watch_playlist` writes `/run/tessaro-kiosk/playlist.json` for what plays
//! now - at every minute, since a timetable entry starts and ends on one,
//! and at once after a change - and tells the page to read it again. It
//! also watches the player's heartbeat: a player that stopped reporting
//! while on screen is loaded again. `watch_media` keeps the copies of the
//! playlists' remote media (`crate::media`). A change that moves whether the
//! player is on screen, or the origins its frames get the device grants and
//! the bridge for, renders the configuration again like a setting does.
//! Like the scripts, playlists stay through an unclaim and go with a
//! factory reset.

use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use protocol::playlist::{
    self as wire, ActiveReason, CacheStatus, Day, PlayingItem, PlaylistInfo, PlaylistItem,
    PlaylistSpec, PlaylistStatus, SkippedItem, TimetableInfo, TimetableSpec, Transition,
    SKIPPED_KEPT,
};
use protocol::{keys, Done};
use serde_json::{json, Value};
use tokio::time::Instant;

use super::{Caller, Control, CDP_LIMIT};
use crate::cdp::session::BindingCall;
use crate::deadline::blocking;
use crate::media::{self, MediaCache};
use crate::playlists::{self, Entry, Playlist, Playlists, Timetable};
use crate::state;
use crate::sync::lock;
use crate::watchdog::Heartbeat;

/// A player on screen that has not reported for this long is loaded again.
const STALL: Duration = Duration::from_secs(20);
/// How often the heartbeat is looked at, and the longest the timetable
/// waits past a minute boundary.
const TICK: Duration = Duration::from_secs(5);

/// What the player page last said, kept for `playlist status`.
#[derive(Debug, Default)]
pub struct PlayerState {
    /// The playlist the agent last wrote for the page, and why.
    pub playlist: Option<String>,
    pub name: Option<String>,
    pub reason: Option<ActiveReason>,
    pub entry: Option<String>,
    /// The item the page says is on screen, and since when (seconds since
    /// the epoch).
    pub item: Option<(PlaylistItemShown, i64)>,
    pub skipped: VecDeque<(u32, String, String, i64)>,
    pub nothing_playable: bool,
    pub heartbeat: Option<Instant>,
    /// What the journal was last told plays; `None` while the player is not
    /// on screen.
    pub announced: Option<String>,
    pub cache: CacheStatus,
}

#[derive(Debug, Clone)]
pub struct PlaylistItemShown {
    pub position: u32,
    pub kind: wire::ItemKind,
    pub src: String,
}

/// What `playlist-set` changes; `None` keeps what is there.
pub(super) struct Change {
    pub name: Option<String>,
    pub transition: Option<Transition>,
    pub transition_ms: Option<u32>,
    pub items: Option<Vec<PlaylistItem>>,
}

/// What `timetable-set` changes; `None` keeps what is there.
pub(super) struct EntryChange {
    pub playlist: Option<String>,
    pub days: Option<Vec<Day>>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub priority: Option<i32>,
    pub enabled: Option<bool>,
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

fn now_usec() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_micros() as u64)
}

impl Control {
    async fn read_playlists(&self) -> Result<(Playlists, Timetable), String> {
        let db = self.db.clone();
        let log = Arc::clone(&self.log);
        blocking("reading the playlists", move || {
            Ok((db.read::<Playlists>(&log), db.read::<Timetable>(&log)))
        })
        .await
    }

    /// playlist.default as set, empty when not.
    async fn default_playlist(&self) -> String {
        match self.read_state().await {
            Ok(state) => state::setting(&state.settings, &self.defaults, keys::PLAYLIST_DEFAULT)
                .unwrap_or_default(),
            Err(_) => String::new(),
        }
    }

    /// `config set playlist.default=NAME` names a playlist the device has.
    pub(super) async fn check_playlist(&self, name: &str) -> Result<(), String> {
        let (all, _) = self.read_playlists().await?;
        playlists::find(&all.playlists, name).map(drop)
    }

    fn info(&self, playlist: &Playlist, default: &str, timetable: &[Entry]) -> PlaylistInfo {
        let playing = lock(&self.player).playlist.as_deref() == Some(playlist.id.as_str());
        PlaylistInfo {
            id: playlist.id.clone(),
            spec: playlist.spec.clone(),
            default: default == playlist.spec.name || default == playlist.id,
            timetable: timetable
                .iter()
                .filter(|entry| entry.spec.playlist == playlist.id)
                .map(|entry| entry.id.clone())
                .collect(),
            playing: playing && self.current.borrow().config.player,
        }
    }

    pub(super) async fn playlist_list(&self) -> Result<Vec<PlaylistInfo>, String> {
        let (all, timetable) = self.read_playlists().await?;
        let default = self.default_playlist().await;
        Ok(all
            .playlists
            .iter()
            .map(|playlist| self.info(playlist, &default, &timetable.entries))
            .collect())
    }

    pub(super) async fn playlist_show(&self, query: String) -> Result<PlaylistInfo, String> {
        let (all, timetable) = self.read_playlists().await?;
        let at = playlists::find(&all.playlists, &query)?;
        let default = self.default_playlist().await;
        Ok(self.info(&all.playlists[at], &default, &timetable.entries))
    }

    pub(super) async fn playlist_create(
        &self,
        caller: &Caller,
        spec: PlaylistSpec,
    ) -> Result<PlaylistInfo, String> {
        let _writes = self.writes.lock().await;
        let db = self.db.clone();
        let (id, name) = blocking("updating the playlists", move || {
            db.update(|all: &mut Playlists| {
                let others: Vec<&Playlist> = all.playlists.iter().collect();
                let spec = playlists::validate(spec, &others)?;
                let id = crate::scripts::new_id(|id| {
                    all.playlists.iter().any(|playlist| playlist.id == id)
                })?;
                let name = spec.name.clone();
                all.playlists.push(Playlist {
                    id: id.clone(),
                    spec,
                });
                Ok((id, name))
            })
        })
        .await?;
        self.log.info(format!(
            "playlist {name} ({id}) created by {}",
            caller.describe()
        ));
        self.playlists_changed().await;
        self.playlist_show(id).await
    }

    pub(super) async fn playlist_set(
        &self,
        caller: &Caller,
        query: String,
        change: Change,
    ) -> Result<PlaylistInfo, String> {
        let _writes = self.writes.lock().await;
        let db = self.db.clone();
        let (id, name, renamed) = blocking("updating the playlists", move || {
            db.update(|all: &mut Playlists| {
                let at = playlists::find(&all.playlists, &query)?;
                let mut spec = all.playlists[at].spec.clone();
                let before = spec.name.clone();
                if let Some(name) = change.name {
                    spec.name = name;
                }
                if let Some(transition) = change.transition {
                    spec.transition = transition;
                }
                if let Some(ms) = change.transition_ms {
                    spec.transition_ms = ms;
                }
                if let Some(items) = change.items {
                    spec.items = items;
                }
                let id = all.playlists[at].id.clone();
                let others: Vec<&Playlist> = all
                    .playlists
                    .iter()
                    .filter(|other| other.id != id)
                    .collect();
                let spec = playlists::validate(spec, &others)?;
                let renamed = (spec.name != before).then_some(before);
                let name = spec.name.clone();
                all.playlists[at].spec = spec;
                Ok((id, name, renamed))
            })
        })
        .await?;
        if let Some(before) = renamed {
            self.follow_rename(&before, &name).await;
        }
        self.log.info(format!(
            "playlist {name} ({id}) changed by {}",
            caller.describe()
        ));
        self.playlists_changed().await;
        self.playlist_show(id).await
    }

    /// playlist.default names a playlist by name: a rename takes it along.
    /// Whether the player is on screen does not move, so nothing is
    /// rendered again; the revision does, as for any change to a setting.
    async fn follow_rename(&self, before: &str, after: &str) {
        if self.default_playlist().await != before {
            return;
        }
        let db = self.db.clone();
        let after = after.to_string();
        let saved = blocking("updating the settings", move || {
            db.update(|state: &mut crate::state::State| {
                state
                    .settings
                    .insert(keys::PLAYLIST_DEFAULT.to_string(), after);
                state.revision += 1;
                Ok(())
            })
        })
        .await;
        if let Err(err) = saved {
            self.log
                .info(format!("playlist.default kept the old name: {err}"));
        }
    }

    pub(super) async fn playlist_remove(
        &self,
        caller: &Caller,
        query: String,
    ) -> Result<Done, String> {
        let _writes = self.writes.lock().await;
        let default = self.default_playlist().await;
        let db = self.db.clone();
        let removed = blocking("updating the playlists", move || {
            let timetable = db.read::<Timetable>(&crate::log::Log::new(false));
            db.update(|all: &mut Playlists| {
                let at = playlists::find(&all.playlists, &query)?;
                let playlist = &all.playlists[at];
                if default == playlist.spec.name || default == playlist.id {
                    return Err(format!(
                        "{} is playlist.default; `tessaro-ctl config unset playlist.default` first",
                        playlist.spec.name
                    ));
                }
                if timetable
                    .entries
                    .iter()
                    .any(|entry| entry.spec.playlist == playlist.id)
                {
                    return Err(format!(
                        "{} is in the timetable; `tessaro-ctl playlist timetable remove` its entries first",
                        playlist.spec.name
                    ));
                }
                Ok(all.playlists.remove(at))
            })
        })
        .await?;
        self.log.info(format!(
            "playlist {} ({}) removed by {}",
            removed.spec.name,
            removed.id,
            caller.describe()
        ));
        self.playlists_changed().await;
        Ok(Done::new(format!("removed playlist {}", removed.spec.name)))
    }

    /// Change the items of one playlist with `edit`, checked and saved as a
    /// whole.
    async fn playlist_items(
        &self,
        caller: &Caller,
        query: String,
        what: &'static str,
        edit: impl FnOnce(&mut Vec<PlaylistItem>) -> Result<(), String> + Send + 'static,
    ) -> Result<PlaylistInfo, String> {
        let _writes = self.writes.lock().await;
        let db = self.db.clone();
        let (id, name) = blocking("updating the playlists", move || {
            db.update(|all: &mut Playlists| {
                let at = playlists::find(&all.playlists, &query)?;
                let mut spec = all.playlists[at].spec.clone();
                edit(&mut spec.items)?;
                let id = all.playlists[at].id.clone();
                let others: Vec<&Playlist> = all
                    .playlists
                    .iter()
                    .filter(|other| other.id != id)
                    .collect();
                let spec = playlists::validate(spec, &others)?;
                let name = spec.name.clone();
                all.playlists[at].spec = spec;
                Ok((id, name))
            })
        })
        .await?;
        self.log.info(format!(
            "playlist {name} ({id}): item {what} by {}",
            caller.describe()
        ));
        self.playlists_changed().await;
        self.playlist_show(id).await
    }

    pub(super) async fn playlist_item_add(
        &self,
        caller: &Caller,
        query: String,
        item: PlaylistItem,
        at: Option<u32>,
    ) -> Result<PlaylistInfo, String> {
        self.playlist_items(caller, query, "added", move |items| {
            let index = match at {
                Some(position) => playlists::index(position, items.len(), true)?,
                None => items.len(),
            };
            items.insert(index, item);
            Ok(())
        })
        // naked: playlist_items waits only through blocking() and the writes lock
        .await
    }

    pub(super) async fn playlist_item_set(
        &self,
        caller: &Caller,
        query: String,
        position: u32,
        item: PlaylistItem,
    ) -> Result<PlaylistInfo, String> {
        self.playlist_items(caller, query, "changed", move |items| {
            let index = playlists::index(position, items.len(), false)?;
            items[index] = item;
            Ok(())
        })
        // naked: playlist_items waits only through blocking() and the writes lock
        .await
    }

    pub(super) async fn playlist_item_remove(
        &self,
        caller: &Caller,
        query: String,
        position: u32,
    ) -> Result<PlaylistInfo, String> {
        self.playlist_items(caller, query, "removed", move |items| {
            let index = playlists::index(position, items.len(), false)?;
            items.remove(index);
            Ok(())
        })
        // naked: playlist_items waits only through blocking() and the writes lock
        .await
    }

    pub(super) async fn playlist_item_move(
        &self,
        caller: &Caller,
        query: String,
        position: u32,
        to: u32,
    ) -> Result<PlaylistInfo, String> {
        self.playlist_items(caller, query, "moved", move |items| {
            let from = playlists::index(position, items.len(), false)?;
            let to = playlists::index(to, items.len(), false)?;
            let item = items.remove(from);
            items.insert(to, item);
            Ok(())
        })
        // naked: playlist_items waits only through blocking() and the writes lock
        .await
    }

    pub(super) async fn timetable_list(&self) -> Result<Vec<TimetableInfo>, String> {
        let (all, timetable) = self.read_playlists().await?;
        let active = lock(&self.player).entry.clone();
        let player = self.current.borrow().config.player;
        Ok(timetable
            .entries
            .iter()
            .map(|entry| timetable_info(entry, &all.playlists, player, active.as_deref()))
            .collect())
    }

    async fn timetable_info_of(&self, id: &str) -> Result<TimetableInfo, String> {
        self.timetable_list()
            .await?
            .into_iter()
            .find(|entry| entry.id == id)
            .ok_or_else(|| format!("no timetable entry {id}"))
    }

    pub(super) async fn timetable_create(
        &self,
        caller: &Caller,
        spec: TimetableSpec,
    ) -> Result<TimetableInfo, String> {
        let _writes = self.writes.lock().await;
        let db = self.db.clone();
        let log = Arc::clone(&self.log);
        let id = blocking("updating the timetable", move || {
            let all = db.read::<Playlists>(&log);
            db.update(|timetable: &mut Timetable| {
                if timetable.entries.len() >= wire::TIMETABLE_MAX {
                    return Err(format!(
                        "the timetable takes at most {} entries",
                        wire::TIMETABLE_MAX
                    ));
                }
                let spec = playlists::validate_entry(spec, &all.playlists)?;
                let id = crate::scripts::new_id(|id| {
                    timetable.entries.iter().any(|entry| entry.id == id)
                })?;
                timetable.entries.push(Entry {
                    id: id.clone(),
                    spec,
                });
                Ok(id)
            })
        })
        .await?;
        self.log.info(format!(
            "timetable entry {id} created by {}",
            caller.describe()
        ));
        self.playlists_changed().await;
        self.timetable_info_of(&id).await
    }

    pub(super) async fn timetable_set(
        &self,
        caller: &Caller,
        query: String,
        change: EntryChange,
    ) -> Result<TimetableInfo, String> {
        let _writes = self.writes.lock().await;
        let db = self.db.clone();
        let log = Arc::clone(&self.log);
        let id = blocking("updating the timetable", move || {
            let all = db.read::<Playlists>(&log);
            db.update(|timetable: &mut Timetable| {
                let at = playlists::find_entry(&timetable.entries, &query)?;
                let mut spec = timetable.entries[at].spec.clone();
                if let Some(playlist) = change.playlist {
                    spec.playlist = playlist;
                }
                if let Some(days) = change.days {
                    spec.days = days;
                }
                if let Some(from) = change.from {
                    spec.from = from;
                }
                if let Some(to) = change.to {
                    spec.to = to;
                }
                if let Some(priority) = change.priority {
                    spec.priority = priority;
                }
                if let Some(enabled) = change.enabled {
                    spec.enabled = enabled;
                }
                timetable.entries[at].spec = playlists::validate_entry(spec, &all.playlists)?;
                Ok(timetable.entries[at].id.clone())
            })
        })
        .await?;
        self.log.info(format!(
            "timetable entry {id} changed by {}",
            caller.describe()
        ));
        self.playlists_changed().await;
        self.timetable_info_of(&id).await
    }

    pub(super) async fn timetable_remove(
        &self,
        caller: &Caller,
        query: String,
    ) -> Result<Done, String> {
        let _writes = self.writes.lock().await;
        let db = self.db.clone();
        let removed = blocking("updating the timetable", move || {
            db.update(|timetable: &mut Timetable| {
                let at = playlists::find_entry(&timetable.entries, &query)?;
                Ok(timetable.entries.remove(at))
            })
        })
        .await?;
        self.log.info(format!(
            "timetable entry {} removed by {}",
            removed.id,
            caller.describe()
        ));
        self.playlists_changed().await;
        Ok(Done::new(format!("removed timetable entry {}", removed.id)))
    }

    pub(super) async fn playlist_status(&self) -> Result<PlaylistStatus, String> {
        let player = self.current.borrow().config.player;
        let (reason, playlist, entry, item, skipped, nothing_playable, cache) = {
            let state = lock(&self.player);
            (
                state.reason.unwrap_or(ActiveReason::Url),
                state.name.clone().filter(|name| !name.is_empty()),
                state.entry.clone(),
                state.item.clone(),
                state.skipped.clone(),
                state.nothing_playable,
                state.cache.clone(),
            )
        };
        blocking("reading the local time", move || {
            Ok(PlaylistStatus {
                player,
                playlist,
                reason,
                entry,
                item: item.map(|(shown, since)| PlayingItem {
                    position: shown.position,
                    kind: shown.kind,
                    src: shown.src,
                    since: crate::schedules::moment(since),
                }),
                skipped: skipped
                    .into_iter()
                    .map(|(position, src, reason, at)| SkippedItem {
                        position,
                        src,
                        reason,
                        at: crate::schedules::moment(at),
                    })
                    .collect(),
                nothing_playable: player && nothing_playable,
                cache,
            })
        })
        .await
    }

    /// The store changed: the configuration is rendered again when whether
    /// the player is on screen or its frames' origins moved (what the agent
    /// runs on carries the `playlists::Shared` it was made with), and the
    /// watchers are woken. Holds `writes`, like every change.
    async fn playlists_changed(&self) {
        let before = lock(&self.live_playlists).clone();
        let now = match self.read_playlists().await {
            Ok((all, timetable)) => playlists::Shared::of(&all, &timetable),
            Err(err) => {
                self.log
                    .info(format!("playlists: could not read them back: {err}"));
                return;
            }
        };
        if now != before {
            *lock(&self.live_playlists) = now;
            match self.read_state().await {
                Ok(state) => {
                    // naked: converge waits only through blocking(), the Bus and Network, each bounded
                    let reply = self.converge(&[], &state, true, None).await;
                    if let Err(err) = &reply.result {
                        self.log
                            .info(format!("playlists: applying the change: {err}"));
                    }
                    if let Some(after) = reply.after {
                        // naked: run_after waits only through Bus and Network, whose calls are within()
                        self.run_after(after).await;
                    }
                }
                Err(err) => self
                    .log
                    .info(format!("playlists: applying the change: {err}")),
            }
            self.poke_bridge();
        }
        self.playlist_wake.notify_one();
        self.media_wake.notify_one();
    }

    /// One call of the player's binding: a report from the player page, or
    /// input in one of its frames.
    pub(super) async fn player_call(&self, call: BindingCall) {
        let Ok(report) = serde_json::from_str::<Value>(&call.payload) else {
            return;
        };
        let event = report["event"].as_str().unwrap_or("");
        if event == "input" {
            // naked: SessionHandle::call bounds itself with within()
            let _ = self
                .session
                .call(
                    &Heartbeat::detached(),
                    "Runtime.evaluate",
                    json!({ "expression": "window.__tessaroPlayer && window.__tessaroPlayer.input()" }),
                    CDP_LIMIT,
                )
                .await;
            return;
        }
        // Reports come from the player page itself, never from a frame.
        let player_origin = playlists::origin(&self.player_url()).unwrap_or_default();
        if !call.top || call.origin != player_origin {
            self.log.debug(format!(
                "playlist: ignored a report from {:?} (top frame: {})",
                call.origin, call.top
            ));
            return;
        }
        let position = report["position"].as_u64().map(|position| position as u32);
        let src = report["src"].as_str().unwrap_or("").to_string();
        let mut state = lock(&self.player);
        state.heartbeat = Some(Instant::now());
        match event {
            "started" => {
                if state.nothing_playable {
                    self.log.info("playlist: playing again");
                }
                state.nothing_playable = false;
                if let Some(position) = position {
                    let kind = report["kind"]
                        .as_str()
                        .and_then(|kind| kind.parse().ok())
                        .unwrap_or(wire::ItemKind::Url);
                    self.log
                        .debug(format!("playlist: item {position} on screen: {src}"));
                    state.item = Some((
                        PlaylistItemShown {
                            position,
                            kind,
                            src,
                        },
                        now_unix(),
                    ));
                }
            }
            "skipped" => {
                let reason = report["reason"]
                    .as_str()
                    .unwrap_or("it did not load")
                    .chars()
                    .filter(|ch| !ch.is_control())
                    .take(200)
                    .collect::<String>();
                self.log.info(format!(
                    "playlist: skipped item {} ({src}): {reason}",
                    position.unwrap_or(0)
                ));
                state
                    .skipped
                    .push_front((position.unwrap_or(0), src, reason, now_unix()));
                state.skipped.truncate(SKIPPED_KEPT);
            }
            "nothing-playable" => {
                if !state.nothing_playable {
                    self.log.info(
                        "playlist: nothing in the playlist can be shown; the offline page is up",
                    );
                }
                state.nothing_playable = true;
                state.item = None;
            }
            _ => {}
        }
    }

    fn player_url(&self) -> String {
        self.defaults
            .get("KIOSK_PLAYER_URL")
            .filter(|url| !url.is_empty())
            .cloned()
            .unwrap_or_else(|| state::PLAYER_URL.to_string())
    }

    /// Write what plays now for the page, and tell it when that changed.
    /// Returns whether it did.
    pub(super) async fn refresh_playlist(&self) -> Result<bool, String> {
        let (all, timetable) = self.read_playlists().await?;
        let state = self.read_state().await?;
        let live = self.live().await;
        let default = state::setting(&state.settings, &self.defaults, keys::PLAYLIST_DEFAULT);
        let clock = blocking("reading the local time", move || {
            Ok(crate::time::local_minute(now_usec()))
        })
        .await?;
        let (day, minute) = clock
            .map(|(day, minute, _)| (Day::from_index(day), minute))
            .unwrap_or((Day::Mon, 0));
        let pick = playlists::pick(
            &all.playlists,
            &timetable.entries,
            default.as_deref(),
            day,
            minute,
        );
        let url_template = self.template(&state.settings, keys::URL);
        let (url, _) = state::expand_url(&url_template, &state.settings, &self.defaults, &live);
        let expand =
            |template: &str| state::expand_url(template, &state.settings, &self.defaults, &live).0;

        let dir = self.paths.media_cache_dir();
        let db = self.db.clone();
        let log = Arc::clone(&self.log);
        let local = blocking("reading the media cache", move || {
            let cache = db.read::<MediaCache>(&log);
            Ok(media::local(&cache, &|file| media::present(&dir, file)))
        })
        .await?;
        let doc = playlists::player_doc(&pick, &url, &expand, &|src| local.get(src).cloned());
        let body = serde_json::to_vec_pretty(&doc).map_err(|err| err.to_string())?;

        // Said once each time what plays, or why, moves while the player is
        // on screen - and again when the player comes on screen, whatever
        // was picked while it was not.
        let player = self.current.borrow().config.player;
        let announcement = player.then(|| match (pick.playlist, &pick.entry) {
            (Some(playlist), Some(entry)) => format!(
                "playlist: playing {} (timetable entry {entry})",
                playlist.spec.name
            ),
            (Some(playlist), None) => format!(
                "playlist: playing {} (playlist.default)",
                playlist.spec.name
            ),
            (None, _) => "playlist: playing browser.url".to_string(),
        });
        let announce = {
            let mut held = lock(&self.player);
            if held.playlist.as_deref() != Some(doc.id.as_str()) {
                held.item = None;
            }
            held.playlist = Some(doc.id.clone());
            held.name = pick.playlist.map(|playlist| playlist.spec.name.clone());
            held.reason = Some(pick.reason);
            held.entry = pick.entry.clone();
            let announce = announcement.is_some() && held.announced != announcement;
            held.announced = announcement.clone();
            announce
        };
        if let (true, Some(line)) = (announce, announcement) {
            self.log.info(line);
        }

        let path = self.paths.player_doc();
        let written = blocking("writing the playlist for the player", move || {
            crate::store::replace_if_changed(&path, &body, 0o644)
                .map_err(|err| format!("{}: {err}", path.display()))
        })
        .await?;
        if written && self.current.borrow().config.player {
            // The page reads it again; one that is not up yet reads it when
            // it starts.
            // naked: SessionHandle::call bounds itself with within()
            let _ = self
                .session
                .call(
                    &Heartbeat::detached(),
                    "Runtime.evaluate",
                    json!({ "expression": "window.__tessaroPlayer && window.__tessaroPlayer.reload()" }),
                    CDP_LIMIT,
                )
                .await;
        }
        Ok(written)
    }

    /// The player is on screen and stopped reporting: load it again. Not
    /// while someone is in DevTools, nor before it had the time to start.
    async fn check_player(&self, shown_since: &mut Option<Instant>, nudged: &mut Option<Instant>) {
        let config = self.current.borrow().config.clone();
        let on_player = config.player
            && self.session.is_up()
            && self
                .session
                .current_url()
                .is_some_and(|url| url.starts_with(&config.kiosk_url));
        if !on_player {
            *shown_since = None;
            return;
        }
        let since = *shown_since.get_or_insert_with(Instant::now);
        let last = lock(&self.player).heartbeat;
        let quiet_since = match last {
            Some(beat) if beat > since => beat,
            _ => since,
        };
        if quiet_since.elapsed() < STALL || nudged.is_some_and(|at| at.elapsed() < STALL) {
            return;
        }
        // naked: others() waits only through blocking()
        if self.session.others().await > 0 {
            return;
        }
        self.log
            .info("playlist: the player stopped reporting; loading it again");
        *nudged = Some(Instant::now());
        *shown_since = None;
        // naked: SessionHandle::call bounds itself with within()
        let _ = self
            .session
            .call(
                &Heartbeat::detached(),
                "Page.navigate",
                json!({ "url": config.kiosk_url }),
                CDP_LIMIT,
            )
            .await;
    }

    /// Keeps `playlist.json` on what plays now and the player on screen
    /// alive: every minute boundary, at once after a change to the store or
    /// the configuration, and the heartbeat every `TICK`.
    pub fn watch_playlist(self: &Arc<Self>) {
        let control = Arc::clone(self);
        let mut shutdown = self.shutdown.clone();
        let mut config = self.current.subscribe();
        tokio::spawn(async move {
            let mut minute: Option<(usize, u32)> = None;
            let mut due = true;
            let mut shown_since: Option<Instant> = None;
            let mut nudged: Option<Instant> = None;
            loop {
                let clock = blocking("reading the local time", move || {
                    Ok(crate::time::local_minute(now_usec()))
                })
                .await
                .ok()
                .flatten();
                let now = clock.map(|(day, minute, _)| (day, minute));
                if now != minute {
                    due = true;
                }
                if due {
                    // naked: refresh_playlist waits only through blocking() and the session's own within()
                    match control.refresh_playlist().await {
                        Ok(_) => {
                            minute = now;
                            due = false;
                        }
                        Err(err) => control.log.info(format!("playlist: {err}")),
                    }
                }
                // naked: check_player waits only through blocking() and the session's own within()
                control.check_player(&mut shown_since, &mut nudged).await;

                // Up to the next minute boundary, at most a tick.
                let second = clock.map_or(0, |(_, _, second)| second);
                let wait = TICK.min(Duration::from_secs(u64::from(60 - second.min(59))));
                // naked: a timer, a notification and the shutdown signal, not the outside world
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = control.playlist_wake.notified() => due = true,
                    changed = config.changed() => {
                        if changed.is_err() {
                            return;
                        }
                        due = true;
                    }
                    _ = shutdown.changed() => return,
                }
            }
        });
    }

    /// The sources the media cache keeps: the image and video items of every
    /// playlist the player can play - the default and those in the
    /// timetable - that do not come from the device itself.
    async fn wanted_media(&self) -> Result<BTreeSet<String>, String> {
        let (all, timetable) = self.read_playlists().await?;
        let default = self.default_playlist().await;
        let mut playable: BTreeSet<&str> = timetable
            .entries
            .iter()
            .map(|entry| entry.spec.playlist.as_str())
            .collect();
        if let Ok(at) = playlists::find(&all.playlists, &default) {
            playable.insert(all.playlists[at].id.as_str());
        }
        Ok(all
            .playlists
            .iter()
            .filter(|playlist| playable.contains(playlist.id.as_str()))
            .flat_map(|playlist| &playlist.spec.items)
            .filter(|item| item.cached())
            .map(|item| item.src.clone())
            .collect())
    }

    /// One round of the media cache: what is due is fetched, one source at a
    /// time, what nothing wants goes, and the page is told when a copy came.
    async fn refresh_media(&self, http: &crate::http::HyperHttp) -> Result<(), String> {
        let wanted = self.wanted_media().await?;
        let dir = self.paths.media_cache_dir();
        let db = self.db.clone();
        let log = Arc::clone(&self.log);
        let (cache, due) = {
            let dir = dir.clone();
            let wanted = wanted.clone();
            blocking("reading the media cache", move || {
                media::make_dir(&dir)?;
                let mut cache = db.read::<MediaCache>(&log);
                let removed = media::evict(&dir, &mut cache, &wanted);
                if removed > 0 {
                    db.update(|stored: &mut MediaCache| {
                        stored.entries.retain(|entry| wanted.contains(&entry.url));
                        Ok(())
                    })?;
                }
                let due = media::due(&cache, &wanted, now_unix(), &|file| {
                    media::present(&dir, file)
                });
                Ok((cache, due))
            })
            .await?
        };
        let mut fresh = false;
        for url in due {
            let known = cache.get(&url).cloned();
            let present = known
                .as_ref()
                .is_some_and(|known| media::present(&dir, &known.file));
            // naked: fetch() waits only through download()'s own within()s and blocking()
            let outcome = media::fetch(http, &dir, &url, known.as_ref(), present).await;
            match &outcome {
                Ok(media::Fetched::Fresh { size, .. }) => {
                    fresh = true;
                    self.log.info(format!(
                        "playlist: cached {url} ({})",
                        update::megabytes(*size)
                    ));
                }
                Ok(media::Fetched::NotModified) => {}
                Err(err) => self
                    .log
                    .info(format!("playlist: could not cache {url}: {err}")),
            }
            let db = self.db.clone();
            let url = url.clone();
            blocking("updating the media cache", move || {
                db.update(|stored: &mut MediaCache| {
                    stored.record(&url, now_unix(), &outcome);
                    Ok(())
                })
            })
            .await?;
        }
        let db = self.db.clone();
        let log = Arc::clone(&self.log);
        let status = blocking("reading the media cache", move || {
            let cache = db.read::<MediaCache>(&log);
            Ok(media::status(&cache, &wanted, &|file| {
                media::present(&dir, file)
            }))
        })
        .await?;
        lock(&self.player).cache = status;
        if fresh {
            self.playlist_wake.notify_one();
        }
        Ok(())
    }

    /// Keeps the media cache on the playlists: every minute, and at once
    /// after a change.
    pub fn watch_media(self: &Arc<Self>) {
        let control = Arc::clone(self);
        let mut shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            let http = media::client(control.proxy);
            loop {
                // naked: refresh_media waits only through blocking() and the client's own within()s
                if let Err(err) = control.refresh_media(&http).await {
                    control
                        .log
                        .info(format!("playlist: the media cache: {err}"));
                }
                // naked: a timer, a notification and the shutdown signal, not the outside world
                tokio::select! {
                    _ = tokio::time::sleep(media::TICK) => {}
                    _ = control.media_wake.notified() => {}
                    _ = shutdown.changed() => return,
                }
            }
        });
    }
}

fn timetable_info(
    entry: &Entry,
    playlists: &[Playlist],
    player: bool,
    active: Option<&str>,
) -> TimetableInfo {
    let playlist_name = playlists::find(playlists, &entry.spec.playlist)
        .map(|at| playlists[at].spec.name.clone())
        .unwrap_or_else(|_| entry.spec.playlist.clone());
    TimetableInfo {
        id: entry.id.clone(),
        spec: entry.spec.clone(),
        playlist_name,
        active: player && active == Some(entry.id.as_str()),
    }
}
