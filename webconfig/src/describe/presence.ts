// agent/protocol/src/presence.rs: the presence events a script runs on, as
// a form reads them. The presence_triggers fixture keeps the list and the
// messages equal to the Rust's.

/** What the agent reports when the people in front of the screen change (`EVENTS`). */
export const EVENTS = ["arrived", "left", "near", "far", "classified"];

/** The face detectors camera.presence.model picks from (`MODELS`). */
export const MODELS = ["face-full", "face-short"];

/** One entry of what a script runs on, as typed. Stored lower-case. */
export function trigger(typed: string): string {
  const lower = typed.trim().toLowerCase();
  if (EVENTS.includes(lower)) return lower;
  throw new Error(`${JSON.stringify(typed.trim())} is not a presence event; one of ${EVENTS.join(", ")}`);
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
