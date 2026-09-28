// agent/client/src/describe/net.rs, speedtest.rs and certs.rs: the network,
// its profiles, WiFi, the proxy, certificate authorities and what a change
// did, for the Network, WiFi and Certificates pages.

import type { Schemas } from "../api/client";
import { Line, row, unitState, yesNo, type Tone } from "../text/line";
import { fixed } from "./common";

/** `192.168.1.5/24 global`. */
export function address(one: Schemas["NetAddress"]): Line {
  return Line.plain(`${one.address}/${one.prefix} `).add("muted", one.scope);
}

/** One profile, in full. */
export function profile(detail: Schemas["NetProfileDetail"]): Line[] {
  const p = detail.profile;
  const none = () => Line.of("muted", "(none)");
  const sub = (label: string, value: Line) => Line.plain("    ").pad("label", label, 15).text(" ").join(value);

  let device: Line;
  if (p.device && p.active) device = Line.plain(`${p.device}  `).add("ok", "active");
  else if (p.device) device = Line.plain(`${p.device}  `).add("muted", "(not active)");
  else device = Line.of("muted", "(any, not active)");

  const lines = [
    Line.of("heading", p.name).text(" ").add("muted", `(${p.uuid})`),
    row("kind", p.kind),
    row("device", device),
    row("autoconnect", yesNo(p.autoconnect).text("  ").add("muted", `(priority ${p.priority})`)),
    row(
      "saved",
      p.managed
        ? Line.of("ok", "managed").text("  ").add("muted", "(rendered from settings every boot)")
        : p.saved
          ? Line.of("ok", "yes")
          : Line.of("warn", "no").text("  ").add("muted", "(lost at reboot)"),
    ),
  ];
  if (detail.wifi) {
    lines.push(row("ssid", Line.of("heading", detail.wifi.ssid)));
    lines.push(row("security", detail.wifi.security));
    lines.push(row("hidden", yesNo(detail.wifi.hidden)));
  }
  for (const [family, settings] of [
    ["ipv4", detail.ipv4],
    ["ipv6", detail.ipv6],
  ] as const) {
    lines.push(row(family, settings.method));
    if (settings.addresses.length > 0 || settings.method === "manual") {
      lines.push(
        sub("addresses", settings.addresses.length === 0 ? none() : Line.plain(settings.addresses.join(", "))),
      );
    }
    if (settings.gateway) lines.push(sub("gateway", Line.plain(settings.gateway)));
    if (settings.dns.length > 0) lines.push(sub("dns", Line.plain(settings.dns.join(", "))));
    if (settings.ignore_auto_dns) lines.push(sub("ignore-auto-dns", yesNo(true)));
  }
  if (p.active) {
    let now: Line;
    if (detail.addresses.length === 0) {
      now = Line.of("warn", "(no address)");
    } else {
      now = new Line();
      detail.addresses.forEach((one, at) => {
        if (at > 0) now = now.text(", ");
        now = now.join(address(one));
      });
    }
    lines.push(row("now", now));
  }
  return lines;
}

/** How good a signal is, in percent. */
export function signalTone(signal: number): Tone {
  return signal >= 60 ? "ok" : signal >= 30 ? "warn" : "bad";
}

/** The band of a frequency: `2.4G`, `5G`, `6G`. */
export function band(mhz: number): string {
  return mhz <= 3000 ? "2.4G" : mhz <= 5925 ? "5G" : "6G";
}

/** What a network change did: kept or rolled back, its checks, why. */
export function change(change: Schemas["NetChange"]): Line[] {
  let first = (change.outcome === "committed" ? Line.of("ok", "committed") : Line.of("bad", "rolled back"))
    .text(" ")
    .add("muted", change.action);
  if (change.profile) first = first.text(" ").add("heading", change.profile);
  if (change.uuid) first = first.text(" ").add("muted", `(${change.uuid})`);
  const lines = [first];
  for (const check of change.checks) {
    const result = check.passed ? Line.of("ok", "passed") : Line.of("bad", "failed");
    lines.push(new Line().pad("label", check.name, 12).text(" ").join(result).text(` ${check.detail}`));
  }
  if (change.reason) lines.push(new Line().pad("label", "why", 12).text(` ${change.reason}`));
  if (change.note) lines.push(Line.of("warn", change.note));
  return lines;
}

