// agent/client/src/describe/playlist.rs, function for function, and what
// the Playlists page needs of agent/client/src/playlist.rs: the spans of
// time read and written the one way, an item built or changed from what
// was typed, and a timetable entry from typed days and times. The checks
// are protocol::playlist's, so a dialog refuses what the device would.

import type { Schemas } from "../api/client";
import { fact, Line, type Fact, type Tone } from "../text/line";
import { formatTimeout, relative } from "./schedule";

type Item = Schemas["PlaylistItem"];
type ItemKind = Schemas["ItemKind"];
type Transition = Schemas["Transition"];
type Fit = Schemas["Fit"];
type Day = Schemas["Day"];

// --- protocol::playlist ---

/** protocol::playlist's limits. */
export const PLAYLIST_SRC_MAX = 4096;
export const PLAYLIST_DURATION_MAX = 86_400;
export const PLAYLIST_TRANSITION_MAX = 10_000;
export const PLAYLIST_READY_DELAY_MAX = 60_000;
export const TRANSITION_MS_DEFAULT = 800;
export const RESERVED_NAMES = ["timetable", "status"];

export const ITEM_KINDS: ItemKind[] = ["url", "image", "video"];
export const TRANSITIONS: Transition[] = ["cut", "fade", "slide"];
export const FITS: Fit[] = ["contain", "cover", "stretch"];
export const DAYS: Day[] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/** The setting that names the playlist shown when no timetable entry is. */
export const PLAYLIST_DEFAULT = "playlist.default";

/** Rust's `{:?}` of a string, near enough for a refusal. */
function quoted(text: string): string {
  return JSON.stringify(text);
}

export function parseKind(name: string): ItemKind {
  if ((ITEM_KINDS as string[]).includes(name)) return name as ItemKind;
  throw new Error(`${quoted(name)} is not url, image or video`);
}

export function parseTransition(name: string): Transition {
  if ((TRANSITIONS as string[]).includes(name)) return name as Transition;
  throw new Error(`${quoted(name)} is not cut, fade or slide`);
}

export function parseFit(name: string): Fit {
  if ((FITS as string[]).includes(name)) return name as Fit;
  throw new Error(`${quoted(name)} is not contain, cover or stretch`);
}

/** protocol::playlist::is_local: served by the device itself. */
export function isLocal(src: string): boolean {
  const rest = src.startsWith("http://127.0.0.1")
    ? src.slice("http://127.0.0.1".length)
    : src.startsWith("http://localhost")
      ? src.slice("http://localhost".length)
      : null;
  return rest !== null && (rest === "" || rest.startsWith("/"));
}

/** protocol::playlist::check_playlist_name. */
export function checkPlaylistName(name: string): void {
  const valid = name.length > 0 && name.length <= 40 && /^[a-z0-9][a-z0-9-]*$/.test(name);
  if (!valid) {
    throw new Error(
      `${quoted(name)} is not a playlist name: lower-case letters, digits and -, starting with a letter or digit, at most 40`,
    );
  }
  if (RESERVED_NAMES.includes(name)) {
    throw new Error(`${quoted(name)} cannot name a playlist`);
  }
}

/** protocol::playlist::check_color: `#rrggbb`. */
export function checkColor(color: string): void {
  if (!/^#[0-9a-fA-F]{6}$/.test(color)) {
    throw new Error(`${quoted(color)} is not a colour: #rrggbb`);
  }
}

