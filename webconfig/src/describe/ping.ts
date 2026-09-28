// agent/client/src/ping.rs: the round trips to the device, and the lines
// a device's `network ping` job reads as.

import type { Schemas } from "../api/client";
import { Line, type Tone } from "../text/line";
import { fixed } from "./common";

const LABEL = 9;
/** protocol::PING_MAX_COUNT. */
export const PING_MAX_COUNT = 100;

export function label(text: string): Line {
  return new Line().pad("label", text, LABEL).text(" ");
}

export function ms(millis: number): string {
  return `${fixed(millis, 1)} ms`;
}

/** ping::check_count. */
export function checkCount(count: number): string | null {
  if (!Number.isInteger(count) || count < 1) return "the count must be at least 1";
  if (count > PING_MAX_COUNT) return `the count can be at most ${PING_MAX_COUNT}`;
  return null;
}

export function summaryLine(
  sent: number,
  received: number,
  min: number | null | undefined,
  avg: number | null | undefined,
  max: number | null | undefined,
  verb: string,
): Line {
  const loss = sent === 0 ? 0 : Math.floor(((sent - Math.min(received, sent)) * 100) / sent);
  const lossTone: Tone = loss === 0 ? "ok" : loss === 100 ? "bad" : "warn";
  const number = (value: number | null | undefined) => (value == null ? "n/a" : fixed(value, 1));
  const line = new Line()
    .pad("heading", "result", LABEL)
    .text(` ${received}/${sent} ${verb}, `)
    .add(lossTone, `${loss}% lost`);
  if (min == null && avg == null && max == null) return line;
  return line.text(" ").add("muted", `(min ${number(min)}, avg ${number(avg)}, max ${number(max)} ms)`);
}

/** One step of a device's `network ping` job. */
export function eventLine(step: Schemas["PingEvent"]): Line {
  switch (step.event) {
    case "start": {
      const line = label("ping").add("heading", step.host);
      return step.host === step.address ? line : line.text(` (${step.address})`);
    }
    case "reply":
      return label("reply")
        .add("heading", ms(step.rtt_ms))
        .text(" ")
        .add("muted", `seq=${step.seq} ${step.bytes} bytes`);
    case "timeout":
      return label("timeout").add("warn", "no reply").text(" ").add("muted", `seq=${step.seq}`);
    case "summary":
      return summaryLine(step.sent, step.received, step.min_ms, step.avg_ms, step.max_ms, "received");
  }
}

/** A round trip to the device itself, from this browser. */
export function replyLine(rtt: number, seq: number): Line {
  return label("reply").add("heading", ms(rtt)).text(" ").add("muted", `seq=${seq}`);
}

export function lostLine(error: string, seq: number): Line {
  return label("lost").add("bad", error).text(" ").add("muted", `seq=${seq}`);
}

/** `result 3/4 answered ...` from the round trips measured. */
export function deviceSummary(sent: number, rtts: number[]): Line {
  const min = rtts.length ? Math.min(...rtts) : null;
  const max = rtts.length ? Math.max(...rtts) : null;
  const avg = rtts.length ? rtts.reduce((a, b) => a + b, 0) / rtts.length : null;
  return summaryLine(sent, rtts.length, min, avg, max, "answered");
}
