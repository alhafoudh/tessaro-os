// agent/client/src/schedule.rs: the words for schedules and the times and
// runs they share with scripts, and reading a timeout the way every client
// accepts it. What depends on the time takes `now`, seconds since the epoch.

import type { Schemas } from "../api/client";
import { fact, Line, type Fact } from "../text/line";

/** A timeout as `parseTimeout` reads it back: `none`, `90s`, `1h30m`. */
export function formatTimeout(seconds: number): string {
  if (seconds === 0) return "none";
  const parts: [number, string][] = [
    [Math.floor(seconds / 86_400), "d"],
    [Math.floor(seconds / 3600) % 24, "h"],
    [Math.floor(seconds / 60) % 60, "m"],
    [seconds % 60, "s"],
  ];
  return parts
    .filter(([count]) => count > 0)
    .map(([count, unit]) => `${count}${unit}`)
    .join("");
}

/** `2026-09-28 07:00:00 CEST (in 1 day 15h)`. */
export function moment(moment: Schemas["Moment"], now: number): Line {
  const local = moment.local === "" ? String(moment.unix) : moment.local;
  return Line.plain(`${local} `).add("muted", `(${relative(moment.unix, now)})`);
}

/** The next times a calendar fires, a fact each, the first under `label`. */
export function upcoming(check: Schemas["CalendarCheck"], label: string, now: number): Fact[] {
  if (check.next.length === 0) {
    return [fact(label, Line.of("warn", "never again"))];
  }
  return check.next.map((next, at) => fact(at === 0 ? label : "", moment(next, now)));
}

export function succeeded(run: Schemas["ScriptRun"]): boolean {
  return run.result === "success";
}

/** How a run ended: `success`, or systemd's result with its exit status. */
export function outcome(run: Schemas["ScriptRun"]): string {
  if (succeeded(run) || run.status === "" || run.status === "0") return run.result;
  return `${run.result} ${run.status}`;
}

/** How the last run the schedule started went and when, and how many it has running now. */
export function lastRun(info: Schemas["ScheduleInfo"], now: number): Line {
  return runs(info.last_run ?? null, info.running ?? 0, now);
}

/** How run `last` went and when, and `running` beside it when there are. */
export function runs(last: Schemas["ScriptRun"] | null, running: number, now: number): Line {
  const line = last
    ? Line.of(succeeded(last) ? "ok" : "bad", `${outcome(last)}, ${relative(last.finished.unix, now)}`)
    : Line.of("muted", "never");
  return running > 0 ? line.add("warn", ` (${running} running)`) : line;
}

/** A timeout as typed: `90`, `90s`, `10m`, `1h30m`; `none` or `0` for none. */
export function parseTimeout(typed: string): number {
  const text = typed.trim();
  if (text === "none") return 0;
  const refuse = () => new Error(`${JSON.stringify(text)} is not a timeout: seconds, or like 90s, 10m, 2h, 1h30m`);
  if (text === "") throw refuse();
  const units: Record<string, number> = { s: 1, m: 60, h: 3600, d: 86_400 };
  let total = 0;
  let digits = "";
  for (const ch of text) {
    if (ch >= "0" && ch <= "9") {
      digits += ch;
      continue;
    }
    const unit = units[ch];
    if (unit === undefined || digits === "") throw refuse();
    total += Number(digits) * unit;
    digits = "";
  }
  if (digits !== "") total += Number(digits);
  if (!Number.isSafeInteger(total)) throw refuse();
  return total;
}

/** Seconds as `45s`, `10min`, `1h 30min`, `2d 3h`. */
export function duration(seconds: number): string {
  const days = Math.floor(seconds / 86_400);
  const hours = Math.floor(seconds / 3600) % 24;
  const minutes = Math.floor(seconds / 60) % 60;
  const rest = seconds % 60;
  if (days === 0 && hours === 0 && minutes === 0) return `${rest}s`;
  if (days === 0 && hours === 0) return rest === 0 ? `${minutes}min` : `${minutes}min ${rest}s`;
  if (days === 0) return minutes === 0 ? `${hours}h` : `${hours}h ${minutes}min`;
  return hours === 0 ? `${days}d` : `${days}d ${hours}h`;
}

/** `unix` from `now`: `in 1h 20min`, `3min ago`, `now`. */
export function relative(unix: number, now: number): string {
  const difference = unix - now;
  // Minutes are enough past an hour; seconds would only jitter.
  const rounded = (seconds: number) => (seconds >= 3600 ? duration(Math.floor(seconds / 60) * 60) : duration(seconds));
  if (difference === 0) return "now";
  if (difference > 0) return `in ${rounded(difference)}`;
  return `${rounded(-difference)} ago`;
}

/** Now, by this computer's clock. */
export function now(): number {
  return Math.floor(Date.now() / 1000);
}