/** protocol::playlist::check_item: the item as the device keeps it, or why not. */
export function checkItem(given: Item, position: number): Item {
  const at = (message: string) => new Error(`item ${position}: ${message}`);
  const item = { ...given, src: given.src.trim() };
  if (item.src === "") throw at("it needs a src");
  if (new TextEncoder().encode(item.src).length > PLAYLIST_SRC_MAX) {
    throw at(`src is at most ${PLAYLIST_SRC_MAX} bytes long`);
  }
  if (!(item.src.startsWith("http://") || item.src.startsWith("https://"))) {
    throw at(`${item.src} is not an http or https URL`);
  }
  // Rust's char::is_control and char::is_whitespace.
  if (/[\p{Cc}\s\u0085]/u.test(item.src)) throw at("src cannot carry spaces or control characters");
  if (item.kind === "url" || item.kind === "image") {
    if (item.duration_s === null) throw at(`a ${item.kind} item needs a duration`);
    if (item.duration_s === 0 || item.duration_s > PLAYLIST_DURATION_MAX) {
      throw at(`the duration is 1 to ${PLAYLIST_DURATION_MAX} seconds`);
    }
    if (item.trim_start_ms !== null || item.trim_end_ms !== null) throw at("only a video can be trimmed");
    if (item.sound || item.volume !== null) throw at("only a video has sound");
  } else {
    if (item.duration_s !== null) throw at("a video plays to its end or its trim, so it takes no duration");
    if (item.interactive) throw at("a video cannot be interactive");
    if (item.trim_start_ms !== null && item.trim_end_ms !== null && item.trim_end_ms <= item.trim_start_ms) {
      throw at("the trim ends before it starts");
    }
    if (item.trim_end_ms === 0) throw at("the trim ends before it starts");
    if (item.volume !== null && item.volume > 100) throw at("the volume is 0 to 100");
  }
  if (item.kind !== "url") {
    if (item.bridge) throw at("only a URL item gets the bridge");
    if (item.ready_delay_ms !== null) throw at("only a URL item waits after it loads");
  }
  if (!item.interactive && item.idle_s !== null) throw at("only an interactive item has an idle time");
  if (item.idle_s !== null && (item.idle_s === 0 || item.idle_s > PLAYLIST_DURATION_MAX)) {
    throw at(`the idle time is 1 to ${PLAYLIST_DURATION_MAX} seconds`);
  }
  if (item.ready_delay_ms !== null && item.ready_delay_ms > PLAYLIST_READY_DELAY_MAX) {
    throw at(`the ready delay is at most ${PLAYLIST_READY_DELAY_MAX} ms`);
  }
  if (item.transition_ms !== null && item.transition_ms > PLAYLIST_TRANSITION_MAX) {
    throw at(`a transition is at most ${PLAYLIST_TRANSITION_MAX} ms`);
  }
  if (item.background !== null) {
    try {
      checkColor(item.background);
    } catch (problem) {
      throw at((problem as Error).message);
    }
  }
  return item;
}

/** protocol::playlist::parse_clock: minutes since midnight, 24:00 the end of a day. */
export function parseClock(text: string): number {
  const wrong = () => new Error(`${quoted(text)} is not a time of day: HH:MM`);
  const trimmed = text.trim();
  const colon = trimmed.indexOf(":");
  if (colon < 0) throw wrong();
  const hoursText = trimmed.slice(0, colon);
  const minutesText = trimmed.slice(colon + 1);
  if (hoursText === "" || hoursText.length > 2 || minutesText.length !== 2) throw wrong();
  if (!/^\+?\d+$/.test(hoursText) || !/^\+?\d+$/.test(minutesText)) throw wrong();
  const hours = Number(hoursText);
  const minutes = Number(minutesText);
  if (minutes > 59 || hours > 24 || (hours === 24 && minutes !== 0)) throw wrong();
  return hours * 60 + minutes;
}

/** protocol::playlist::format_clock: `09:05`. */
export function formatClock(minutes: number): string {
  const two = (value: number) => String(value).padStart(2, "0");
  return `${two(Math.floor(minutes / 60))}:${two(minutes % 60)}`;
}

function dayIndex(name: string): number {
  const index = (DAYS as string[]).indexOf(name);
  if (index < 0) throw new Error(`${quoted(name)} is not a day: mon, tue, wed, thu, fri, sat or sun`);
  return index;
}

/** The days in their order, once each; every day is none. */
function tidyDays(indexes: number[]): Day[] {
  const unique = [...new Set(indexes)].sort((a, b) => a - b);
  return unique.length === DAYS.length ? [] : unique.map((index) => DAYS[index]!);
}

