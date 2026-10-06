//! Playlists: the screens the player page shows in turn instead of one URL,
//! and the timetable that picks which playlist plays when.
//!
//! The types on the wire and the checks both ends make, so a client refuses
//! what the device would refuse before sending it. How the player plays a
//! playlist and how the agent keeps it fed is docs/playlists.md.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Moment;

/// Most items one playlist may carry.
pub const PLAYLIST_ITEMS_MAX: usize = 200;
/// Longest item source, in bytes.
pub const PLAYLIST_SRC_MAX: usize = 4096;
/// Longest an image or URL item may stay on screen, seconds: a day.
pub const PLAYLIST_DURATION_MAX: u32 = 86_400;
/// Longest transition, milliseconds.
pub const PLAYLIST_TRANSITION_MAX: u32 = 10_000;
/// Longest wait after a URL item's `load` before it counts as ready.
pub const PLAYLIST_READY_DELAY_MAX: u32 = 60_000;
/// Most timetable entries a device keeps.
pub const TIMETABLE_MAX: usize = 100;
/// The transition a playlist gets when none is given, milliseconds.
pub const TRANSITION_MS_DEFAULT: u32 = 800;
/// How long an interactive item waits for no input when none is given.
pub const IDLE_S_DEFAULT: u32 = 30;
/// Names a playlist cannot take: they are literal paths next to
/// `/api/v1/playlists/{playlist}`.
pub const RESERVED_NAMES: &[&str] = &["timetable", "status"];

/// What an item shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ItemKind {
    /// A web page, in a frame of the player.
    Url,
    /// A picture the browser can decode.
    Image,
    /// A video the browser can play, from start to end or its trim.
    Video,
}

impl ItemKind {
    pub const NAMES: &'static [&'static str] = &["url", "image", "video"];

    pub fn name(self) -> &'static str {
        match self {
            ItemKind::Url => "url",
            ItemKind::Image => "image",
            ItemKind::Video => "video",
        }
    }
}

impl std::str::FromStr for ItemKind {
    type Err = String;
    fn from_str(name: &str) -> Result<Self, String> {
        match name {
            "url" => Ok(ItemKind::Url),
            "image" => Ok(ItemKind::Image),
            "video" => Ok(ItemKind::Video),
            _ => Err(format!("{name:?} is not url, image or video")),
        }
    }
}

/// How one item replaces the one before it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Transition {
    /// At once.
    Cut,
    /// The new item fades in over the old one.
    #[default]
    Fade,
    /// The new item slides in from the right.
    Slide,
}

impl Transition {
    pub const NAMES: &'static [&'static str] = &["cut", "fade", "slide"];

    pub fn name(self) -> &'static str {
        match self {
            Transition::Cut => "cut",
            Transition::Fade => "fade",
            Transition::Slide => "slide",
        }
    }
}

impl std::str::FromStr for Transition {
    type Err = String;
    fn from_str(name: &str) -> Result<Self, String> {
        match name {
            "cut" => Ok(Transition::Cut),
            "fade" => Ok(Transition::Fade),
            "slide" => Ok(Transition::Slide),
            _ => Err(format!("{name:?} is not cut, fade or slide")),
        }
    }
}

/// How an image or a video fills the screen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Fit {
    /// All of it shows, bars where the shapes differ.
    #[default]
    Contain,
    /// The screen is filled, the edges cut where the shapes differ.
    Cover,
    /// The screen is filled, the picture stretched.
    Stretch,
}

impl Fit {
    pub const NAMES: &'static [&'static str] = &["contain", "cover", "stretch"];

    pub fn name(self) -> &'static str {
        match self {
            Fit::Contain => "contain",
            Fit::Cover => "cover",
            Fit::Stretch => "stretch",
        }
    }
}

impl std::str::FromStr for Fit {
    type Err = String;
    fn from_str(name: &str) -> Result<Self, String> {
        match name {
            "contain" => Ok(Fit::Contain),
            "cover" => Ok(Fit::Cover),
            "stretch" => Ok(Fit::Stretch),
            _ => Err(format!("{name:?} is not contain, cover or stretch")),
        }
    }
}

