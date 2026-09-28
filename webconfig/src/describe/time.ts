// agent/client/src/describe/time.rs, the parts the web pages use.

import type { Schemas } from "../api/client";
import { fact, Line, unitState, type Fact } from "../text/line";
import { drift, leap, offset, offsetBetween, precision, span, utcOffset } from "./clock";

export function summary(summary: Schemas["TimeSummary"]): Line {
  const zone = summary.timezone ?? "(unknown timezone)";
  let sync: Line;
  if (summary.ntp === false) sync = Line.of("warn", "NTP off");
  else if (summary.synchronized === true) sync = Line.of("ok", "in sync");
  else if (summary.synchronized === false) sync = Line.of("warn", "not in sync");
  else sync = Line.of("muted", "sync unknown");
  return Line.plain(`${zone}, `).join(sync);
}

export function outcome(text: string): Line {
  return text.startsWith("saved,") ? Line.of("warn", text) : Line.of("ok", text);
}

/** What could not be read, when something could not. */
export function error(status: Schemas["TimeStatus"]): Line | null {
  return status.error ? Line.of("bad", "not everything could be read:").text(` ${status.error}`) : null;
}

/** TimeStatus::frequency_ppm: adjtimex's 2^-16 ppm as ppm. */
function frequencyPpm(status: Schemas["TimeStatus"]): number | null {
  return status.frequency == null ? null : status.frequency / 65536;
}

/** The clock and how it is kept, a fact a row. */
export function facts(status: Schemas["TimeStatus"]): Fact[] {
  const none = () => Line.of("muted", "(not reported)");
  const facts: Fact[] = [];

  let zone: Line;
  if (status.timezone) {
    zone = Line.plain(status.timezone);
    if (status.zone_abbreviation && status.utc_offset_seconds != null) {
      zone = zone.text(" ").add("muted", `(${status.zone_abbreviation}, ${utcOffset(status.utc_offset_seconds)})`);
    }
  } else {
    zone = none();
  }
  facts.push(fact("timezone", zone));
  if (status.timezone && status.timezone !== status.setting_timezone) {
    facts.push(fact("", Line.of("warn", `time.timezone is ${status.setting_timezone}, not applied yet`)));
  }
  facts.push(fact("local time", status.local_time ? Line.plain(status.local_time) : none()));
  facts.push(
    fact(
      "in sync",
      status.synchronized === true
        ? Line.of("ok", "yes")
        : status.synchronized === false
          ? Line.of("warn", "no")
          : none(),
    ),
  );
  let ntp: Line;
  if (status.can_ntp === false) ntp = Line.of("bad", "not available on this image");
  else if (status.ntp === true) ntp = Line.of("ok", "on");
  else if (status.ntp === false) ntp = Line.of("warn", "off");
  else ntp = none();
  facts.push(fact("ntp", ntp));
  if (status.timesyncd) {
    facts.push(fact("timesyncd", Line.of(unitState(status.timesyncd), status.timesyncd)));
  }

  if (status.timesyncd === "active") {
    const name = status.server_name;
    const address = status.server_address;
    let server: Line;
    if (name && address && name !== address) server = Line.plain(`${name} `).add("muted", `(${address})`);
    else if (name) server = Line.plain(name);
    else if (address) server = Line.plain(address);
    else server = Line.of("warn", "none yet");
    facts.push(fact("server", server));
    if (status.poll_interval_usec != null) {
      const line = Line.plain(`every ${span(status.poll_interval_usec)}`);
      facts.push(
        fact(
          "poll",
          status.poll_interval_min_usec != null && status.poll_interval_max_usec != null
            ? line
                .text(" ")
                .add("muted", `(${span(status.poll_interval_min_usec)} to ${span(status.poll_interval_max_usec)})`)
            : line,
        ),
      );
    }
    const last = status.last;
    if (last) {
      facts.push(fact("offset", offset(last.offset_usec)));
      facts.push(fact("delay", span(Math.abs(last.delay_usec))));
      facts.push(fact("jitter", span(last.jitter_usec)));
      const ppm = frequencyPpm(status);
      if (ppm !== null) {
        facts.push(fact("drift", Line.plain(`${drift(ppm)} `).add("muted", "(the kernel's frequency correction)")));
      }
      const distance = Line.plain(span(Math.floor(last.root_delay_usec / 2) + last.root_dispersion_usec));
      facts.push(
        fact(
          "root dist.",
          status.root_distance_max_usec != null
            ? distance.text(" ").add("muted", `(max ${span(status.root_distance_max_usec)})`)
            : distance,
        ),
      );
      facts.push(
        fact(
          "stratum",
          Line.plain(`${last.stratum} `).add(
            "muted",
            `(reference ${last.reference}, precision ${precision(last.precision)})`,
          ),
        ),
      );
      if (last.leap !== 0) facts.push(fact("leap", Line.of("warn", leap(last.leap))));
      const age =
        status.now_usec != null && status.now_usec >= last.received_usec
          ? `${span(status.now_usec - last.received_usec)} ago, `
          : "";
      const answers = Line.plain(`${age}${last.packet_count} so far`);
      facts.push(fact("answers", last.spike ? answers.text(", ").add("warn", "the last one was an outlier") : answers));
    } else {
      facts.push(fact("answers", Line.of("warn", "none yet")));
    }
  }

  if (status.rtc_usec != null && status.now_usec != null) {
    const line = Line.plain(`${offsetBetween(status.rtc_usec, status.now_usec)} from the system clock`);
    facts.push(fact("hw clock", status.local_rtc === true ? line.add("warn", " (keeps local time)") : line));
  } else if (status.now_usec != null) {
    facts.push(fact("hw clock", Line.of("muted", "none")));
  }
  return facts;
}

/** Each source of NTP servers, with where it comes from. */
export function servers(status: Schemas["TimeStatus"]): Fact[] {
  const sources: [string, string[], string][] = [
    ["runtime", status.servers.runtime, "DHCP's, while time.ntp.servers is empty"],
    ["system", status.servers.system, "time.ntp.servers"],
    ["fallback", status.servers.fallback, "the image's"],
    ["dhcp", status.servers.dhcp, "what the network offers"],
  ];
  return sources.map(([label, names, from]) =>
    fact(
      label,
      (names.length === 0 ? Line.of("muted", "(none)") : Line.plain(names.join(" ")))
        .text(" ")
        .add("muted", `(${from})`),
    ),
  );
}

/** What to try when NTP is on but the clock is not in sync. */
export function hint(status: Schemas["TimeStatus"]): Line | null {
  return status.ntp === true && status.synchronized === false
    ? Line.of("muted", "not in sync? name a reachable server:")
        .text(" ")
        .add("cmd", "tessaro-ctl time ntp on --server HOST")
    : null;
}