/** protocol::playlist::parse_days: `mon-fri`, `sat,sun`, `fri-mon`; empty or `all` for every day. */
export function parseDays(text: string): Day[] {
  const trimmed = text.trim();
  if (trimmed === "" || trimmed === "all") return [];
  const indexes: number[] = [];
  for (const raw of trimmed.split(",")) {
    const part = raw.trim();
    const dash = part.indexOf("-");
    if (dash < 0) {
      indexes.push(dayIndex(part));
      continue;
    }
    const first = dayIndex(part.slice(0, dash).trim());
    const last = dayIndex(part.slice(dash + 1).trim());
    let index = first;
    for (;;) {
      indexes.push(index % 7);
      if (index % 7 === last) break;
      index += 1;
    }
  }
  return tidyDays(indexes);
}

/** protocol::playlist::format_days: `mon-fri`, `sat,sun`, `every day`. */
export function formatDays(days: Day[]): string {
  if (days.length === 0) return "every day";
  const indexes = [...new Set(days.map((day) => DAYS.indexOf(day)))].sort((a, b) => a - b);
  const parts: string[] = [];
  let start = 0;
  while (start < indexes.length) {
    let end = start;
    while (end + 1 < indexes.length && indexes[end + 1] === indexes[end]! + 1) end += 1;
    const first = DAYS[indexes[start]!]!;
    const last = DAYS[indexes[end]!]!;
    if (end === start) parts.push(first);
    else if (end - start === 1) parts.push(first, last);
    else parts.push(`${first}-${last}`);
    start = end + 1;
  }
  return parts.join(",");
}

// --- client::playlist: spans of time ---

/** client::playlist::DURATION_DEFAULT_S: how long a page or an image shows when no duration is given. */
export const DURATION_DEFAULT_S = 10;

/** A whole number of `unitMs` from `number` (`1.5`, `.25`), rounded half up; null when it is no number. */
function decimalTimes(number: string, unitMs: number): number | null {
  const point = number.indexOf(".");
  const whole = point < 0 ? number : number.slice(0, point);
  const fraction = point < 0 ? "" : number.slice(point + 1);
  if (whole === "" && fraction === "") return null;
  if (fraction.includes(".") || fraction.length > 9) return null;
  if (!/^\d*$/.test(whole) || !/^\d*$/.test(fraction)) return null;
  let ms = BigInt(whole === "" ? 0 : whole) * BigInt(unitMs);
  if (fraction !== "") {
    const scale = 10n ** BigInt(fraction.length);
    ms += (BigInt(fraction) * BigInt(unitMs) + scale / 2n) / scale;
  }
  return ms <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(ms) : null;
}

function parseClockReading(text: string): number | null {
  const parts = text.split(":");
  let hoursText: string;
  let minutesText: string;
  let secondsText: string;
  if (parts.length === 2) [hoursText, minutesText, secondsText] = ["0", parts[0]!, parts[1]!];
  else if (parts.length === 3) [hoursText, minutesText, secondsText] = [parts[0]!, parts[1]!, parts[2]!];
  else return null;
  const number = (part: string) => (/^\d+$/.test(part) ? Number(part) : null);
  const hours = number(hoursText);
  const minutes = number(minutesText);
  if (hours === null || minutes === null) return null;
  if (parts.length === 3 && minutes > 59) return null;
  if (secondsText.split(".")[0]!.length !== 2) return null;
  const seconds = decimalTimes(secondsText, 1000);
  if (seconds === null || seconds >= 60_000) return null;
  const total = hours * 3_600_000 + minutes * 60_000 + seconds;
  return Number.isSafeInteger(total) ? total : null;
}

/** client::playlist::parse_ms: `90` seconds, `1.5s`, `500ms`, `1m30s`, `2h`, or a clock reading like `0:05`. */
export function parseMs(typed: string): number {
  const text = typed.trim();
  const refuse = () =>
    new Error(`${quoted(text)} is not a time: seconds, or like 500ms, 1.5s, 1m30s, or a clock reading like 0:05`);
  if (text === "") throw refuse();
  if (text.includes(":")) {
    const ms = parseClockReading(text);
    if (ms === null) throw refuse();
    return ms;
  }
  const units: Record<string, number> = { ms: 1, s: 1000, m: 60_000, h: 3_600_000, d: 86_400_000 };
  let total = 0;
  let rest = text;
  while (rest !== "") {
    const number = /^[0-9.]*/.exec(rest)![0];
    const afterNumber = rest.slice(number.length);
    const unit = /^[A-Za-z]*/.exec(afterNumber)![0];
    const after = afterNumber.slice(unit.length);
    const unitMs = unit === "" ? (after === "" ? 1000 : undefined) : units[unit];
    if (unitMs === undefined) throw refuse();
    const ms = decimalTimes(number, unitMs);
    if (ms === null) throw refuse();
    total += ms;
    if (!Number.isSafeInteger(total)) throw refuse();
    rest = after;
  }
  return total;
}

