// agent/client/src/describe/scanner.rs, line for line, with the parts of
// agent/protocol/src/scanner.rs the words need: a scanner's device id, its
// defaults, and the scanners a script runs on as a form reads them. The
// scanner_* fixtures keep them equal to the Rust's.

import type { Schemas } from "../api/client";
import { fact, Line, type Fact, type Tone } from "../text/line";

type Transport = Schemas["Transport"];

/** Names a scanner cannot have: the API's own path segments (`RESERVED`). */
export const RESERVED = ["discover", "identify", "logs"];

/** A script's trigger that runs it on every scanner's scans (`ANY`). */
export const ANY = "*";

/** The transports a scanner is read by, as the API names them. */
export const TRANSPORTS: Transport[] = ["keyboard", "serial", "hidpos"];

/** The baud rates a serial scanner may be read at (`BAUDS`). */
export const BAUDS = [1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200];

/** The terminators a scanner read this way may end its scans with. */
export function terminators(transport: Transport): string[] {
  switch (transport) {
    case "keyboard":
      return ["auto", "enter", "tab", "none"];
    case "serial":
      return ["auto", "cr", "lf", "crlf", "none", "0xNN"];
    case "hidpos":
      return ["auto"];
  }
}

/** The quiet that ends a scan by default, milliseconds. */
export function gapDefault(transport: Transport): number {
  return transport === "hidpos" ? 500 : 100;
}

/** `ScannerSpec::device`: `keyboard:0c2e:0b61:S1`, `serial:1a86:7523:@1-1.2`. */
export function device(spec: Schemas["ScannerSpec"]): string {
  let id = `${spec.transport}:${spec.vendor.toLowerCase()}:${spec.product.toLowerCase()}`;
  if (spec.serial != null) {
    id += `:${spec.serial}`;
  } else if (spec.port != null) {
    id += `:@${spec.port}`;
  }
  return id;
}

/** protocol::scanner::parse_device: the transport, ids, serial number or port of a device id. */
export function parseDevice(
  typed: string,
): Pick<Schemas["ScannerSpec"], "transport" | "vendor" | "product" | "serial" | "port"> {
  const wrong = () =>
    new Error(
      `${JSON.stringify(typed.trim())} is not a scanner device; one from \`tessaro-ctl scanner discover\`, e.g. keyboard:0c2e:0b61:@1-1.2`,
    );
  const parts = typed.trim().split(":");
  if (parts.length < 3) throw wrong();
  const kind = parts[0]!.trim().toLowerCase().replace("hid-pos", "hidpos");
  if (!(TRANSPORTS as string[]).includes(kind)) {
    throw new Error(`${JSON.stringify(parts[0])} is not a scanner transport; one of ${TRANSPORTS.join(", ")}`);
  }
  const vendor = parts[1]!.toLowerCase();
  const product = parts[2]!.toLowerCase();
  if (!/^[0-9a-f]{4}$/.test(vendor) || !/^[0-9a-f]{4}$/.test(product)) throw wrong();
  const rest = parts.slice(3).join(":");
  return {
    transport: kind as Transport,
    vendor,
    product,
    serial: rest !== "" && !rest.startsWith("@") ? rest : null,
    port: rest.startsWith("@") ? rest.slice(1) : null,
  };
}

/** `ScannerSpec::terminator`, by its name: what is stored, or `auto`. */
function terminatorName(spec: Schemas["ScannerSpec"]): string {
  const lower = (spec.terminator ?? "auto").trim().toLowerCase();
  if (lower === "") return "auto";
  if (/^0x[0-9a-f]{2}$/.test(lower)) return lower;
  return lower;
}

/** A scanner's state: being read, unplugged, switched off, or failing. */
export function stateTone(state: string): Tone {
  switch (state) {
    case "reading":
      return "ok";
    case "disabled":
      return "muted";
    case "failed":
      return "bad";
    default:
      return "warn";
  }
}

/** How a scanner is read, in words. */
export function transport(transport: Transport): string {
  switch (transport) {
    case "keyboard":
      return "keyboard (its keys never reach the page)";
    case "serial":
      return "serial port";
    case "hidpos":
      return "HID POS";
  }
}