/// One screen of a playlist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlaylistItem {
    pub kind: ItemKind,
    /// An http or https URL: a page, or the image or video file. Files in
    /// the store are `http://127.0.0.1/files/...`; any other image or video
    /// is cached on the device first.
    pub src: String,
    /// How long a URL or image item stays on screen, seconds. For an
    /// interactive item it is the least it stays. A video plays to its end
    /// or its trim, so it takes none.
    #[serde(default)]
    pub duration_s: Option<u32>,
    /// Where a video starts, milliseconds into it.
    #[serde(default)]
    pub trim_start_ms: Option<u64>,
    /// Where a video stops, milliseconds into it.
    #[serde(default)]
    pub trim_end_ms: Option<u64>,
    /// A video plays with its sound; muted otherwise.
    #[serde(default)]
    pub sound: bool,
    /// The video's volume with `sound`, 0-100.
    #[serde(default)]
    pub volume: Option<u8>,
    #[serde(default)]
    pub fit: Fit,
    /// What shows around an image or video that does not fill the screen,
    /// `#rrggbb`. Black when not given.
    #[serde(default)]
    pub background: Option<String>,
    /// How this item comes on screen, instead of the playlist's.
    #[serde(default)]
    pub transition: Option<Transition>,
    #[serde(default)]
    pub transition_ms: Option<u32>,
    /// A URL or image item takes touch and keys: input holds it on screen,
    /// and it moves on once `duration_s` has passed and nobody has touched
    /// it for `idle_s`.
    #[serde(default)]
    pub interactive: bool,
    /// Seconds without input before an interactive item moves on.
    #[serde(default)]
    pub idle_s: Option<u32>,
    /// How long after its `load` a URL item counts as ready to show,
    /// milliseconds, for a page that is still drawing then.
    #[serde(default)]
    pub ready_delay_ms: Option<u32>,
    /// A URL item gets `window.tessaro`, as the kiosk page would
    /// (docs/bridge.md).
    #[serde(default)]
    pub bridge: bool,
}

impl PlaylistItem {
    /// An item of `kind` showing `src`, every option left as it defaults.
    pub fn new(kind: ItemKind, src: &str) -> Self {
        PlaylistItem {
            kind,
            src: src.to_string(),
            duration_s: None,
            trim_start_ms: None,
            trim_end_ms: None,
            sound: false,
            volume: None,
            fit: Fit::default(),
            background: None,
            transition: None,
            transition_ms: None,
            interactive: false,
            idle_s: None,
            ready_delay_ms: None,
            bridge: false,
        }
    }

    /// Whether the device keeps a copy of `src`: an image or video from
    /// anywhere but its own file store.
    pub fn cached(&self) -> bool {
        self.kind != ItemKind::Url && !is_local(&self.src)
    }
}

/// What a playlist is: its items in order and how they change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlaylistSpec {
    /// `[a-z0-9][a-z0-9-]*`, unique on the device.
    pub name: String,
    #[serde(default)]
    pub transition: Transition,
    #[serde(default = "transition_ms_default")]
    pub transition_ms: u32,
    #[serde(default)]
    pub items: Vec<PlaylistItem>,
}

fn transition_ms_default() -> u32 {
    TRANSITION_MS_DEFAULT
}

/// One playlist and where it is used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlaylistInfo {
    /// Stable across renames.
    pub id: String,
    #[serde(flatten)]
    pub spec: PlaylistSpec,
    /// It is `playlist.default`.
    #[serde(default)]
    pub default: bool,
    /// The timetable entries that play it, by id.
    #[serde(default)]
    pub timetable: Vec<String>,
    /// It is on screen now.
    #[serde(default)]
    pub playing: bool,
}

/// A day of the week, as the timetable names it.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Day {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

impl Day {
    pub const ALL: [Day; 7] = [
        Day::Mon,
        Day::Tue,
        Day::Wed,
        Day::Thu,
        Day::Fri,
        Day::Sat,
        Day::Sun,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Day::Mon => "mon",
            Day::Tue => "tue",
            Day::Wed => "wed",
            Day::Thu => "thu",
            Day::Fri => "fri",
            Day::Sat => "sat",
            Day::Sun => "sun",
        }
    }

    /// Monday is 0.
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(index: usize) -> Day {
        Day::ALL[index % 7]
    }

    /// The day before.
    pub fn previous(self) -> Day {
        Day::from_index(self.index() + 6)
    }
}

