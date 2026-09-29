// The ports by the Rust function's name, rendered as the golden fixtures
// record them (agent/client/tests/describe.rs `render`). Keep both lists
// the same.

/* eslint-disable @typescript-eslint/no-explicit-any */

import type { Fact, Line } from "../text/line";
import * as audio from "./audio";
import * as browser from "./browser";
import * as clock from "./clock";
import * as schedule from "./schedule";
import * as storage from "./storage";
import * as device from "./device";
import * as journal from "./journal";
import * as net from "./net";
import * as ping from "./ping";
import * as printer from "./printer";
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
  "device::eval": (input) => {
    const shown = device.evalResult(input);
    return { line: spans(shown.line), thrown: shown.thrown };
  },
  "device::restarts": (input) =>
    input.map((consumer: any) => ({ long: device.restarts(consumer), short: device.restartsShort(consumer) })),
  "audio::summary": (input) => spans(audio.summary(input)),
  "time::summary": (input) => spans(time.summary(input)),
  "ping::event_line": (input) => input.map((event: any) => spans(ping.eventLine(event))),
  // --- screen and browser ---
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
  "printer::list": (input) => lines(printer.list(input)),
  "printer::show": (input) => facts(printer.show(input)),
  "printer::jobs": (input) => lines(printer.jobs(input)),
  "printer::found": (input) => ({
    found: input.map((one: any) => lines(printer.found(one))),
    hint: spans(printer.foundHint(input)),
    none: spans(printer.foundHint([])),
  }),
  "printer::queued": (input) => spans(printer.queued(input)),
  "schedule::moment": (input) => input.moments.map((at: any) => spans(schedule.moment(at, input.now))),
  "schedule::last_run": (input) => input.infos.map((info: any) => spans(schedule.lastRun(info, input.now))),
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