/** A scan's text with what cannot be shown named: `<CR>`, `<GS>`, `<0x07>`. */
export function visible(text: string): string {
  let out = "";
  for (const ch of text) {
    const code = ch.codePointAt(0)!;
    switch (ch) {
      case "\r":
        out += "<CR>";
        break;
      case "\n":
        out += "<LF>";
        break;
      case "\t":
        out += "<TAB>";
        break;
      case "\u001d":
        out += "<GS>";
        break;
      case "\u001e":
        out += "<RS>";
        break;
      case "\u0004":
        out += "<EOT>";
        break;
      default:
        out += code < 0x20 || code === 0x7f ? `<0x${code.toString(16).padStart(2, "0")}>` : ch;
    }
  }
  return out;
}

/** Whether scanners are read at all, in a line. */
export function enabled(list: Schemas["ScannerList"]): Line {
  if (list.enabled) {
    return Line.of("ok", "scanners are read").text(" ").add("muted", "(scanner.enable)");
  }
  return Line.of("warn", "scanners are not read; turn them on with")
    .text(" ")
    .add("cmd", "tessaro-ctl config set scanner.enable=1");
}

/** `scanner list`: a line per scanner with its state, its device under it, and whether scanners are read. */
export function list(list: Schemas["ScannerList"]): Line[] {
  const lines: Line[] = [];
  if (list.scanners.length === 0) {
    lines.push(
      Line.of("warn", "no scanners;")
        .text(" ")
        .add("cmd", "tessaro-ctl scanner identify")
        .text(" ")
        .add("muted", "names the device you scan with"),
    );
  }
  for (const scanner of list.scanners) {
    let line = new Line()
      .pad("heading", scanner.name, 16)
      .text(" ")
      .pad("label", scanner.transport, 8)
      .text(" ")
      .add(stateTone(scanner.state), scanner.state);
    if (scanner.last_scan != null) {
      line = line.text("  ").add("muted", `last scan ${scanner.last_scan}`);
    }
    lines.push(line);
    let node = Line.plain("    ").add("muted", device(scanner));
    if (scanner.node != null) {
      node = node.text(" ").add("muted", `on ${scanner.node}`);
    }
    lines.push(node);
    if (scanner.message != null) {
      lines.push(Line.plain("    ").add(stateTone(scanner.state), scanner.message));
    }
  }
  lines.push(new Line());
  lines.push(enabled(list));
  return lines;
}

/** `scanner show`: one scanner in full. */
export function show(scanner: Schemas["ScannerInfo"]): Fact[] {
  const facts: Fact[] = [
    fact("name", Line.of("heading", scanner.name)),
    fact("state", Line.of(stateTone(scanner.state), scanner.state)),
  ];
  if (scanner.message != null) {
    facts.push(fact("says", Line.of(stateTone(scanner.state), scanner.message)));
  }
  facts.push(fact("read as", transport(scanner.transport)));
  facts.push(fact("device", device(scanner)));
  facts.push(fact("node", scanner.node != null ? Line.plain(scanner.node) : Line.of("muted", "not plugged in")));
  if (scanner.transport === "keyboard") {
    facts.push(fact("layout", scanner.layout ?? "us"));
  }
  if (scanner.transport !== "hidpos") {
    facts.push(fact("ends on", terminatorName(scanner)));
  }
  facts.push(fact("gap", Line.plain(`${scanner.gap_ms ?? gapDefault(scanner.transport)} ms of quiet ends a scan`)));
  if (scanner.transport === "serial") {
    facts.push(fact("baud", Line.plain(String(scanner.baud ?? 9600))));
  }
  if (scanner.strip_prefix != null) {
    facts.push(fact("strips", Line.plain(`${visible(scanner.strip_prefix)} first`)));
  }
  if (scanner.strip_suffix != null) {
    facts.push(fact("strips", Line.plain(`${visible(scanner.strip_suffix)} last`)));
  }
  facts.push(fact("enabled", (scanner.enabled ?? true) ? Line.of("ok", "yes") : Line.of("muted", "no")));
  facts.push(fact("scans", Line.plain(String(scanner.scans ?? 0))));
  facts.push(
    fact("last scan", scanner.last_scan != null ? Line.plain(scanner.last_scan) : Line.of("muted", "none yet")),
  );
  return facts;
}

/** One device `scanner discover` found, and the id to add it by. */
export function candidate(candidate: Schemas["ScannerCandidate"]): Line[] {
  let first = new Line().pad("label", candidate.transport, 8).text(" ").add("heading", candidate.description);
  if (candidate.known != null) {
    first = first.text(" ").add("muted", `(scanner ${candidate.known})`);
  }
  return [first, Line.plain("         ").add("plain", candidate.device).text(" ").add("muted", `on ${candidate.node}`)];
}