impl std::str::FromStr for Day {
    type Err = String;
    fn from_str(name: &str) -> Result<Self, String> {
        Day::ALL
            .into_iter()
            .find(|day| day.name() == name)
            .ok_or_else(|| format!("{name:?} is not a day: mon, tue, wed, thu, fri, sat or sun"))
    }
}

/// When a playlist plays instead of the default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TimetableSpec {
    /// The playlist: its id or its name when sent, its id when answered.
    pub playlist: String,
    /// The days the entry starts on. Empty is every day.
    #[serde(default)]
    pub days: Vec<Day>,
    /// `HH:MM`, the device's time.
    pub from: String,
    /// `HH:MM`. Earlier than `from` runs past midnight into the next day;
    /// the same as `from` is the whole day.
    pub to: String,
    /// The highest wins where entries overlap; then the one listed first.
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "crate::yes")]
    pub enabled: bool,
}

/// One timetable entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TimetableInfo {
    pub id: String,
    #[serde(flatten)]
    pub spec: TimetableSpec,
    /// The name of the playlist it plays.
    pub playlist_name: String,
    /// It decides what plays now.
    #[serde(default)]
    pub active: bool,
}

/// Why the player shows what it shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ActiveReason {
    /// A timetable entry covers now.
    Timetable,
    /// No entry does; `playlist.default` plays.
    Default,
    /// Neither: browser.url plays, as a playlist of one.
    Url,
}

/// The item on screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlayingItem {
    /// From 1.
    pub position: u32,
    pub kind: ItemKind,
    pub src: String,
    pub since: Moment,
}

/// An item the player could not show and went past.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkippedItem {
    pub position: u32,
    pub src: String,
    pub reason: String,
    pub at: Moment,
}

/// How far the copies of the playlists' media are.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CacheStatus {
    /// On the device, ready to play offline.
    pub ready: u32,
    /// Not fetched yet; they play from their source meanwhile.
    pub pending: u32,
    /// The last fetch failed and there is no copy.
    pub failed: u32,
}

/// What the player is doing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlaylistStatus {
    /// The player is on screen: `playlist.default` is set or the timetable
    /// has an entry. Otherwise browser.url is shown directly.
    pub player: bool,
    /// The playlist playing, by name; `None` when browser.url plays.
    #[serde(default)]
    pub playlist: Option<String>,
    pub reason: ActiveReason,
    /// The timetable entry that picked it, by id.
    #[serde(default)]
    pub entry: Option<String>,
    /// The item on screen, once the player has said.
    #[serde(default)]
    pub item: Option<PlayingItem>,
    /// The latest items it went past, newest first.
    #[serde(default)]
    pub skipped: Vec<SkippedItem>,
    /// The player has nothing it can show, and the offline page is up.
    #[serde(default)]
    pub nothing_playable: bool,
    #[serde(default)]
    pub cache: CacheStatus,
}

/// How many skipped items `PlaylistStatus` keeps.
pub const SKIPPED_KEPT: usize = 10;

/// `src` is served by the device itself: its file store or its own pages.
pub fn is_local(src: &str) -> bool {
    let rest = src
        .strip_prefix("http://127.0.0.1")
        .or_else(|| src.strip_prefix("http://localhost"));
    matches!(rest, Some(rest) if rest.is_empty() || rest.starts_with('/'))
}

/// The file name a cached copy of `src` gets: SHA-256 of the URL, and the
/// extension of its path when it has a short one, so nginx serves the
/// right type.
pub fn cache_name(src: &str) -> String {
    let hash = crate::hex(&Sha256::digest(src.as_bytes()));
    let path = src.split(['?', '#']).next().unwrap_or("");
    let ext = path
        .rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .filter(|ext| {
            (1..=5).contains(&ext.len()) && ext.bytes().all(|b| b.is_ascii_alphanumeric())
        });
    match ext {
        Some(ext) => format!("{hash}.{ext}"),
        None => hash,
    }
}