/** client::playlist::parse_seconds: `parseMs`, in whole seconds. */
export function parseSeconds(typed: string): number {
  const ms = parseMs(typed);
  if (ms % 1000 !== 0) throw new Error(`${quoted(typed.trim())} is not whole seconds`);
  if (ms / 1000 > 0xffff_ffff) throw new Error(`${quoted(typed.trim())} is too long`);
  return ms / 1000;
}

/** client::playlist::format_seconds: `10s`, `1m30s`, `0s`. */
export function formatSeconds(seconds: number): string {
  return seconds === 0 ? "0s" : formatTimeout(seconds);
}

function secondsWithFraction(ms: number): string {
  const whole = Math.floor(ms / 1000);
  const fraction = ms % 1000;
  if (fraction === 0) return String(whole);
  return `${whole}.${String(fraction).padStart(3, "0").replace(/0+$/, "")}`;
}

/** client::playlist::format_ms: `500ms`, `1.5s`, `1m2.5s`, as `parseMs` reads it back. */
export function formatMs(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  if (ms % 1000 === 0) {
    const seconds = ms / 1000;
    return seconds <= 0xffff_ffff ? formatSeconds(seconds) : `${seconds}s`;
  }
  const minutes = Math.floor(ms / 60_000);
  const seconds = secondsWithFraction(ms % 60_000);
  return minutes === 0 ? `${seconds}s` : `${formatSeconds(minutes * 60)}${seconds}s`;
}

/** client::playlist::format_position: a place in a video, `0:05`, `1:02.5`, `1:00:00`. */
export function formatPosition(ms: number): string {
  const hours = Math.floor(ms / 3_600_000);
  const minutes = Math.floor(ms / 60_000) % 60;
  let seconds = secondsWithFraction(ms % 60_000);
  if (seconds.split(".")[0]!.length < 2) seconds = `0${seconds}`;
  return hours > 0 ? `${hours}:${String(minutes).padStart(2, "0")}:${seconds}` : `${minutes}:${seconds}`;
}

/** client::playlist::short_id: enough of an id to name it. */
export function shortId(id: string): string {
  return id.length > 8 ? id.slice(0, 8) : id;
}

// --- client::playlist: an item from what was typed ---

/**
 * client::playlist::ItemFields: what a dialog has, as typed. Undefined is
 * not given and keeps what the item has; an empty text is the default.
 */
export interface ItemFields {
  kind?: string;
  src?: string;
  duration?: string;
  from?: string;
  to?: string;
  sound?: boolean;
  volume?: string;
  fit?: string;
  background?: string;
  transition?: string;
  transitionMs?: string;
  interactive?: boolean;
  idle?: string;
  readyDelay?: string;
  bridge?: boolean;
}

/** PlaylistItem::new. */
export function newItem(kind: ItemKind, src: string): Item {
  return {
    kind,
    src,
    duration_s: null,
    trim_start_ms: null,
    trim_end_ms: null,
    sound: false,
    volume: null,
    fit: "contain",
    background: null,
    transition: null,
    transition_ms: null,
    interactive: false,
    idle_s: null,
    ready_delay_ms: null,
    bridge: false,
  };
}

/** An item as the device sends it, the fields serde defaults filled in. */
export function fullItem(item: Partial<Item> & Pick<Item, "kind" | "src">): Item {
  return { ...newItem(item.kind, item.src), ...item };
}

function optional<T>(text: string, parse: (text: string) => T): T | null {
  return text.trim() === "" ? null : parse(text.trim());
}