/** certs::date: seconds since the epoch as a UTC date, `2036-09-25`. */
export function certDate(unix: number): string {
  const day = new Date(Math.floor(unix / 86400) * 86400 * 1000);
  const year = day.getUTCFullYear();
  const pad = (n: number, width: number) => String(n).padStart(width, "0");
  return `${pad(year, 4)}-${pad(day.getUTCMonth() + 1, 2)}-${pad(day.getUTCDate(), 2)}`;
}

/** certs::expired, by this browser's clock. */
export function expired(notAfter: number, now = Date.now() / 1000): boolean {
  return Math.floor(now) > notAfter;
}

/** One certificate authority on a line. `now` for the fixtures. */
export function cert(info: Schemas["CertInfo"], now?: number): Line {
  const date = certDate(info.not_after);
  const line = Line.of("muted", info.fingerprint).text("  ").add("heading", info.subject).text("  ");
  const dated = expired(info.not_after, now) ? line.add("bad", `expired ${date}`) : line.add("label", `until ${date}`);
  return info.self_signed ? dated : dated.text(" ").add("muted", `issued by ${info.issuer}`);
}

/** The proxy the device uses, what bypasses it, the local one. */
export function proxy(status: Schemas["ProxyStatus"]): Line[] {
  const lines = [
    row(
      "proxy",
      status.url ? Line.of("heading", status.url) : Line.of("muted", "(none: everything goes straight out)"),
    ),
  ];
  if (status.url) {
    lines.push(
      row(
        "bypass",
        status.bypass.length === 0 ? Line.of("muted", "(loopback only)") : Line.plain(status.bypass.join(", ")),
      ),
    );
  }
  lines.push(row("local proxy", Line.plain(`${status.listen} `).add(unitState(status.unit), status.unit)));
  lines.push(new Line());
  lines.push(
    Line.of("cmd", status.url ? "tessaro-ctl network proxy test" : "tessaro-ctl network proxy set http://HOST:PORT"),
  );
  return lines;
}

/** What `network proxy test` found. */
export function proxyTest(tested: Schemas["ProxyTested"]): Line {
  if (tested.ip) {
    return Line.of("ok", "through the proxy the internet sees").text(" ").add("heading", tested.ip);
  }
  return Line.of("bad", "the proxy did not get through:").text(` ${tested.error ?? "no answer"}`);
}

/** protocol::SPEEDTEST_SIZES. */
export const SPEEDTEST_SIZES = [100_000, 1_000_000, 10_000_000, 25_000_000, 100_000_000];

/** protocol::speedtest_size_label: `100k`, `1m`. */
export function speedtestSizeLabel(size: number): string {
  return size >= 1_000_000 ? `${Math.floor(size / 1_000_000)}m` : `${Math.floor(size / 1_000)}k`;
}

/** speedtest::event_line: one step of the speed test job. */
export function speedtestLine(step: Schemas["SpeedtestEvent"]): Line {
  const value = (tone: Tone, v: number | null | undefined, unit: string) =>
    v == null ? Line.of("muted", "n/a") : Line.of(tone, `${fixed(v, 1)} ${unit}`);
  const number = (v: number | null | undefined, unit: string) => (v == null ? "n/a" : `${fixed(v, 1)} ${unit}`);
  const label = (text: string) => new Line().pad("label", text, 9).text(" ");
  switch (step.phase) {
    case "server":
      return label("server")
        .text("Cloudflare ")
        .add("heading", step.colo)
        .text(`, seen from ${step.ip} (${step.country})`);
    case "latency":
      return label("latency")
        .join(value("heading", step.avg_ms, "ms"))
        .text(" ")
        .add("muted", `(min ${number(step.min_ms, "ms")}, max ${number(step.max_ms, "ms")}, ${step.samples} samples)`);
    case "transfer": {
      const counted: Tone = step.samples < step.attempts ? "warn" : "muted";
      return label(step.direction)
        .pad("heading", speedtestSizeLabel(step.size), 5)
        .text(" ")
        .join(value("heading", step.median_mbit, "Mbit/s"))
        .text(" ")
        .add("muted", `(min ${number(step.min_mbit, "Mbit/s")}, max ${number(step.max_mbit, "Mbit/s")},`)
        .text(" ")
        .add(counted, `${step.samples}/${step.attempts} samples)`);
    }
    case "result":
      return new Line()
        .pad("heading", "result", 9)
        .text(" download ")
        .join(value("ok", step.download_mbit, "Mbit/s"))
        .text(", upload ")
        .join(value("ok", step.upload_mbit, "Mbit/s"))
        .text(", latency ")
        .join(value("ok", step.latency_ms, "ms"));
  }
}