/// A playlist name: the rules of every name on the device, and not one of
/// `RESERVED_NAMES`.
pub fn check_playlist_name(name: &str) -> Result<(), String> {
    let valid = !name.is_empty()
        && name.len() <= 40
        && name.starts_with(|ch: char| ch.is_ascii_lowercase() || ch.is_ascii_digit())
        && name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-');
    if !valid {
        return Err(format!(
            "{name:?} is not a playlist name: lower-case letters, digits and -, \
             starting with a letter or digit, at most 40"
        ));
    }
    if RESERVED_NAMES.contains(&name) {
        return Err(format!("{name:?} cannot name a playlist"));
    }
    Ok(())
}

/// `item` made ready to save: trimmed and checked. `position` is from 1,
/// for the message.
pub fn check_item(mut item: PlaylistItem, position: usize) -> Result<PlaylistItem, String> {
    let at = |message: String| format!("item {position}: {message}");
    item.src = item.src.trim().to_string();
    if item.src.is_empty() {
        return Err(at("it needs a src".to_string()));
    }
    if item.src.len() > PLAYLIST_SRC_MAX {
        return Err(at(format!("src is at most {PLAYLIST_SRC_MAX} bytes long")));
    }
    if !(item.src.starts_with("http://") || item.src.starts_with("https://")) {
        return Err(at(format!("{} is not an http or https URL", item.src)));
    }
    if item
        .src
        .chars()
        .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err(at(
            "src cannot carry spaces or control characters".to_string()
        ));
    }
    match item.kind {
        ItemKind::Url | ItemKind::Image => {
            let duration = item
                .duration_s
                .ok_or_else(|| at(format!("a {} item needs a duration", item.kind.name())))?;
            if duration == 0 || duration > PLAYLIST_DURATION_MAX {
                return Err(at(format!(
                    "the duration is 1 to {PLAYLIST_DURATION_MAX} seconds"
                )));
            }
            if item.trim_start_ms.is_some() || item.trim_end_ms.is_some() {
                return Err(at("only a video can be trimmed".to_string()));
            }
            if item.sound || item.volume.is_some() {
                return Err(at("only a video has sound".to_string()));
            }
        }
        ItemKind::Video => {
            if item.duration_s.is_some() {
                return Err(at(
                    "a video plays to its end or its trim, so it takes no duration".to_string(),
                ));
            }
            if item.interactive {
                return Err(at("a video cannot be interactive".to_string()));
            }
            if let (Some(start), Some(end)) = (item.trim_start_ms, item.trim_end_ms) {
                if end <= start {
                    return Err(at("the trim ends before it starts".to_string()));
                }
            }
            if item.trim_end_ms == Some(0) {
                return Err(at("the trim ends before it starts".to_string()));
            }
            if matches!(item.volume, Some(volume) if volume > 100) {
                return Err(at("the volume is 0 to 100".to_string()));
            }
        }
    }
    if item.kind != ItemKind::Url {
        if item.bridge {
            return Err(at("only a URL item gets the bridge".to_string()));
        }
        if item.ready_delay_ms.is_some() {
            return Err(at("only a URL item waits after it loads".to_string()));
        }
    }
    if !item.interactive && item.idle_s.is_some() {
        return Err(at("only an interactive item has an idle time".to_string()));
    }
    if matches!(item.idle_s, Some(idle) if idle == 0 || idle > PLAYLIST_DURATION_MAX) {
        return Err(at(format!(
            "the idle time is 1 to {PLAYLIST_DURATION_MAX} seconds"
        )));
    }
    if matches!(item.ready_delay_ms, Some(delay) if delay > PLAYLIST_READY_DELAY_MAX) {
        return Err(at(format!(
            "the ready delay is at most {PLAYLIST_READY_DELAY_MAX} ms"
        )));
    }
    if matches!(item.transition_ms, Some(ms) if ms > PLAYLIST_TRANSITION_MAX) {
        return Err(at(format!(
            "a transition is at most {PLAYLIST_TRANSITION_MAX} ms"
        )));
    }
    if let Some(background) = &item.background {
        check_color(background).map_err(at)?;
    }
    Ok(item)
}

