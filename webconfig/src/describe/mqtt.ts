// agent/client/src/describe/mqtt.rs: the device's link to its MQTT broker,
// for the MQTT page and the line of the device status. Kept equal to the
// Rust by the golden fixtures.

import type { Schemas } from "../api/client";
import { fact, Line, type Fact } from "../text/line";
import { duration } from "./schedule";

type Status = Schemas["MqttStatus"];

/** protocol::mqtt::TRIGGERS_MAX: the most topic filters one script runs on. */
export const TRIGGERS_MAX = 16;

/** protocol::mqtt::topic::check_filter: `+` alone on its level, `#` alone and last. */
export function checkFilter(filter: string): void {
  if (filter === "" || filter.length > 65535) throw new Error("a filter is 1 to 65535 bytes");
  if (filter.includes("\0")) throw new Error("NUL is not allowed in a filter");
  const levels = filter.split("/");
  levels.forEach((level, at) => {
    if (level.includes("#") && (level !== "#" || at + 1 !== levels.length)) {
      throw new Error(`${JSON.stringify(filter)}: # must be a level of its own, and the last`);
    }
    if (level.includes("+") && level !== "+") {
      throw new Error(`${JSON.stringify(filter)}: + must be a level of its own`);
    }
  });
}

/** protocol::mqtt::triggers on what was typed, comma separated: each checked, none twice. */
export function triggers(typed: string): string[] {
  const out: string[] = [];
  for (const filter of typed.split(",").map((one) => one.trim())) {
    if (filter === "") continue;
    checkFilter(filter);
    if (!out.includes(filter)) out.push(filter);
  }
  if (out.length > TRIGGERS_MAX) throw new Error(`a script runs on at most ${TRIGGERS_MAX} topic filters`);
  return out;
}

/** The state in a word, toned. */
function state(status: Status): Line {
  switch (status.state) {
    case "off":
      return Line.of("muted", "off");
    case "connecting":
      return Line.of("warn", "connecting");
    case "connected":
      return Line.of("ok", "connected");
    case "failed":
      return Line.of("bad", "not connected");
  }
}

/** The one line the device status shows: `connected to mqtts://b:8883`. */
export function summary(status: Status): Line {
  const line = state(status);
  if (status.state === "off") return line.add("muted", ", mqtt.url is not set");
  if (status.broker == null) return line;
  const to = line.text(" to ").add("heading", status.broker);
  if (status.state === "connected") return to;
  return status.last_error != null ? to.text(`: ${status.last_error}`) : to;
}

/** The link, a fact a row. */
export function facts(status: Status): Fact[] {
  const facts: Fact[] = [];
  let now = state(status);
  if (status.seconds != null) now = now.add("muted", ` for ${duration(status.seconds)}`);
  facts.push(fact("state", now));
  if (status.state === "off") {
    facts.push(
      fact("", Line.of("muted", "set mqtt.url to connect: ").add("cmd", "tessaro-ctl config set mqtt.url=mqtts://...")),
    );
  }
  if (status.broker != null) facts.push(fact("broker", Line.of("heading", status.broker)));
  if (status.last_error != null) facts.push(fact("last error", Line.of("bad", status.last_error)));
  facts.push(fact("client id", Line.plain(status.client_id)));
  facts.push(fact("user", status.username != null ? Line.plain(status.username) : Line.of("muted", "none")));
  facts.push(fact("password", status.password ? Line.of("secret", "kept") : Line.of("muted", "none")));
  facts.push(fact("client cert", status.client_cert ? Line.plain("kept") : Line.of("muted", "none")));
  facts.push(fact("topics", Line.plain(`${status.topic}/...`)));
  let commands: Line;
  if (status.commands === "full") commands = Line.plain("full").add("muted", ", every command");
  else if (status.commands === "read") commands = Line.plain("read").add("muted", ", only what reads");
  else commands = Line.of("muted", status.commands);
  facts.push(fact("commands", commands));
  for (const warning of status.warnings ?? []) facts.push(fact("", Line.of("warn", warning)));
  if (status.state !== "off") {
    const queued = status.queued ?? 0;
    facts.push(
      fact(
        "messages",
        Line.plain(`${status.published ?? 0} published, ${status.received ?? 0} received`).add(
          queued > 0 ? "warn" : "muted",
          `, ${queued} waiting`,
        ),
      ),
    );
  }
  const subscriptions = status.subscriptions ?? [];
  if (subscriptions.length > 0) facts.push(fact("subscribed", Line.plain(subscriptions.join(" "))));
  return facts;
}

/** What a test found, a line a finding. */
export function tested(tested: Schemas["MqttTested"]): Line[] {
  const lines = [
    tested.ok
      ? Line.of("ok", "the broker took the connection")
          .text(" at ")
          .add("heading", tested.broker)
          .add("muted", ` (${tested.millis} ms)`)
      : Line.of("bad", `stopped at ${tested.stage ?? "the link"}:`)
          .text(` ${tested.error ?? "no reason given"}`)
          .add("muted", ` (${tested.broker})`),
  ];
  if (tested.fingerprint != null) {
    lines.push(
      Line.of("muted", "its certificate's SHA-256, for mqtt.tls.fingerprint: ").add("plain", tested.fingerprint),
    );
  }
  return lines;
}
