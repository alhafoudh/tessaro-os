// The ports by the Rust function's name, rendered as the golden fixtures
// record them (agent/client/tests/describe.rs `render`). Keep both lists
// the same.

/* eslint-disable @typescript-eslint/no-explicit-any */

import type { Fact, Line } from "../text/line";
import * as audio from "./audio";
import * as browser from "./browser";
import * as camera from "./camera";
import * as cec from "./cec";
import * as presence from "./presence";
import * as clock from "./clock";
import * as schedule from "./schedule";
import * as script from "./script";
import * as storage from "./storage";
import * as device from "./device";
import * as journal from "./journal";
import * as net from "./net";
import * as ping from "./ping";
import * as playlist from "./playlist";
import * as printer from "./printer";
import * as screen from "./screen";
import * as tags from "./tags";
import * as transfer from "./transfer";
import * as update from "./update";
import * as time from "./time";

type Spans = [string, string, number][];

function spans(line: Line): Spans {
  return line.spans.map((span) => [span.tone, span.text, span.width]);
}

function lines(list: Line[]): Spans[] {
  return list.map(spans);
}

function facts(list: Fact[]): { label: string; value: Spans }[] {
  return list.map((item) => ({ label: item.label, value: spans(item.value) }));
}

export const renderers: Record<string, (input: any) => unknown> = {
  "device::status": (input) => {
    const text = device.status(input);
    return {
      facts: facts(text.facts),
      units: facts(text.units),
      more: facts(text.more),
      pending: text.pending ? spans(text.pending) : null,
    };
  },
  "device::applied": (input) => lines(device.applied(input.applied, input.no_apply ?? false)),
  "device::reverting": (input) => input.map((one: any) => device.reverting(one.pending, one.left)),
  "device::eval": (input) => {
    const shown = device.evalResult(input);
    return { line: spans(shown.line), thrown: shown.thrown };
  },
  "device::tags": (input) => input.map((list: string[]) => spans(device.tagsLine(list))),
  "tags::colour": (input) => input.map((tag: string) => tags.colour(tag)),
  "device::restarts": (input) =>
    input.map((consumer: any) => ({ long: device.restarts(consumer), short: device.restartsShort(consumer) })),
  "audio::summary": (input) => spans(audio.summary(input)),
  "time::summary": (input) => spans(time.summary(input)),
  "ping::event_line": (input) => input.map((event: any) => spans(ping.eventLine(event))),
  "cec::triggers": (input) => ({
    events: cec.EVENTS,
    keys: cec.KEYS,
    typed: input.map((typed: string) => {
      try {
        return { ok: cec.triggers(typed) };
      } catch (error) {
        return { err: (error as Error).message };
      }
    }),
  }),
  "cec::acted": (input) => input.map((one: any) => lines(cec.acted(one))),
  "cec::messages": (input) => lines(cec.messages(input)),
  "presence::triggers": (input) => ({
    events: presence.EVENTS,
    words: presence.EVENTS.map(script.presenceEvent),
    typed: input.map((typed: string) => {
      try {
        return { ok: presence.triggers(typed) };
      } catch (error) {
        return { err: (error as Error).message };
      }
    }),
  }),
  // --- screen and browser ---
  "screen::show": (input) =>
    Array.isArray(input) ? input.map((one) => lines(screen.show(one))) : lines(screen.show(input)),
  "browser::policies": (input) => lines(browser.policies(input)),
  "browser::policy_saved": (input) => input.map((one: any) => lines(browser.policySaved(one))),
  "browser::policy_removed": (input) => input.map((one: any) => lines(browser.policyRemoved(one))),
  "browser::policy_moved": (input) => input.map((one: any) => lines(browser.policyMoved(one))),
  "browser::effective": (input) => lines(browser.effective(input)),
  // --- network, wifi and certificates ---
  "net::change": (input) => lines(net.change(input)),
  "net::profile": (input) => lines(net.profile(input)),
  "net::proxy_test": (input) => input.map((one: any) => spans(net.proxyTest(one))),
  "net::cert": (input) => input.map((one: any) => spans(net.cert(one))),
  "speedtest::event_line": (input) => input.map((event: any) => spans(net.speedtestLine(event))),
  // --- storage, audio, time and schedules ---
  "time::facts": (input) => facts(time.facts(input)),
  "time::servers": (input) => facts(time.servers(input)),
  "time::hint_and_error": (input) => {
    const hint = time.hint(input);
    const error = time.error(input);
    return { hint: hint ? spans(hint) : null, error: error ? spans(error) : null };
  },
  "audio::show": (input) => lines(audio.show(input)),
  "audio::test": (input) => lines(audio.test(input)),
  "storage::event_line": (input) =>
    input.map((step: any) => {
      const shown = storage.eventLine(step);
      return shown ? spans(shown) : null;
    }),
  "storage::plan": (input) => {
    const shown = storage.planFacts(input);
    return {
      facts: facts(shown.facts),
      nothing: shown.nothing ? spans(shown.nothing) : null,
      grows: storage.grows(input),
    };
  },
  "camera::list": (input) => lines(camera.list(input)),
  "camera::presence": (input) => input.map((one: any) => lines(camera.presence(one))),
  "camera::calibrated": (input) => spans(camera.calibrated(input)),
  "printer::list": (input) => lines(printer.list(input)),
  "printer::show": (input) => facts(printer.show(input)),
  "printer::jobs": (input) => lines(printer.jobs(input)),
  "printer::found": (input) => ({
    found: input.map((one: any) => lines(printer.found(one))),
    hint: spans(printer.foundHint(input)),
    none: spans(printer.foundHint([])),
  }),
  "printer::queued": (input) => spans(printer.queued(input)),
  // --- playlists and the timetable ---
  "playlist::list": (input) => lines(playlist.list(input)),
  "playlist::show": (input) => ({ facts: facts(playlist.show(input)), items: lines(playlist.items(input)) }),
  "playlist::item": (input) =>
    input.map((one: any, at: number) => ({
      line: spans(playlist.item(one, at + 1)),
      timing: playlist.timing(one),
      options: playlist.options(one),
    })),
  "playlist::timetable": (input) => ({
    lines: lines(playlist.timetable(input)),
    entries: input.map((one: any) => spans(playlist.entry(one))),
  }),
  "playlist::status": (input) =>
    input.cases.map((one: any) => ({
      facts: facts(playlist.status(one, input.now)),
      summary: spans(playlist.summary(one)),
    })),
  "playlist::words": (input) => {
    const ok = <T>(parse: () => T): T | null => {
      try {
        return parse();
      } catch {
        return null;
      }
    };
    return {
      shorten: input.src.map(playlist.shorten),
      format_ms: input.ms.map(playlist.formatMs),
      format_position: input.ms.map(playlist.formatPosition),
      format_seconds: input.seconds.map(playlist.formatSeconds),
      parse_ms: input.typed.map((typed: string) => ok(() => playlist.parseMs(typed))),
      parse_seconds: input.typed.map((typed: string) => ok(() => playlist.parseSeconds(typed))),
      transition: input.transition.map(([kind, ms]: [any, number]) => playlist.transition(kind, ms)),
    };
  },
  "schedule::moment": (input) => input.moments.map((at: any) => spans(schedule.moment(at, input.now))),
  "schedule::last_run": (input) => input.infos.map((info: any) => spans(schedule.lastRun(info, input.now))),
  "script::last_run": (input) => input.infos.map((info: any) => spans(script.lastRun(info, input.now))),
  "script::runs": (input) => ({
    run_line: input.runs.map((run: any) => spans(script.runLine(run, input.now))),
    ended: input.runs.map((run: any) => spans(script.ended(run))),
    started_by: input.runs.map(script.startedBy),
  }),
  "script::events": (input) => input.events.map((event: any) => spans(script.eventLine(event, input.name))),
  "script::behaviour": (input) => input.map(script.behaviour),
  "schedule::words": (input) => ({
    format_timeout: input.seconds.map(schedule.formatTimeout),
    duration: input.seconds.map(schedule.duration),
    relative: input.relative.map(([at, now]: [number, number]) => schedule.relative(at, now)),
    parse_timeout: input.typed.map((typed: string) => {
      try {
        return schedule.parseTimeout(typed);
      } catch {
        return null;
      }
    }),
  }),
  "clock::words": (input) => ({
    span: input.span.map(clock.span),
    offset: input.offset.map(clock.offset),
    utc_offset: input.utc_offset.map(clock.utcOffset),
    precision: input.precision.map(clock.precision),
    drift: input.drift.map(clock.drift),
  }),
  // --- access, ssh, files, update and log ---
  "update::status_lines": (input) => lines(update.statusLines(input)),
  "update::warning": (input) => update.warning(input.name, input.wipe_data, input.repartition),
  "transfer::date": (input) => input.map((time: number) => transfer.date(time)),
  "files::summary_line": (input) => spans(transfer.summaryLine(input, input.verb)),
  "journal::parse": (input) =>
    input.map((event: any) => {
      const entry = journal.parse(event);
      return { clock: journal.clock(entry), message: entry.message, priority: entry.priority, source: entry.source };
    }),
};

export { facts, lines, spans };