function switchKind(item: Item, kind: ItemKind): void {
  item.kind = kind;
  if (kind === "video") {
    item.duration_s = null;
    item.interactive = false;
    item.idle_s = null;
  } else {
    item.trim_start_ms = null;
    item.trim_end_ms = null;
    item.sound = false;
    item.volume = null;
  }
  if (kind !== "url") {
    item.bridge = false;
    item.ready_delay_ms = null;
  }
}

function whole(text: string, max: number, refuse: () => Error): number {
  if (!/^\+?\d+$/.test(text)) throw refuse();
  const value = Number(text);
  if (value > max) throw refuse();
  return value;
}

/** client::playlist::apply: what was typed onto `item`. */
export function apply(item: Item, fields: ItemFields): void {
  if (fields.kind !== undefined) {
    const kind = parseKind(fields.kind.trim());
    if (kind !== item.kind) switchKind(item, kind);
  }
  if (fields.src !== undefined) item.src = fields.src.trim();
  if (fields.duration !== undefined) item.duration_s = optional(fields.duration, parseSeconds);
  if (fields.from !== undefined) item.trim_start_ms = optional(fields.from, parseMs);
  if (fields.to !== undefined) item.trim_end_ms = optional(fields.to, parseMs);
  if (fields.volume !== undefined) {
    item.volume = optional(fields.volume, (text) =>
      whole(text, 100, () => new Error(`${quoted(text)} is not a volume: 0 to 100`)),
    );
    if (item.volume !== null && fields.sound === undefined) item.sound = true;
  }
  if (fields.sound !== undefined) {
    item.sound = fields.sound;
    if (!fields.sound) item.volume = null;
  }
  if (fields.fit !== undefined) item.fit = optional(fields.fit, parseFit) ?? "contain";
  if (fields.background !== undefined) {
    item.background = optional(fields.background, (text) => {
      checkColor(text);
      return text.toLowerCase();
    });
  }
  if (fields.transition !== undefined) item.transition = optional(fields.transition, parseTransition);
  if (fields.transitionMs !== undefined) {
    item.transition_ms = optional(fields.transitionMs, (text) =>
      whole(text, 0xffff_ffff, () => new Error(`${quoted(text)} is not a transition: milliseconds`)),
    );
  }
  if (fields.idle !== undefined) {
    item.idle_s = optional(fields.idle, parseSeconds);
    if (item.idle_s !== null && fields.interactive === undefined) item.interactive = true;
  }
  if (fields.interactive !== undefined) {
    item.interactive = fields.interactive;
    if (!fields.interactive) item.idle_s = null;
  }
  if (fields.readyDelay !== undefined) {
    item.ready_delay_ms = optional(fields.readyDelay, (text) => {
      const ms = parseMs(text);
      if (ms > 0xffff_ffff) throw new Error(`${quoted(text)} is too long`);
      return ms;
    });
  }
  if (fields.bridge !== undefined) item.bridge = fields.bridge;
  if (item.kind !== "video" && item.duration_s === null) item.duration_s = DURATION_DEFAULT_S;
}

function checked(item: Item, position: number | null): Item {
  if (position !== null) return checkItem(item, position);
  try {
    return checkItem(item, 0);
  } catch (problem) {
    const message = (problem as Error).message;
    throw new Error(message.startsWith("item 0: ") ? `the new item: ${message.slice("item 0: ".length)}` : message);
  }
}

/** client::playlist::item_from_fields: a new item, refused as the device would. */
export function itemFromFields(fields: ItemFields, position: number | null): Item {
  if (fields.kind === undefined) throw new Error("an item is a url, an image or a video");
  const kind = parseKind(fields.kind.trim());
  if (fields.src === undefined || fields.src.trim() === "") throw new Error(`a ${kind} item needs its URL`);
  const item = newItem(kind, "");
  apply(item, fields);
  return checked(item, position);
}

/** client::playlist::edit_item: `item` with what was typed, refused as the device would. */
export function editItem(item: Item, fields: ItemFields, position: number): Item {
  const edited = { ...fullItem(item) };
  apply(edited, fields);
  return checked(edited, position);
}

/** client::playlist::EntryFields: a timetable entry as typed. */
export interface EntryFields {
  playlist?: string;
  /** Like `mon-fri`; empty or `all` for every day. */
  days?: string;
  from?: string;
  to?: string;
  priority?: number;
  enabled?: boolean;
}

