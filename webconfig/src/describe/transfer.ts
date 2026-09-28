// agent/client/src/report.rs, transfer.rs and files.rs: progress lines, the
// rate of a transfer, and what a transfer did, in words.

import { Line, type Tone } from "../text/line";
import { fixed, sizeLabel } from "./common";

/** transfer::mb: `12.0 MB`. */
export function mb(bytes: number): string {
  return `${fixed(bytes / 1e6, 1)} MB`;
}

/** report::percent. */
export function percent(done: number, total: number): number {
  return Math.floor((done * 100) / Math.max(total, 1));
}

/** report::clock: `1:15`, `1:02:05`. */
export function clock(seconds: number): string {
  const whole = Math.floor(seconds);
  const two = (value: number) => String(value).padStart(2, "0");
  if (whole >= 3600) {
    return `${Math.floor(whole / 3600)}:${two(Math.floor(whole / 60) % 60)}:${two(whole % 60)}`;
  }
  return `${Math.floor(whole / 60)}:${two(whole % 60)}`;
}

/** report::step_line: the step's verb in its own column, then the details. */
export function stepLine(tone: Tone, verb: string, rest: Line | string): Line {
  return new Line()
    .pad(tone, verb, 10)
    .text(" ")
    .join(typeof rest === "string" ? Line.plain(rest) : rest);
}

/** report::Rate: speed over the last few seconds, and the time left. */
export class Rate {
  readonly started = performance.now();
  private samples: [number, number][];

  constructor(from: number) {
    this.samples = [[this.started, from]];
  }

  /** Seconds since it started. */
  elapsed(): number {
    return (performance.now() - this.started) / 1000;
  }

  update(done: number, total: number): [number, string] {
    const now = performance.now();
    this.samples.push([now, done]);
    while (this.samples.length > 2 && now - this.samples[0]![0] > 5000) {
      this.samples.shift();
    }
    const [then, before] = this.samples[0]!;
    const seconds = (now - then) / 1000;
    if (seconds <= 0 || done <= before) {
      return [0, "--:--"];
    }
    const speed = (done - before) / seconds;
    return [speed, clock(Math.max(0, total - done) / speed)];
  }

  /** `12.0 MB/40.0 MB   30%  4.1 MB/s  ETA 0:07`. */
  line(done: number, total: number): Line {
    const [speed, eta] = this.update(done, total);
    return Line.plain(
      `${mb(done)}/${mb(total)}  ${String(percent(done, total)).padStart(3)}%  ${mb(Math.floor(speed))}/s  `,
    )
      .add("label", "ETA")
      .text(` ${eta}`);
  }
}

/** transfer::date: `2025-09-24 16:00`, UTC. */
export function date(mtime: number): string {
  const days = Math.floor(mtime / 86_400);
  const seconds = mtime - days * 86_400;
  // Howard Hinnant's days-to-civil, as the Rust.
  const z = days + 719_468;
  const era = Math.floor(z / 146_097);
  const doe = z - era * 146_097;
  const yoe = Math.floor((doe - Math.floor(doe / 1460) + Math.floor(doe / 36_524) - Math.floor(doe / 146_096)) / 365);
  const doy = doe - (365 * yoe + Math.floor(yoe / 4) - Math.floor(yoe / 100));
  const mp = Math.floor((5 * doy + 2) / 153);
  const day = doy - Math.floor((153 * mp + 2) / 5) + 1;
  const month = mp < 10 ? mp + 3 : mp - 9;
  const year = yoe + era * 400 + (month <= 2 ? 1 : 0);
  const two = (value: number) => String(value).padStart(2, "0");
  return `${String(year).padStart(4, "0")}-${two(month)}-${two(day)} ${two(Math.floor(seconds / 3600))}:${two(Math.floor(seconds / 60) % 60)}`;
}

/** What a transfer did (files::Summary). */
export interface Summary {
  sent: string[];
  received: string[];
  removed: string[];
  made: string[];
  unchanged: string[];
  skipped: string[];
  bytes: number;
}

export function emptySummary(): Summary {
  return { sent: [], received: [], removed: [], made: [], unchanged: [], skipped: [], bytes: 0 };
}

/** `done: 3 sent (1.2 MB), 2 unchanged`, with `verb` for what moved. */
export function summaryLine(summary: Summary, verb: string): Line {
  const moved = summary.sent.length + summary.received.length;
  let line = Line.of("ok", "done:").text(` ${moved} ${verb} (${mb(summary.bytes)})`);
  if (summary.removed.length > 0) {
    line = line.text(`, ${summary.removed.length} removed`);
  }
  line = line.text(`, ${summary.unchanged.length} unchanged`);
  if (summary.skipped.length > 0) {
    line = line.text(", ").add("warn", `${summary.skipped.length} skipped`);
  }
  return line;
}

/** files: `sent path  1.2 MB`, `received ...`. */
export function movedLine(verb: "sent" | "received", path: string, size: number): Line {
  return stepLine("ok", verb, Line.plain(`${path}  `).add("muted", sizeLabel(size)));
}

/** A file's progress while it moves: `sending path  12.0 MB/40.0 MB ...`. */
export function movingLine(verb: "sending" | "receiving", path: string, rate: Rate, done: number, total: number): Line {
  return stepLine("label", verb, Line.plain(`${path}  `).join(rate.line(done, total)));
}
