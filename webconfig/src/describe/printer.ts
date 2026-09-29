// agent/client/src/describe/printer.rs, line for line.

import type { Schemas } from "../api/client";
import { fact, Line, type Fact, type Tone } from "../text/line";
import { sizeLabel } from "./common";

/** A printer's state: printing along, stopped, or not set up yet. */
export function stateTone(state: string): Tone {
  switch (state) {
    case "idle":
    case "printing":
      return "ok";
    case "stopped":
      return "bad";
    default:
      return "warn";
  }
}

/** Whether the page may print, in a line. */
export function enabled(list: Schemas["PrinterList"]): Line {
  if (list.enabled) {
    return Line.of("ok", "page printing is on").text(" ").add("muted", "(window.print() and the page bridge)");
  }
  return Line.of("warn", "page printing is off; turn it on with")
    .text(" ")
    .add("cmd", "tessaro-ctl config set printer.enable=1");
}

/** `printer list`: a line per printer, the default marked, its URI under it, and whether the page may print. */
export function list(list: Schemas["PrinterList"]): Line[] {
  const lines: Line[] = [];
  if (list.printers.length === 0) {
    lines.push(
      Line.of("warn", "no printers;")
        .text(" ")
        .add("cmd", "tessaro-ctl printer discover")
        .text(" ")
        .add("muted", "finds them"),
    );
  }
  for (const printer of list.printers) {
    const marker = printer.default ? Line.of("ok", "*") : Line.plain(" ");
    let line = marker
      .text(" ")
      .pad("heading", printer.name, 16)
      .text(" ")
      .pad("label", printer.kind ?? "ipp", 4)
      .text(" ")
      .add(stateTone(printer.state), printer.state);
    const queued = printer.queued ?? 0;
    if (queued > 0) {
      line = line.text("  ").add("plain", `${queued} queued`);
    }
    lines.push(line);
    lines.push(Line.plain("    ").add("muted", printer.uri));
    if (printer.message) {
      lines.push(Line.plain("    ").add(stateTone(printer.state), printer.message));
    }
  }
  lines.push(new Line());
  lines.push(enabled(list));
  if (list.printers.length > 0) {
    lines.push(
      Line.of("muted", "* the default printer, used by window.print(); change it with")
        .text(" ")
        .add("cmd", "tessaro-ctl printer default NAME"),
    );
  }
  return lines;
}

/** `printer show`: one printer in full. */
export function show(printer: Schemas["PrinterInfo"]): Fact[] {
  const facts: Fact[] = [
    fact("name", Line.of("heading", printer.name)),
    fact("state", Line.of(stateTone(printer.state), printer.state)),
  ];
  if (printer.message) {
    facts.push(fact("says", Line.of(stateTone(printer.state), printer.message)));
  }
  facts.push(fact("uri", printer.uri));
  facts.push(fact("kind", (printer.kind ?? "ipp") === "ipp" ? "ipp (driverless)" : "raw (bytes as they are)"));
  if (printer.model) {
    facts.push(fact("model", printer.model));
  }
  facts.push(fact("paper", printer.media ? Line.plain(printer.media) : Line.of("muted", "the printer's own")));
  facts.push(fact("default", printer.default ? Line.of("ok", "yes") : Line.of("muted", "no")));
  facts.push(fact("queued", Line.plain(String(printer.queued ?? 0))));
  for (const marker of printer.markers ?? []) {
    const level =
      marker.level === null || marker.level === undefined
        ? Line.of("muted", "unknown")
        : Line.of(marker.level < 10 ? "warn" : "ok", `${marker.level}%`);
    facts.push(fact(marker.name, level));
  }
  return facts;
}

/** `printer jobs`: what is waiting, oldest first. */
export function jobs(jobs: Schemas["PrintJob"][]): Line[] {
  if (jobs.length === 0) {
    return [Line.of("muted", "no jobs waiting")];
  }
  const lines = jobs.map((job) =>
    new Line()
      .pad("heading", job.job, 20)
      .text(" ")
      .pad("plain", sizeLabel(job.size), 10)
      .text(" ")
      .add("muted", job.submitted),
  );
  lines.push(new Line());
  lines.push(Line.of("muted", "cancel one with").text(" ").add("cmd", "tessaro-ctl printer cancel JOB"));
  return lines;
}

/** One printer `printer discover` found, and the URI to add it by. */
export function found(found: Schemas["PrinterFound"]): Line[] {
  let first = new Line().pad("label", found.kind, 4).text(" ").add("heading", found.description);
  if (found.known) {
    first = first.text(" ").add("muted", `(printer ${found.known})`);
  }
  return [first, Line.plain("     ").add("muted", found.uri)];
}

/** What to do with what `printer discover` found. */
export function foundHint(found: Schemas["PrinterFound"][]): Line {
  if (found.length === 0) {
    return Line.of("warn", "no printers found; a network printer that is not announced is added by its URI");
  }
  return Line.of("muted", "add one with").text(" ").add("cmd", "tessaro-ctl printer create NAME --uri URI");
}

/** A document handed to CUPS. */
export function queued(queued: Schemas["PrintQueued"]): Line {
  return Line.of("ok", queued.message);
}
