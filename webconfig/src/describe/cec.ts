// agent/protocol/src/cec.rs: the CEC events a script runs on, as a form
// reads them. The cec_triggers fixture keeps the lists and the messages
// equal to the Rust's.

/** What the agent reports from the bus (`EVENTS`). */
export const EVENTS = ["tv-on", "tv-standby", "source-gained", "source-lost", "key"];

/** The names of the TV remote's keys (`KEYS`). */
export const KEYS = [
  "select",
  "up",
  "down",
  "left",
  "right",
  "root-menu",
  "setup-menu",
  "contents-menu",
  "favorite-menu",
  "exit",
  "0",
  "1",
  "2",
  "3",
  "4",
  "5",
  "6",
  "7",
  "8",
  "9",
  "dot",
  "enter",
  "clear",
  "channel-up",
  "channel-down",
  "previous-channel",
  "info",
  "help",
  "page-up",
  "page-down",
  "power",
  "volume-up",
  "volume-down",
  "mute",
  "play",
  "stop",
  "pause",
  "record",
  "rewind",
  "fast-forward",
  "eject",
  "forward",
  "backward",
  "guide",
  "blue",
  "red",
  "green",
  "yellow",
  "data",
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