/** protocol::playlist::check_timetable, as client::playlist::entry_spec gives it. */
export function entrySpec(
  playlist: string,
  days: string,
  from: string,
  to: string,
  priority: number,
  enabled: boolean,
): Schemas["TimetableSpec"] {
  const chosen = parseDays(days);
  const start = parseClock(from);
  const end = parseClock(to);
  if (start === 24 * 60) throw new Error("an entry cannot start at 24:00");
  const name = playlist.trim();
  if (name === "") throw new Error("a timetable entry needs a playlist");
  return { playlist: name, days: chosen, from: formatClock(start), to: formatClock(end), priority, enabled };
}

/** client::playlist::entry_change: what `fields` change. */
export function entryChange(fields: EntryFields): Schemas["TimetableEntryChange"] {
  const clock = (text: string | undefined) => (text === undefined ? null : formatClock(parseClock(text)));
  const playlist = fields.playlist === undefined ? null : fields.playlist.trim();
  if (playlist === "") throw new Error("a timetable entry needs a playlist");
  const change = {
    playlist,
    days: fields.days === undefined ? null : parseDays(fields.days),
    from: clock(fields.from),
    to: clock(fields.to),
    priority: fields.priority ?? null,
    enabled: fields.enabled ?? null,
  };
  if (change.from === "24:00") throw new Error("an entry cannot start at 24:00");
  if (Object.values(change).every((value) => value === null)) {
    throw new Error("nothing to change: give --playlist, --days, --from, --to, --priority, --enable or --disable");
  }
  return change;
}

// --- describe::playlist ---

/** How much of a source a line shows. */
const SRC_SHOWN = 48;

/** A source short enough for a line: the device's own without its origin, the middle of a long one left out. */
export function shorten(src: string): string {
  let bare: string;
  if (isLocal(src)) {
    const rest = src.startsWith("http://127.0.0.1")
      ? src.slice("http://127.0.0.1".length)
      : src.slice("http://localhost".length);
    bare = rest === "" ? "/" : rest;
  } else if (src.startsWith("https://")) {
    bare = src.slice("https://".length);
  } else if (src.startsWith("http://")) {
    bare = src.slice("http://".length);
  } else {
    bare = src;
  }
  const chars = Array.from(bare);
  if (chars.length <= SRC_SHOWN) return bare;
  const head = Math.floor((SRC_SHOWN - 3) / 2);
  const tail = SRC_SHOWN - 3 - head;
  return `${chars.slice(0, head).join("")}...${chars.slice(chars.length - tail).join("")}`;
}

/** How long an item shows: its duration, or what part of a video plays. */
export function timing(item: Item): string {
  if (item.kind === "url" || item.kind === "image") {
    return item.duration_s != null ? formatSeconds(item.duration_s) : "-";
  }
  const start = item.trim_start_ms ?? null;
  const end = item.trim_end_ms ?? null;
  if (start === null && end === null) return "whole";
  if (end === null) return `${formatPosition(start ?? 0)}-end`;
  return `${formatPosition(start ?? 0)}-${formatPosition(end)}`;
}

/** A transition and how long it takes: `cut`, `fade 800 ms`. */
export function transition(kind: Transition, ms: number): string {
  return kind === "cut" ? "cut" : `${kind} ${ms} ms`;
}

/** What an item does beyond the defaults, a word or two each. */
export function options(item: Item): string[] {
  const words: string[] = [];
  const fit = item.fit ?? "contain";
  if (item.kind === "video") {
    const volume = item.volume ?? null;
    words.push(item.sound ? (volume !== null ? `sound ${volume}%` : "sound") : "muted");
  }
  if (item.kind !== "url" && fit !== "contain") words.push(fit);
  if (item.background != null) words.push(`on ${item.background}`);
  const kind = item.transition ?? null;
  const ms = item.transition_ms ?? null;
  if (kind === "cut") words.push("cut in");
  else if (kind !== null && ms !== null) words.push(`${kind} in ${ms} ms`);
  else if (kind !== null) words.push(`${kind} in`);
  else if (ms !== null) words.push(`in ${ms} ms`);
  if (item.interactive) {
    words.push(item.idle_s != null ? `interactive, idle ${formatSeconds(item.idle_s)}` : "interactive");
  }
  if (item.ready_delay_ms != null) words.push(`ready ${formatMs(item.ready_delay_ms)} after load`);
  if (item.bridge) words.push("bridge");
  return words;
}

