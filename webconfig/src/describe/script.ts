// agent/client/src/script.rs: a script built from what was typed, and the
// words for scripts and their runs. What depends on the time takes `now`,
// seconds since the epoch.

import type { Schemas } from "../api/client";
import { Line } from "../text/line";
import { duration, formatTimeout, outcome, parseTimeout, relative, runs, succeeded } from "./schedule";

/** The most output lines a run answers with (`SCRIPT_OUTPUT_MAX`). */
export const OUTPUT_MAX = 1000;

/** One step of a run. */
export type ScriptEvent = Schemas["ScriptEvent"];

/** A script's fields as a form has them, each as typed. */
export interface Typed {
  name: string;
  description: string;
  body: string;
  onError: string;
  timeout: string;
  concurrency: string;
  bridge: boolean;
}

/** What a form shows for `spec`: saving it unchanged changes nothing. */
export function typedOf(spec: Schemas["ScriptSpec"]): Typed {
  return {
    name: spec.name,
    description: spec.description ?? "",
    body: spec.body,
    onError: spec.on_error ?? "stop",
    timeout: formatTimeout(spec.timeout_s ?? 0),
    concurrency: spec.concurrency ?? "overlap",
    bridge: spec.bridge ?? false,
  };
}

function onError(typed: Typed): Schemas["OnError"] {
  const name = typed.onError.trim();
  if (name === "") return "stop";
  if (name === "stop" || name === "continue") return name;
  throw new Error(`${JSON.stringify(name)} is not stop or continue`);
}

function concurrency(typed: Typed): Schemas["Concurrency"] {
  const name = typed.concurrency.trim();
  if (name === "") return "overlap";
  if (name === "overlap" || name === "skip") return name;
  throw new Error(`${JSON.stringify(name)} is not overlap or skip`);
}

function timeoutS(typed: Typed): number {
  const text = typed.timeout.trim();
  return text === "" ? 0 : parseTimeout(text);
}

/** The script to create. */
export function specOf(typed: Typed): Schemas["ScriptSpec"] {
  const timeout = timeoutS(typed);
  return {
    name: typed.name.trim(),
    description: typed.description.trim(),
    body: typed.body,
    on_error: onError(typed),
    timeout_s: timeout > 0 ? timeout : null,
    concurrency: concurrency(typed),
    bridge: typed.bridge,
  };
}

/** Every field of an existing script replaced with what was typed. */
export function changeOf(typed: Typed): Schemas["ScriptChange"] {
  return {
    name: typed.name.trim(),
    description: typed.description.trim(),
    body: typed.body,
    on_error: onError(typed),
    timeout_s: timeoutS(typed),
    concurrency: concurrency(typed),
    bridge: typed.bridge,
  };
}

/** How the script's last run went and when, and how many run now. */
export function lastRun(info: Schemas["ScriptInfo"], now: number): Line {
  return runs(info.runs?.[0] ?? null, info.running ?? 0, now);
}

/** Who started a run: `by hand`, `from the page`, `by schedule night`. */
export function startedBy(run: Schemas["ScriptRun"]): string {
  const schedule = run.schedule ?? null;
  if (run.trigger === "manual") return "by hand";
  if (run.trigger === "bridge") return "from the page";
  if (run.trigger === "schedule") return schedule !== null ? `by schedule ${schedule}` : "by a removed schedule";
  return `by ${run.trigger}`;
}

function took(run: Schemas["ScriptRun"]): number {
  return Math.max(0, run.finished.unix - run.started.unix);
}

/** One finished run in a list: `exit-code 1, 3min ago, by hand (2s)`. */
export function runLine(run: Schemas["ScriptRun"], now: number): Line {
  return Line.of(succeeded(run) ? "ok" : "bad", outcome(run))
    .add("plain", `, ${relative(run.finished.unix, now)}, ${startedBy(run)}`)
    .add("muted", ` (${duration(took(run))})`);
}

/** How a followed run ended: `run manual-... succeeded after 2s`. */
export function ended(run: Schemas["ScriptRun"]): Line {
  const after = duration(took(run));
  return succeeded(run)
    ? Line.of("ok", `run ${run.run} succeeded after ${after}`)
    : Line.of("bad", `run ${run.run} failed: ${outcome(run)}, after ${after}`);
}

/**
 * What a followed run's step says: an output line as it is, the start and
 * the end around it. `name` is the script's, for where the rest of a cut
 * output is.
 */
export function eventLine(event: ScriptEvent, name: string): Line {
  switch (event.event) {
    case "started":
      return Line.of("muted", `run ${event.run} started`);
    case "line":
      return Line.plain(event.text);
    case "cut":
      return Line.of("warn", `output cut at ${OUTPUT_MAX} lines; \`tessaro-ctl script logs ${name}\` has the rest`);
    case "ended":
      return ended(event.record);
  }
}

/** How a script's runs behave, in a few words: `stop on error, overlap`. */
export function behaviour(spec: Schemas["ScriptSpec"]): string {
  const words = [
    (spec.on_error ?? "stop") === "stop" ? "stop on error" : "continue on error",
    (spec.concurrency ?? "overlap") === "overlap" ? "overlap" : "skip if running",
  ];
  if (spec.timeout_s !== null && spec.timeout_s !== undefined) words.push(`${duration(spec.timeout_s)} timeout`);
  if (spec.bridge) words.push("page may run it");
  return words.join(", ");
}
