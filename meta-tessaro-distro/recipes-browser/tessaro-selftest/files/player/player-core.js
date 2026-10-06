// The player's decisions, with no DOM in them: which item comes next, when
// an item has been on screen long enough, which part of a video plays, when
// a whole cycle has failed, and what a new playlist.json means for the one
// playing. player.js does everything the screen needs and asks this module
// at every turn, so the rules are unit-tested with `node --test`
// (`mise run player:test`) instead of being found out on a public screen.

export const KINDS = ["image", "video", "url"];
export const FITS = ["contain", "cover", "stretch"];
export const TRANSITIONS = ["cut", "fade", "slide"];

// An item whose JSON leaves a field out still plays: the agent always
// writes every field, so these matter only for a hand-written sample.
const DEFAULT_DURATION_S = 10;
const DEFAULT_TRANSITION_MS = 800;
const DEFAULT_IDLE_S = 30;

// After a full cycle in which every item failed, the cycle is tried again
// this long after, for as long as it keeps failing.
export const RETRY_CYCLE_MS = 30_000;

// How long a prepare may take before the item is skipped.
export const PREPARE_TIMEOUT_MS = 30_000;

// A playing video whose time has not moved for this long is skipped.
export const STALL_MS = 10_000;

// How often the page tells the agent it is alive.
export const HEARTBEAT_MS = 5_000;

function number(value, fallback, min = -Infinity, max = Infinity) {
  if (typeof value !== "number" || !Number.isFinite(value)) return fallback;
  return Math.min(max, Math.max(min, value));
}

function oneOf(value, allowed, fallback) {
  return allowed.includes(value) ? value : fallback;
}

// One item, every field present and in range, keys in one order so two
// playlists compare by their JSON text. An unknown kind is kept: the item
// is then skipped with a reason the agent sees, rather than vanishing.
export function normalizeItem(raw, index) {
  const item = raw && typeof raw === "object" ? raw : {};
  const kind = typeof item.kind === "string" ? item.kind : "";
  const timed = kind === "image" || kind === "url";
  // A trim end at or before the start is kept as it is: trimWindow then
  // finds nothing to play and the item is skipped with a reason, rather
  // than quietly playing the whole video.
  const trimStart = number(item.trim_start_ms, 0, 0);
  const trimEnd = number(item.trim_end_ms, null, 0);
  return {
    position: number(item.position, index + 1),
    kind,
    src: typeof item.src === "string" ? item.src : "",
    source: typeof item.source === "string" ? item.source : String(item.src ?? ""),
    duration_s: timed ? number(item.duration_s, DEFAULT_DURATION_S, 0) || DEFAULT_DURATION_S : null,
    trim_start_ms: trimStart,
    trim_end_ms: trimEnd,
    sound: item.sound === true,
    volume: number(item.volume, 100, 0, 100),
    fit: oneOf(item.fit, FITS, "contain"),
    background: typeof item.background === "string" && item.background ? item.background : "#000000",
    transition: oneOf(item.transition, TRANSITIONS, "cut"),
    transition_ms: number(item.transition_ms, DEFAULT_TRANSITION_MS, 0),
    interactive: item.interactive === true,
    idle_s: number(item.idle_s, DEFAULT_IDLE_S, 0),
    ready_delay_ms: number(item.ready_delay_ms, 0, 0),
    bridge: item.bridge === true,
  };
}

// The whole document, items in the order of their position. Anything that
// is not a playlist at all comes back as null.
export function normalizePlaylist(raw) {
  if (!raw || typeof raw !== "object" || typeof raw.id !== "string") return null;
  const items = Array.isArray(raw.items) ? raw.items.map(normalizeItem) : [];
  items.sort((a, b) => a.position - b.position);
  return {
    id: raw.id,
    name: typeof raw.name === "string" ? raw.name : "",
    items,
  };
}

export function samePlaylist(a, b) {
  return JSON.stringify(a) === JSON.stringify(b);
}

// The index after `index`, back to the first after the last.
export function nextIndex(index, count) {
  if (count <= 0) return -1;
  if (index < 0) return 0;
  return (index + 1) % count;
}

// Where a cycle carries on in `items` after the item at `position` was the
// one on screen: the first item placed after it, else the first of all.
// Positions rather than indices, so a list replaced at a boundary picks up
// where the old one was instead of at an index that now means another item.
export function indexAfterPosition(items, position) {
  if (items.length === 0) return -1;
  if (position === null || position === undefined) return 0;
  const found = items.findIndex((item) => item.position > position);
  return found === -1 ? 0 : found;
}

