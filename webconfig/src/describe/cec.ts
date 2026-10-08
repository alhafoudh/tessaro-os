// agent/protocol/src/cec.rs, agent/client/src/cec.rs and
// agent/client/src/describe/cec.rs: the CEC events a script runs on, the
// remote's keys, addresses and bytes as typed, and what an action and the
// message log say in words. The cec_triggers, cec_acted and cec_messages
// fixtures keep the lists, the messages and the lines equal to the Rust's.

import type { Schemas } from "../api/client";
import { Line, type Tone } from "../text/line";
import { power } from "./screen";

/** What the agent reports from the bus (`EVENTS`). */
export const EVENTS = ["tv-on", "tv-standby", "source-gained", "source-lost", "key"];

/** The TV remote's keys by their UI command (`KEYS`, codes and names). */
const REMOTE: [number, string][] = [
  [0x00, "select"],
  [0x01, "up"],
  [0x02, "down"],
  [0x03, "left"],
  [0x04, "right"],
  [0x09, "root-menu"],
  [0x0a, "setup-menu"],
  [0x0b, "contents-menu"],
  [0x0c, "favorite-menu"],
  [0x0d, "exit"],
  [0x20, "0"],
  [0x21, "1"],
  [0x22, "2"],
  [0x23, "3"],
  [0x24, "4"],
  [0x25, "5"],
  [0x26, "6"],
  [0x27, "7"],
  [0x28, "8"],
  [0x29, "9"],
  [0x2a, "dot"],
  [0x2b, "enter"],
  [0x2c, "clear"],
  [0x30, "channel-up"],
  [0x31, "channel-down"],
  [0x32, "previous-channel"],
  [0x35, "info"],
  [0x36, "help"],
  [0x37, "page-up"],
  [0x38, "page-down"],
  [0x40, "power"],
  [0x41, "volume-up"],
  [0x42, "volume-down"],
  [0x43, "mute"],
  [0x44, "play"],
  [0x45, "stop"],
  [0x46, "pause"],
  [0x47, "record"],
  [0x48, "rewind"],
  [0x49, "fast-forward"],
  [0x4a, "eject"],
  [0x4b, "forward"],
  [0x4c, "backward"],
  [0x53, "guide"],
  [0x71, "blue"],
  [0x72, "red"],
  [0x73, "green"],
  [0x74, "yellow"],
  [0x76, "data"],
];

/** The names of the TV remote's keys. */
export const KEYS = REMOTE.map(([, name]) => name);

/** The TV's logical address, always. */
export const TV = 0;
/** The audio system's: a sound bar or a receiver. */
export const AUDIO = 5;
/** The destination of a broadcast, and the sender without an address. */
export const BROADCAST = 15;
/** The most a message carries after its header: an opcode and operands. */
export const DATA_MAX = 15;

/** Every opcode of CEC 1.4 and 2.0, by the name `screen cec messages` gives it. */
const OPCODES: [number, string][] = [
  [0x00, "feature-abort"],
  [0x04, "image-view-on"],
  [0x05, "tuner-step-increment"],
  [0x06, "tuner-step-decrement"],
  [0x07, "tuner-device-status"],
  [0x08, "give-tuner-device-status"],
  [0x09, "record-on"],
  [0x0a, "record-status"],
  [0x0b, "record-off"],
  [0x0d, "text-view-on"],
  [0x0f, "record-tv-screen"],
  [0x1a, "give-deck-status"],
  [0x1b, "deck-status"],
  [0x32, "set-menu-language"],
  [0x33, "clear-analogue-timer"],
  [0x34, "set-analogue-timer"],
  [0x35, "timer-status"],
  [0x36, "standby"],
  [0x41, "play"],
  [0x42, "deck-control"],
  [0x43, "timer-cleared-status"],
  [0x44, "user-control-pressed"],
  [0x45, "user-control-released"],
  [0x46, "give-osd-name"],
  [0x47, "set-osd-name"],
  [0x64, "set-osd-string"],
  [0x67, "set-timer-program-title"],
  [0x70, "system-audio-mode-request"],
  [0x71, "give-audio-status"],
  [0x72, "set-system-audio-mode"],
  [0x7a, "report-audio-status"],
  [0x7d, "give-system-audio-mode-status"],
  [0x7e, "system-audio-mode-status"],
  [0x80, "routing-change"],
  [0x81, "routing-information"],
  [0x82, "active-source"],
  [0x83, "give-physical-address"],
  [0x84, "report-physical-address"],
  [0x85, "request-active-source"],
  [0x86, "set-stream-path"],
  [0x87, "device-vendor-id"],
  [0x89, "vendor-command"],
  [0x8a, "vendor-remote-button-down"],
  [0x8b, "vendor-remote-button-up"],
  [0x8c, "give-device-vendor-id"],
  [0x8d, "menu-request"],
  [0x8e, "menu-status"],
  [0x8f, "give-device-power-status"],
  [0x90, "report-power-status"],
  [0x91, "get-menu-language"],
  [0x92, "select-analogue-service"],
  [0x93, "select-digital-service"],
  [0x97, "set-digital-timer"],
  [0x99, "clear-digital-timer"],
  [0x9a, "set-audio-rate"],
  [0x9d, "inactive-source"],
  [0x9e, "cec-version"],
  [0x9f, "get-cec-version"],
  [0xa0, "vendor-command-with-id"],
  [0xa1, "clear-external-timer"],
  [0xa2, "set-external-timer"],
  [0xa5, "give-features"],
  [0xa6, "report-features"],
  [0xa7, "request-current-latency"],
  [0xa8, "report-current-latency"],
  [0xc0, "initiate-arc"],
  [0xc1, "report-arc-initiated"],
  [0xc2, "report-arc-terminated"],
  [0xc3, "request-arc-initiation"],
  [0xc4, "request-arc-termination"],
  [0xc5, "terminate-arc"],
  [0xf8, "cdc-message"],
  [0xff, "abort"],
];