/** What to do with what `scanner discover` found. */
export function candidatesHint(found: Schemas["ScannerCandidate"][]): Line {
  if (found.length === 0) {
    return Line.of(
      "warn",
      "nothing that may be a scanner is plugged in: no USB keyboard, serial port or HID POS device",
    );
  }
  return Line.of("muted", "add one with")
    .text(" ")
    .add("cmd", "tessaro-ctl scanner create NAME --device DEVICE")
    .text(", ")
    .add("muted", "or let a scan pick it with")
    .text(" ")
    .add("cmd", "tessaro-ctl scanner identify");
}

/** What `scanner identify` heard: the device and its scan, and how to add it, or that nothing was scanned. */
export function identified(heard: Schemas["ScannerCandidate"] | null | undefined): Line[] {
  if (heard == null) {
    return [Line.of("warn", "nothing was scanned while the device listened; scan a code while it does")];
  }
  const lines = candidate(heard);
  if (heard.scan != null) {
    lines.push(Line.plain("         ").join(scanText(heard.scan)));
  }
  lines.push(
    heard.known != null
      ? Line.of("muted", `it is scanner ${heard.known} already; see`)
          .text(" ")
          .add("cmd", `tessaro-ctl scanner show ${heard.known}`)
      : Line.of("muted", "add it with")
          .text(" ")
          .add("cmd", `tessaro-ctl scanner create NAME --device ${heard.device}`),
  );
  return lines;
}

/** `1 byte`, `33 bytes`. */
export function bytes(length: number): string {
  return length === 1 ? "1 byte" : `${length} bytes`;
}

/** What a scan says, and how long it is. */
function scanText(scan: Schemas["Scan"]): Line {
  const text = scan.text != null ? Line.of("heading", visible(scan.text)) : Line.of("warn", "not text");
  let about = `${bytes(scan.length)} in ${scan.ms} ms`;
  if (scan.symbology != null) {
    about += `, ${scan.symbology}`;
  }
  return text.text(" ").add("muted", `(${about})`);
}

/** One scan of `scanner test`, with when it ended. */
export function scan(scan: Schemas["Scan"]): Line {
  return new Line().pad("muted", scan.time, 13).join(scanText(scan));
}

/** One entry of the scanners' log: when, which scanner, what happened. */
export function logEntry(entry: Schemas["ScanLogEntry"]): Line {
  const line = new Line().pad("muted", entry.time, 13).pad("heading", entry.scanner, 17);
  switch (entry.event) {
    case "scan": {
      let what = `scan, ${bytes(entry.length ?? 0)} in ${entry.ms ?? 0} ms`;
      if (entry.symbology != null) {
        what += `, ${entry.symbology}`;
      }
      return line.add("plain", what);
    }
    case "connected": {
      const connected = line.add("ok", "connected");
      return entry.message != null ? connected.text(" ").add("muted", `on ${entry.message}`) : connected;
    }
    case "disconnected":
      return line.add("warn", "disconnected");
    case "failed":
      return line.add("bad", `failed: ${entry.message ?? "no reason given"}`);
    default:
      return line.add("plain", entry.event);
  }
}

/** A page of the log, oldest first. */
export function logs(page: Schemas["ScanLog"]): Line[] {
  if (page.entries.length === 0) {
    return [Line.of("muted", "nothing has happened to a scanner yet")];
  }
  return page.entries.map(logEntry);
}

/** protocol::scanner::check_name. */
export function checkName(name: string): void {
  const valid = new TextEncoder().encode(name).length <= 40 && /^[a-z0-9]/.test(name) && /^[a-z0-9_-]*$/.test(name);
  if (!valid) {
    throw new Error(
      `${JSON.stringify(name)} is not a scanner name: lower-case letters, digits, - and _, starting with a letter or digit, at most 40`,
    );
  }
  if (RESERVED.includes(name)) {
    throw new Error(`${JSON.stringify(name)} cannot name a scanner`);
  }
}

/** One entry of what a script runs on, as typed: a scanner's name, or `*` for every scanner. */
export function trigger(typed: string): string {
  const name = typed.trim().toLowerCase();
  if (name === ANY) return name;
  checkName(name);
  return name;
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