// Whether the item being prepared is still the one to come next once the
// same playlist arrives with a new item list: an update that changes
// another item - a copy of a picture further on coming in from the cache -
// must not throw away a video that is half decoded.
export function samePending(preparing, items, position) {
  if (!preparing) return false;
  const next = items[indexAfterPosition(items, position)];
  return next !== undefined && JSON.stringify(next) === JSON.stringify(preparing);
}

// Whether a timed item (image or url) has been on screen long enough. An
// interactive item's duration is the least it stays: it also waits until
// nobody has touched it for idle_s, so the playlist never moves on under
// someone's finger. Videos are not timed; they end.
export function shouldAdvance({ elapsedMs, durationS, interactive, idleS, sinceInputMs }) {
  if (durationS === null || durationS === undefined) return false;
  if (elapsedMs < durationS * 1000) return false;
  return !interactive || sinceInputMs >= idleS * 1000;
}

// The part of a video that plays, in seconds: from trim_start_ms to
// trim_end_ms, or to the end when that is null or past it. A video whose
// length is not known (a live stream reports Infinity) plays to trim_end_ms
// or for ever. Null when nothing is left to play.
export function trimWindow(item, durationS) {
  const known = typeof durationS === "number" && Number.isFinite(durationS);
  const start = item.trim_start_ms / 1000;
  let end = item.trim_end_ms === null ? Infinity : item.trim_end_ms / 1000;
  if (known) end = Math.min(end, durationS);
  if (known && start >= durationS) return null;
  if (end <= start) return null;
  return { start, end };
}

// Whether the frame just presented at `mediaTime` is the last one inside
// the trim window. `frameS` is the time between frames as last measured
// (0 before there is a measurement): stopping when the *next* frame would
// be past the end keeps a frame from outside the window off the screen. The
// half millisecond absorbs the rounding in the times Chromium reports.
export function atTrimEnd(mediaTime, end, frameS = 0) {
  if (!Number.isFinite(end)) return false;
  return mediaTime + frameS >= end - 0.0005;
}

// A playlist of one item has no next item to go to: an image or a page is
// shown once and stays, a video loops from its trim start without a
// transition. Null for any other playlist.
export function singleBehaviour(playlist) {
  if (!playlist || playlist.items.length !== 1) return null;
  return playlist.items[0].kind === "video" ? "loop" : "stay";
}

// What a freshly read playlist.json means for the one playing:
//   "switch"   - play it now: another playlist, or nothing playing yet;
//   "boundary" - the same playlist changed, the new list takes over at the
//                next item boundary;
//   "none"     - nothing changed.
// A playlist of one item never reaches a boundary, so a change to it is a
// switch.
export function reloadOutcome(current, next) {
  if (!next) return "none";
  if (!current || current.id !== next.id) return "switch";
  if (samePlaylist(current, next)) return "none";
  if (current.items.length <= 1) return "switch";
  return "boundary";
}

// Failures in a row, counted across one playlist. Every item failing once
// in a row is a full cycle with nothing playable: that is reported once,
// until an item works again, and each further failed cycle waits
// RETRY_CYCLE_MS before the next.
export function newFailures() {
  return { inARow: 0, reported: false };
}

export function recordSuccess() {
  return newFailures();
}

export function recordFailure(failures, count) {
  const inARow = failures.inARow + 1;
  const cycleFailed = count > 0 && inARow >= count;
  const report = cycleFailed && !failures.reported;
  return {
    failures: { inARow, reported: failures.reported || report },
    report,
    waitMs: cycleFailed && inARow % count === 0 ? RETRY_CYCLE_MS : 0,
  };
}

export function nothingPlayable(failures, count) {
  return count > 0 && failures.inARow >= count;
}

// The events the agent reads, one JSON string each.
export function heartbeatEvent(playlist, position) {
  return JSON.stringify({ event: "heartbeat", playlist: playlist ?? null, position: position ?? null });
}

export function startedEvent(playlist, item) {
  return JSON.stringify({ event: "started", playlist, position: item.position, src: item.source });
}

export function skippedEvent(playlist, item, reason) {
  return JSON.stringify({
    event: "skipped",
    playlist,
    position: item.position,
    src: item.source,
    reason,
  });
}

export function nothingPlayableEvent(playlist) {
  return JSON.stringify({ event: "nothing-playable", playlist });
}