/** One item, its place first. */
export function item(item: Item, position: number): Line {
  const words = options(item);
  const line = new Line()
    .pad("muted", String(position).padStart(3), 3)
    .text(" ")
    .pad("label", item.kind, 5)
    .text(" ")
    .pad("plain", shorten(item.src), 40)
    .text(" ");
  if (words.length === 0) return line.add("plain", timing(item));
  return line.pad("plain", timing(item), 11).text(" ").add("muted", words.join(", "));
}

/** A playlist's items, or how to add one. */
export function items(info: Schemas["PlaylistInfo"]): Line[] {
  if (info.items.length === 0) {
    return [
      Line.of("warn", "no items; add one with")
        .text(" ")
        .add("cmd", `tessaro-ctl playlist items add ${info.name} --url URL`),
    ];
  }
  return info.items.map((one, at) => item(one, at + 1));
}

/** Every playlist, the one on screen marked. */
export function list(playlists: Schemas["PlaylistInfo"][]): Line[] {
  if (playlists.length === 0) {
    return [Line.of("muted", "no playlists; add one with").text(" ").add("cmd", "tessaro-ctl playlist create NAME")];
  }
  const lines: Line[] = [];
  for (const info of playlists) {
    const marker = info.playing ? Line.of("ok", "*") : Line.plain(" ");
    const count = info.items.length === 1 ? "1 item" : `${info.items.length} items`;
    const used: string[] = [];
    if (info.default) used.push(PLAYLIST_DEFAULT);
    const entries = (info.timetable ?? []).length;
    if (entries === 1) used.push("1 timetable entry");
    else if (entries > 1) used.push(`${entries} timetable entries`);
    const line = marker.text(" ").pad("heading", info.name, 20).text(" ");
    lines.push(
      used.length === 0
        ? line.add("plain", count)
        : line.pad("plain", count, 10).text(" ").add("muted", used.join(", ")),
    );
  }
  lines.push(new Line());
  lines.push(
    Line.of("muted", "* on screen now; its items with").text(" ").add("cmd", "tessaro-ctl playlist show NAME"),
  );
  return lines;
}

/** One playlist's facts; its items are `items`. */
export function show(info: Schemas["PlaylistInfo"]): Fact[] {
  const timetable = info.timetable ?? [];
  return [
    fact("name", Line.of("heading", info.name)),
    fact("id", Line.of("muted", info.id)),
    fact("transition", transition(info.transition, info.transition_ms)),
    fact(
      "default",
      info.default ? Line.of("ok", "yes").text(" ").add("muted", `(${PLAYLIST_DEFAULT})`) : Line.of("muted", "no"),
    ),
    fact(
      "timetable",
      timetable.length === 0
        ? Line.of("muted", "not in it")
        : Line.plain(`entries ${timetable.map((id) => shortId(id)).join(", ")}`),
    ),
    fact("playing", info.playing ? Line.of("ok", "yes") : Line.of("muted", "no")),
  ];
}

/** One timetable entry, the active one marked, one switched off muted. */
export function entry(entry: Schemas["TimetableInfo"]): Line {
  const on = (tone: Tone): Tone => (entry.enabled ? tone : "muted");
  const marker = entry.active ? Line.of("ok", "*") : Line.plain(" ");
  const line = marker
    .text(" ")
    .pad("muted", shortId(entry.id), 8)
    .text(" ")
    .pad(on("plain"), formatDays(entry.days ?? []), 13)
    .text(" ")
    .add(on("plain"), `${entry.from}-${entry.to}`)
    .text(" ")
    .add("muted", "->")
    .text(" ")
    .add(on("heading"), entry.playlist_name)
    .add(on("plain"), `, priority ${entry.priority}`);
  return entry.enabled ? line : line.text(" ").add("muted", "(off)");
}