/// `#rrggbb`.
pub fn check_color(color: &str) -> Result<(), String> {
    let hex = color.strip_prefix('#').unwrap_or("");
    if hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(format!("{color:?} is not a colour: #rrggbb"))
    }
}

/// `spec` made ready to save, its name and every item checked.
pub fn check_playlist(mut spec: PlaylistSpec) -> Result<PlaylistSpec, String> {
    spec.name = spec.name.trim().to_string();
    check_playlist_name(&spec.name)?;
    if spec.transition_ms > PLAYLIST_TRANSITION_MAX {
        return Err(format!(
            "a transition is at most {PLAYLIST_TRANSITION_MAX} ms"
        ));
    }
    if spec.items.len() > PLAYLIST_ITEMS_MAX {
        return Err(format!(
            "a playlist takes at most {PLAYLIST_ITEMS_MAX} items"
        ));
    }
    spec.items = spec
        .items
        .into_iter()
        .enumerate()
        .map(|(index, item)| check_item(item, index + 1))
        .collect::<Result<_, _>>()?;
    Ok(spec)
}

/// `HH:MM` as minutes after midnight; `24:00` is allowed, as an end.
pub fn parse_clock(text: &str) -> Result<u32, String> {
    let wrong = || format!("{text:?} is not a time of day: HH:MM");
    let (hours, minutes) = text.trim().split_once(':').ok_or_else(wrong)?;
    if hours.is_empty() || hours.len() > 2 || minutes.len() != 2 {
        return Err(wrong());
    }
    let hours: u32 = hours.parse().map_err(|_| wrong())?;
    let minutes: u32 = minutes.parse().map_err(|_| wrong())?;
    if minutes > 59 || hours > 24 || (hours == 24 && minutes != 0) {
        return Err(wrong());
    }
    Ok(hours * 60 + minutes)
}

/// Minutes after midnight as `HH:MM`.
pub fn format_clock(minutes: u32) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// `spec`'s times and days checked and written the one way; the playlist
/// is resolved by the agent.
pub fn check_timetable(mut spec: TimetableSpec) -> Result<TimetableSpec, String> {
    let from = parse_clock(&spec.from)?;
    let to = parse_clock(&spec.to)?;
    if from == 24 * 60 {
        return Err("an entry cannot start at 24:00".to_string());
    }
    spec.from = format_clock(from);
    spec.to = format_clock(to);
    spec.days.sort();
    spec.days.dedup();
    if spec.days.len() == Day::ALL.len() {
        spec.days.clear();
    }
    spec.playlist = spec.playlist.trim().to_string();
    if spec.playlist.is_empty() {
        return Err("a timetable entry needs a playlist".to_string());
    }
    Ok(spec)
}

/// Whether `spec` covers `minute` (after midnight) of `day`. An entry that
/// runs past midnight covers the early hours of the day after each of its
/// days.
pub fn covers(spec: &TimetableSpec, day: Day, minute: u32) -> bool {
    let (Ok(from), Ok(to)) = (parse_clock(&spec.from), parse_clock(&spec.to)) else {
        return false;
    };
    let on = |day: Day| spec.days.is_empty() || spec.days.contains(&day);
    if from == to {
        return on(day);
    }
    if from < to {
        return on(day) && minute >= from && minute < to;
    }
    (on(day) && minute >= from) || (on(day.previous()) && minute < to)
}

/// The entry that decides what plays at `minute` of `day`: of the enabled
/// ones covering it, the highest priority, then the one listed first.
/// Returns its index.
pub fn active<'a>(
    entries: impl IntoIterator<Item = &'a TimetableSpec>,
    day: Day,
    minute: u32,
) -> Option<usize> {
    let mut best: Option<(usize, i32)> = None;
    for (index, spec) in entries.into_iter().enumerate() {
        if !spec.enabled || !covers(spec, day, minute) {
            continue;
        }
        if best.is_none_or(|(_, priority)| spec.priority > priority) {
            best = Some((index, spec.priority));
        }
    }
    best.map(|(index, _)| index)
}