/**
 * One entry of what a script runs on, as typed: an event, `key` for every
 * key, or `key:<name>` for one. Stored lower-case.
 */
export function trigger(typed: string): string {
  const lower = typed.trim().toLowerCase();
  if (lower.startsWith("key:")) {
    const name = lower.slice("key:".length);
    if (KEYS.includes(name)) return lower;
    throw new Error(`${JSON.stringify(name)} is not a remote key; one of ${KEYS.join(", ")}`);
  }
  if (EVENTS.includes(lower)) return lower;
  throw new Error(`${JSON.stringify(typed.trim())} is not a CEC event; one of ${EVENTS.join(", ")}, or key:<name>`);
}

/** Every entry of a comma-separated list, checked, without repeats. */
export function triggers(typed: string): string[] {
  const out: string[] = [];
  for (const entry of typed.split(",")) {
    if (entry.trim() === "") continue;
    const one = trigger(entry);
    if (!out.includes(one)) out.push(one);
  }
  return out;
}

function byte(value: number): string {
  return value.toString(16).padStart(2, "0");
}

/** The name events give a UI command: the table's, else `0xNN`. */
export function keyName(code: number): string {
  return REMOTE.find(([known]) => known === code)?.[1] ?? `0x${byte(code)}`;
}

/** A remote key by its name, as `screen cec key` takes it. */
export function keyCode(name: string): number {
  const lower = name.trim().toLowerCase();
  const found = REMOTE.find(([, known]) => known === lower);
  if (!found) throw new Error(`${JSON.stringify(lower)} is not a remote key; one of ${KEYS.join(", ")}`);
  return found[0];
}

/** What a logical address makes a device. */
export function kind(address: number): string {
  switch (address) {
    case 0:
      return "tv";
    case 1:
    case 2:
    case 9:
      return "recorder";
    case 3:
    case 6:
    case 7:
    case 10:
      return "tuner";
    case 4:
    case 8:
    case 11:
      return "playback";
    case 5:
      return "audio";
    case 12:
    case 13:
      return "backup";
    case 14:
      return "specific";
    default:
      return "unregistered";
  }
}

/** An opcode's name, null for one the standard does not have. */
export function opcodeName(opcode: number): string | null {
  return OPCODES.find(([known]) => known === opcode)?.[1] ?? null;
}

/** Bytes as `screen cec` shows and takes them: `44 41`. */
export function hex(data: number[]): string {
  return data.map(byte).join(" ");
}

/** Rust's `trim_start_matches`: every leading copy of `prefix` off. */
function trimStart(text: string, prefix: string): string {
  let out = text;
  while (out.startsWith(prefix)) out = out.slice(prefix.length);
  return out;
}

/** The bytes typed as hex: `44 41`, `44:41`, `0x44,0x41`, `4441`. At most `DATA_MAX`, never none. */
export function parseData(typed: string): number[] {
  const digits = typed
    .split(/[\s:,]/)
    .map((part) => trimStart(trimStart(part, "0x"), "0X"))
    .map((part) => (part.length === 1 ? `0${part}` : part))
    .join("");
  if (digits === "") throw new Error("no bytes to send; an opcode in hex, e.g. 8f");
  if (digits.length % 2 !== 0 || !/^[0-9a-fA-F]+$/.test(digits)) {
    throw new Error(`${JSON.stringify(typed.trim())} is not hex bytes, e.g. 44 41`);
  }
  const data: number[] = [];
  for (let at = 0; at < digits.length; at += 2) data.push(parseInt(digits.slice(at, at + 2), 16));
  if (data.length > DATA_MAX) {
    throw new Error(`a message carries at most ${DATA_MAX} bytes after its header, this has ${data.length}`);
  }
  return data;
}

/** A logical address as typed: `tv`, `audio`, `all`, or 0 to 15. */
export function parseAddress(typed: string): number {
  const lower = typed.trim().toLowerCase();
  if (lower === "tv") return TV;
  if (lower === "audio") return AUDIO;
  if (lower === "all" || lower === "broadcast") return BROADCAST;
  if (/^\+?[0-9]+$/.test(lower)) {
    const address = Number(lower);
    if (address <= 15) return address;
  }
  throw new Error(`${JSON.stringify(typed.trim())} is not a CEC address: tv, audio, all, or 0 to 15`);
}