/** The timetable, or how to add an entry. */
export function timetable(entries: Schemas["TimetableInfo"][]): Line[] {
  if (entries.length === 0) {
    return [
      Line.of("muted", "no timetable entries; add one with")
        .text(" ")
        .add("cmd", "tessaro-ctl playlist timetable add PLAYLIST --from HH:MM --to HH:MM"),
    ];
  }
  return [
    ...entries.map(entry),
    new Line(),
    Line.of(
      "muted",
      "* decides what plays now; where entries overlap the highest priority wins, then the one listed first",
    ),
  ];
}

function reason(status: Schemas["PlaylistStatus"]): Line {
  switch (status.reason) {
    case "timetable":
      return Line.of("muted", status.entry != null ? `(timetable entry ${shortId(status.entry)})` : "(timetable)");
    case "default":
      return Line.of("muted", `(${PLAYLIST_DEFAULT})`);
    case "url":
      return Line.of("muted", `(no timetable entry covers now and ${PLAYLIST_DEFAULT} is empty)`);
  }
}

function playing(status: Schemas["PlaylistStatus"]): Line {
  return status.playlist != null ? Line.of("heading", status.playlist) : Line.plain("browser.url");
}

function cache(status: Schemas["PlaylistStatus"]): Line {
  const { ready, pending, failed } = status.cache ?? { ready: 0, pending: 0, failed: 0 };
  if (ready + pending + failed === 0) return Line.of("muted", "nothing to keep a copy of");
  let line = Line.of("ok", `${ready} ready`);
  if (pending > 0) line = line.text(", ").add("warn", `${pending} pending`);
  if (failed > 0) line = line.text(", ").add("bad", `${failed} failed`);
  return line;
}

/** What the player is doing, as `tessaro-ctl playlist status` says it. */
export function status(status: Schemas["PlaylistStatus"], now: number): Fact[] {
  if (!status.player) {
    return [
      fact(
        "player",
        Line.of("muted", "off")
          .text(" ")
          .add("muted", "- browser.url shows; set")
          .text(" ")
          .add("cmd", `tessaro-ctl config set ${PLAYLIST_DEFAULT}=NAME`)
          .text(" ")
          .add("muted", "or add a timetable entry"),
      ),
    ];
  }
  const facts = [fact("player", Line.of("ok", "on"))];
  if (status.nothing_playable) {
    facts.push(fact("showing", Line.of("bad", "nothing playable").text(" ").add("muted", "- the offline page is up")));
  }
  facts.push(fact("playlist", playing(status).text(" ").join(reason(status))));
  const shown = status.item;
  if (shown) {
    const when = relative(shown.since.unix, now);
    facts.push(
      fact(
        "item",
        Line.of("muted", String(shown.position))
          .text(" ")
          .add("label", shown.kind)
          .text(" ")
          .add("plain", shorten(shown.src))
          .text(" ")
          .add("muted", when === "now" ? "(came on just now)" : `(came on ${when})`),
      ),
    );
  } else {
    facts.push(fact("item", Line.of("muted", "(the player has not said yet)")));
  }
  (status.skipped ?? []).forEach((skipped, at) => {
    facts.push(
      fact(
        at === 0 ? "skipped" : "",
        Line.of("warn", `${skipped.position} ${shorten(skipped.src)}`)
          .text(": ")
          .add("plain", skipped.reason)
          .text(" ")
          .add("muted", `(${relative(skipped.at.unix, now)})`),
      ),
    );
  });
  facts.push(fact("media", cache(status)));
  return facts;
}

/** The player in a line, for the device's status. */
export function summary(status: Schemas["PlaylistStatus"]): Line {
  if (!status.player) return Line.of("muted", "off - browser.url shows directly");
  if (status.nothing_playable) {
    return Line.of("bad", "nothing playable").text(" ").add("muted", "- the offline page is up");
  }
  let line = playing(status).text(" ").join(reason(status));
  if (status.item) line = line.text(`, item ${status.item.position} ${status.item.kind}`);
  const failed = status.cache?.failed ?? 0;
  if (failed === 0) return line;
  return line.text(", ").add("warn", failed === 1 ? "1 media copy failed" : `${failed} media copies failed`);
}