/// `mon-fri`, `sat,sun`, `mon,wed-fri`: the days a timetable entry takes on
/// the command line. Empty or `all` is every day.
pub fn parse_days(text: &str) -> Result<Vec<Day>, String> {
    let text = text.trim();
    if text.is_empty() || text == "all" {
        return Ok(Vec::new());
    }
    let mut days = Vec::new();
    for part in text.split(',') {
        let part = part.trim();
        match part.split_once('-') {
            Some((first, last)) => {
                let first: Day = first.trim().parse()?;
                let last: Day = last.trim().parse()?;
                let mut index = first.index();
                loop {
                    days.push(Day::from_index(index));
                    if index % 7 == last.index() {
                        break;
                    }
                    index += 1;
                }
            }
            None => days.push(part.parse()?),
        }
    }
    days.sort();
    days.dedup();
    if days.len() == Day::ALL.len() {
        days.clear();
    }
    Ok(days)
}

/// The days as `parse_days` takes them back: runs of three or more as a
/// range, `every day` for none.
pub fn format_days(days: &[Day]) -> String {
    if days.is_empty() {
        return "every day".to_string();
    }
    let mut indexes: Vec<usize> = days.iter().map(|day| day.index()).collect();
    indexes.sort();
    indexes.dedup();
    let mut parts = Vec::new();
    let mut start = 0;
    while start < indexes.len() {
        let mut end = start;
        while end + 1 < indexes.len() && indexes[end + 1] == indexes[end] + 1 {
            end += 1;
        }
        let first = Day::from_index(indexes[start]).name();
        let last = Day::from_index(indexes[end]).name();
        match end - start {
            0 => parts.push(first.to_string()),
            1 => {
                parts.push(first.to_string());
                parts.push(last.to_string());
            }
            _ => parts.push(format!("{first}-{last}")),
        }
        start = end + 1;
    }
    parts.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(days: &str, from: &str, to: &str, priority: i32) -> TimetableSpec {
        TimetableSpec {
            playlist: "p".to_string(),
            days: parse_days(days).unwrap(),
            from: from.to_string(),
            to: to.to_string(),
            priority,
            enabled: true,
        }
    }

    #[test]
    fn days_parse_ranges_lists_and_wrap() {
        assert_eq!(
            parse_days("mon-fri").unwrap(),
            vec![Day::Mon, Day::Tue, Day::Wed, Day::Thu, Day::Fri]
        );
        assert_eq!(parse_days("sat,sun").unwrap(), vec![Day::Sat, Day::Sun]);
        assert_eq!(
            parse_days("fri-mon").unwrap(),
            vec![Day::Mon, Day::Fri, Day::Sat, Day::Sun]
        );
        assert_eq!(parse_days("mon-sun").unwrap(), Vec::<Day>::new());
        assert_eq!(parse_days("").unwrap(), Vec::<Day>::new());
        assert!(parse_days("monday").is_err());
    }

    #[test]
    fn days_format_back_to_what_parses() {
        for text in ["mon-fri", "sat,sun", "mon,wed-fri", "tue", "every day"] {
            let days = parse_days(if text == "every day" { "" } else { text }).unwrap();
            assert_eq!(format_days(&days), text);
        }
    }

    #[test]
    fn clock_is_hh_mm_with_24_00_as_an_end() {
        assert_eq!(parse_clock("11:30"), Ok(690));
        assert_eq!(parse_clock("9:05"), Ok(545));
        assert_eq!(parse_clock("24:00"), Ok(1440));
        assert!(parse_clock("24:01").is_err());
        assert!(parse_clock("12:60").is_err());
        assert!(parse_clock("1230").is_err());
        assert_eq!(format_clock(545), "09:05");
    }

    #[test]
    fn an_entry_covers_its_window_on_its_days() {
        let lunch = entry("mon-fri", "11:30", "14:00", 0);
        assert!(covers(&lunch, Day::Mon, 11 * 60 + 30));
        assert!(!covers(&lunch, Day::Mon, 14 * 60));
        assert!(!covers(&lunch, Day::Sat, 12 * 60));
    }

    #[test]
    fn an_entry_past_midnight_covers_the_next_morning() {
        let night = entry("fri", "22:00", "02:00", 0);
        assert!(covers(&night, Day::Fri, 23 * 60));
        assert!(covers(&night, Day::Sat, 60));
        assert!(!covers(&night, Day::Sat, 23 * 60));
        assert!(!covers(&night, Day::Fri, 60));
    }

    #[test]
    fn the_same_start_and_end_is_the_whole_day() {
        let all = entry("sun", "00:00", "00:00", 0);
        assert!(covers(&all, Day::Sun, 0));
        assert!(covers(&all, Day::Sun, 1439));
        assert!(!covers(&all, Day::Mon, 0));
    }

    #[test]
    fn the_highest_priority_wins_then_the_first() {
        let entries = [
            entry("", "08:00", "18:00", 0),
            entry("", "11:00", "13:00", 5),
            entry("", "11:00", "13:00", 5),
        ];
        assert_eq!(active(&entries, Day::Tue, 12 * 60), Some(1));
        assert_eq!(active(&entries, Day::Tue, 9 * 60), Some(0));
        assert_eq!(active(&entries, Day::Tue, 20 * 60), None);
    }

    #[test]
    fn a_disabled_entry_never_wins() {
        let mut off = entry("", "00:00", "00:00", 9);
        off.enabled = false;
        assert_eq!(active(&[off], Day::Mon, 0), None);
    }

    #[test]
    fn items_are_checked_by_kind() {
        let mut image = PlaylistItem::new(ItemKind::Image, "http://127.0.0.1/files/a.png");
        assert!(
            check_item(image.clone(), 1).is_err(),
            "an image needs a duration"
        );
        image.duration_s = Some(10);
        assert!(check_item(image.clone(), 1).is_ok());
        image.trim_start_ms = Some(1);
        assert!(check_item(image, 1).is_err());

        let mut video = PlaylistItem::new(ItemKind::Video, "https://cdn.test/a.mp4");
        video.trim_start_ms = Some(5000);
        video.trim_end_ms = Some(10_000);
        assert!(check_item(video.clone(), 1).is_ok());
        video.trim_end_ms = Some(4000);
        assert!(check_item(video.clone(), 1).is_err());
        video.trim_end_ms = None;
        video.duration_s = Some(3);
        assert!(check_item(video, 1).is_err());

        let mut page = PlaylistItem::new(ItemKind::Url, "ftp://x");
        page.duration_s = Some(5);
        assert!(check_item(page, 2).unwrap_err().starts_with("item 2:"));
    }

    #[test]
    fn interactive_is_for_urls_and_images() {
        let mut page = PlaylistItem::new(ItemKind::Url, "https://menu.test/");
        page.duration_s = Some(5);
        page.idle_s = Some(10);
        assert!(
            check_item(page.clone(), 1).is_err(),
            "idle without interactive"
        );
        page.interactive = true;
        assert!(check_item(page, 1).is_ok());
    }

    #[test]
    fn names_follow_the_device_rules_and_skip_the_reserved() {
        assert!(check_playlist_name("lobby-1").is_ok());
        assert!(check_playlist_name("Lobby").is_err());
        assert!(check_playlist_name("timetable").is_err());
    }

    #[test]
    fn local_sources_are_the_device_itself() {
        assert!(is_local("http://127.0.0.1/files/a.mp4"));
        assert!(is_local("http://localhost/media/a.png"));
        assert!(!is_local("http://127.0.0.10/a.png"));
        assert!(!is_local("https://127.0.0.1/a.png"));
        assert!(!is_local("https://cdn.test/a.png"));
    }

    #[test]
    fn cache_names_keep_a_short_extension() {
        let name = cache_name("https://cdn.test/clips/Promo.MP4?v=2");
        assert!(name.ends_with(".mp4"), "{name}");
        assert_eq!(name.len(), 64 + 4);
        assert_eq!(cache_name("https://cdn.test/image").len(), 64);
        assert_ne!(
            cache_name("https://a.test/x.png"),
            cache_name("https://b.test/x.png")
        );
    }
}