/** What was typed for an address, or `fallback` for nothing. */
function typedAddress(typed: string, fallback: number | null): number | null {
  return typed.trim() === "" ? fallback : parseAddress(typed);
}

/** What the connector was typed as; nothing for every adapter. */
function connector(typed: string): string | null {
  const trimmed = typed.trim();
  return trimmed === "" ? null : trimmed;
}

/** `screen cec key NAME [--to ADDRESS]`: the key checked by name, to the TV unless an address was typed. */
export function keyBody(key: string, to: string, on: string): Schemas["CecKeyBody"] {
  const name = keyName(keyCode(key));
  return { key: name, to: typedAddress(to, null), connector: connector(on) };
}

/** `screen cec send HEX --to ADDRESS [--reply OPCODE]`. */
export function sendBody(data: string, to: string, reply: string, on: string): Schemas["CecSendBody"] {
  const bytes = parseData(data);
  const address = typedAddress(to, null);
  if (address === null) throw new Error("a message needs an address to go to: tv, audio, all, or 0 to 15");
  let opcode: number | null = null;
  if (reply.trim() !== "") {
    const typed = parseData(reply);
    if (typed.length !== 1) throw new Error("a reply is one opcode in hex, e.g. 90");
    opcode = typed[0]!;
  }
  return { to: address, data: hex(bytes), reply: opcode, connector: connector(on) };
}

/** The bare connector body, for `wake`, `standby`, `source` and `scan`. */
export function on(typed: string): Schemas["CecBody"] {
  return { connector: connector(typed) };
}

/**
 * A logical address in words: `TV`, `playback 4`, `everyone` as a
 * destination, `unregistered` as a sender.
 */
export function address(address: number, to: boolean): string {
  if (address === TV) return "TV";
  if (address === BROADCAST) return to ? "everyone" : "unregistered";
  return `${kind(address)} ${address}`;
}

/** The opcode and its operands in words: `standby (36)`, `user-control-pressed (44) 41 volume-up`, `poll`. */
export function data(data: string): string {
  let bytes: number[];
  try {
    bytes = parseData(data);
  } catch {
    bytes = [];
  }
  const [opcode, ...operands] = bytes;
  if (opcode === undefined) return "poll";
  const name = opcodeName(opcode);
  let words = name !== null ? `${name} (${byte(opcode)})` : `opcode ${byte(opcode)}`;
  if (operands.length > 0) words += ` ${hex(operands)}`;
  if (opcode === 0x44 && operands.length > 0) words += ` ${keyName(operands[0]!)}`;
  return words;
}

/** Whether a sent message was taken: a broadcast has nobody to say so. */
function taken(to: number, acked: boolean): Line {
  if (to === BROADCAST) return Line.of("muted", "sent");
  return acked ? Line.of("ok", "acknowledged") : Line.of("warn", "not acknowledged");
}

/** One message of the log, in columns: when, which way, who to whom, what. */
export function message(message: Schemas["CecMessage"]): Line {
  const [way, tone]: [string, Tone] = message.direction === "in" ? ["in", "plain"] : ["out", "label"];
  const route = `${address(message.from, false)} -> ${address(message.to, true)}`;
  let line = new Line()
    .pad("muted", message.time, 13)
    .pad(tone, way, 4)
    .pad("plain", route, 26)
    .text(data(message.data));
  if (message.acked != null) line = line.text(", ").join(taken(message.to, message.acked));
  return line;
}

/** A page of the log, oldest first. */
export function messages(page: Schemas["CecMessages"]): Line[] {
  if (page.messages.length === 0) return [Line.of("muted", "no messages yet")];
  return page.messages.map(message);
}

function sent(one: Schemas["CecSent"]): Line {
  return Line.plain(`    ${data(one.data)} to ${address(one.to, true)}, `).join(taken(one.to, one.acked));
}

function adapter(action: string, acted: Schemas["CecAdapterActed"]): Line[] {
  const where = acted.connector ?? acted.device;
  const lines = [Line.of("heading", `${action} on ${where}`)];
  if (acted.error != null) lines.push(Line.plain("    ").add("bad", acted.error));
  lines.push(...(acted.sent ?? []).map(sent));
  const answered = acted.answered ?? [];
  if (action === "scan" && acted.error == null) {
    lines.push(
      answered.length === 0
        ? Line.plain("    ").add("warn", "nobody answered")
        : Line.plain(`    answered: ${answered.map((one) => address(one, true)).join(", ")}`),
    );
  }
  if (acted.reply != null) {
    lines.push(Line.plain("    reply: ").text(`${data(acted.reply.data)} from ${address(acted.reply.from, false)}`));
  }
  if (acted.tv != null) lines.push(Line.plain("    the TV is ").join(power(acted.tv)));
  return lines;
}

/** What an action did on every adapter it went out on. */
export function acted(acted: Schemas["CecActed"]): Line[] {
  if (acted.adapters.length === 0) return [Line.of("warn", `${acted.action}: no adapter answered in time`)];
  return acted.adapters.flatMap((one) => adapter(acted.action, one));
}
