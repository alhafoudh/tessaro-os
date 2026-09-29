// agent/client/src/describe/device.rs: the device itself, for Overview,
// the settings and Browser. Kept equal to the Rust by the golden fixtures.

import type { Schemas } from "../api/client";
import { fact, Line, unitState, usageLevel, yesNo, type Fact } from "../text/line";
import * as audio from "./audio";
import { freeLine, memUsedPercent, previousOrDefault, usageLine } from "./common";
import * as time from "./time";

export const CONFIRM_COMMAND = "tessaro-ctl screen confirm";
export const REBOOT_COMMAND = "tessaro-ctl device reboot";

export function node(info: Schemas["NodeInfo"]): Fact[] {
  return [
    fact("name", Line.of("heading", info.name)),
    fact("node id", info.id),
    fact("machine", info.machine),
    fact("agent", info.version),
    fact("fingerprint", Line.of("muted", info.fingerprint)),
    fact("claimed", yesNo(info.claimed)),
  ];
}

function machine(hardware: Schemas["Hardware"]): string | null {
  const parts = [hardware.vendor, hardware.model].filter((part): part is string => !!part);
  return parts.length > 0 ? parts.join(" ") : null;
}

function cpuLine(hardware: Schemas["Hardware"]): string | null {
  const parts: string[] = [];
  if (hardware.cpu) parts.push(hardware.cpu);
  if (hardware.cores != null) parts.push(`${hardware.cores} ${hardware.cores === 1 ? "core" : "cores"}`);
  return parts.length > 0 ? parts.join(", ") : null;
}

export function hardware(hardware: Schemas["Hardware"]): Fact[] {
  const facts: Fact[] = [];
  const name = machine(hardware);
  if (name) facts.push(fact("hardware", name));
  if (hardware.board && hardware.firmware) {
    facts.push(fact("board", Line.plain(`${hardware.board} `).add("muted", `firmware ${hardware.firmware}`)));
  } else if (hardware.board) {
    facts.push(fact("board", hardware.board));
  } else if (hardware.firmware) {
    facts.push(fact("firmware", hardware.firmware));
  }
  const arch = Line.of("muted", hardware.arch);
  const cpu = cpuLine(hardware);
  facts.push(fact("cpu", cpu ? Line.plain(`${cpu} `).join(arch) : arch));
  if (hardware.serial) facts.push(fact("serial", Line.of("muted", hardware.serial)));
  return facts;
}

function onUntil(command: string, what: string): Line {
  return Line.of("warn", "on").text(" ").add("muted", "-").text(" ").add("cmd", command).text(" ").add("muted", what);
}

export interface StatusText {
  facts: Fact[];
  units: Fact[];
  more: Fact[];
  pending: Line | null;
}

export function status(status: Schemas["Status"]): StatusText {
  const facts = node(status.node);
  if (status.os) {
    facts.push(
      fact(
        "os",
        status.image_version
          ? Line.plain(`${status.os}, `).add("label", "image").text(` ${status.image_version}`)
          : Line.plain(status.os),
      ),
    );
  }
  if (status.hardware) facts.push(...hardware(status.hardware));
  facts.push(fact("revision", String(status.revision)));
  if (status.cpu_percent != null) {
    facts.push(fact("cpu use", Line.of(usageLevel(status.cpu_percent), `${status.cpu_percent}%`)));
  }
  if (status.memory) {
    facts.push(fact("memory", freeLine(status.memory.available, status.memory.total, memUsedPercent(status.memory))));
  }
  if (status.data) facts.push(fact("data", usageLine(status.data)));
  if (status.maintenance) {
    facts.push(fact("maintenance", onUntil("tessaro-ctl browser maintenance off", "returns to browser.url")));
  }
  if (status.debug_screen) {
    facts.push(fact("debug screen", onUntil("tessaro-ctl browser debug off", "returns to the page below")));
  }
  facts.push(fact("browser url", status.kiosk_url));
  facts.push(fact("showing", status.current_url ? Line.plain(status.current_url) : Line.of("warn", "(cannot tell)")));
  facts.push(fact("browser", status.browser_answering ? Line.of("ok", "answering") : Line.of("bad", "not answering")));
  if (status.devtools) {
    facts.push(
      fact(
        "devtools",
        Line.of("warn", "connected").text(" ").add("muted", "- the agent leaves the tab alone until it disconnects"),
      ),
    );
  }

  const units = Object.entries(status.units)
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .map(([unit, state]) => fact(unit, Line.of(unitState(state), state)));

  const more: Fact[] = [];
  if (status.audio) more.push(fact("audio", audio.summary(status.audio)));
  if (status.time) more.push(fact("time", time.summary(status.time)));
  if (status.screen_on === false) {
    more.push(
      fact(
        "screen",
        Line.of("warn", "off").text(" ").add("muted", "-").text(" ").add("cmd", "tessaro-ctl screen power on"),
      ),
    );
  }
  if (status.bridge) {
    if (status.bridge.mode !== "off") more.push(fact("page bridge", status.bridge.mode));
    if (status.bridge.script) {
      const state = status.bridge.script_problem
        ? Line.of("bad", `not injected: ${status.bridge.script_problem}`)
        : Line.of("ok", "injected");
      more.push(fact("inject", Line.plain(`${status.bridge.script} `).join(state)));
    }
  }

  const pending = status.pending
    ? Line.of("warn", "on probation")
        .text(` ${status.pending.key}=${status.pending.value} - `)
        .add("cmd", `\`${CONFIRM_COMMAND}\``)
        .text(
          ` within ${status.pending.seconds_left}s or it goes back to ${previousOrDefault(status.pending.previous)}`,
        )
    : null;
  return { facts, units, more, pending };
}

/** What a `config set` or `unset` did. */
export function applied(applied: Schemas["Applied"], noApply: boolean): Line[] {
  if (applied.changed.length === 0) {
    return [Line.of("muted", `nothing changed (revision ${applied.revision})`)];
  }
  const lines = [Line.of("muted", `revision ${applied.revision}:`).text(" ").add("ok", applied.changed.join(", "))];
  if (applied.audio) lines.push(...audio.outcome(applied.audio));
  if (applied.time) lines.push(time.outcome(applied.time));
  if (noApply) {
    lines.push(Line.of("muted", "saved; nothing restarted"));
  } else if (applied.restarted.length === 0) {
    if (!applied.audio && !applied.time && !applied.reboot) {
      lines.push(Line.of("muted", "nothing to restart"));
    }
  } else {
    lines.push(Line.of("warn", `restarting ${applied.restarted.join(", ")}`));
  }
  if (applied.reboot) {
    lines.push(Line.of("warn", "takes effect at the next reboot:").text(" ").add("cmd", REBOOT_COMMAND));
  }
  if (applied.pending) {
    const pending = applied.pending;
    lines.push(new Line());
    lines.push(Line.of("warn", `${pending.key}=${pending.value} is on probation.`).text(" Check the screen, then run"));
    lines.push(new Line());
    lines.push(Line.plain("    ").add("cmd", CONFIRM_COMMAND));
    lines.push(new Line());
    lines.push(
      Line.plain(
        `within ${pending.seconds_left}s, or it goes back to ${previousOrDefault(pending.previous)} on its own.`,
      ),
    );
  }
  return lines;
}

export function restarts(consumer: Schemas["Consumer"]): string {
  switch (consumer) {
    case "agent":
      return "nothing: the agent applies it at once";
    case "agent-restart":
      return "the agent (it loads the page again)";
    case "browser":
      return "the browser";
    case "weston":
      return "the display (Weston and the browser)";
    case "network":
      return "nothing: the network profiles are switched, and checked before it is saved";
    case "audio":
      return "nothing: applied to the sound server at once";
    case "firmware":
      return "nothing: the Pi firmware reads it at the next reboot";
    case "time":
      return "nothing on screen: applied to the clock at once; systemd-timesyncd when its servers change";
    case "proxy":
      return "the local proxy (tessaro-proxy.service); the browser and the agent when the proxy is switched on or off";
    case "camera":
      return "the camera mirrors (tessaro-camera@*.service); a page showing a camera asks for it again";
  }
}

export function restartsShort(consumer: Schemas["Consumer"]): string {
  switch (consumer) {
    case "agent":
      return "agent (live)";
    case "agent-restart":
      return "agent";
    case "browser":
      return "browser";
    case "weston":
      return "weston";
    case "network":
      return "network";
    case "audio":
      return "audio";
    case "firmware":
      return "firmware (next reboot)";
    case "time":
      return "clock";
    case "proxy":
      return "local proxy (browser, agent on switching)";
    case "camera":
      return "camera mirrors";
  }
}

/** What `browser eval` came to; `thrown` for an exception. */
export function evalResult(result: Schemas["EvalResult"]): { line: Line; thrown: boolean } {
  if (result.exception) {
    return {
      line: Line.of("bad", result.exception.text)
        .text(" ")
        .add("muted", `(line ${result.exception.line}, column ${result.exception.column})`),
      thrown: true,
    };
  }
  const value = result.value;
  if (typeof value === "string") return { line: Line.plain(value), thrown: false };
  if (
    value &&
    typeof value === "object" &&
    !Array.isArray(value) &&
    Object.keys(value).length === 0 &&
    result.kind !== "object"
  ) {
    return { line: Line.of("muted", `(${result.kind})`), thrown: false };
  }
  if (value !== undefined && value !== null) return { line: Line.plain(JSON.stringify(value, null, 2)), thrown: false };
  if (result.description) {
    return { line: Line.plain(`${result.description} `).add("muted", `(${result.kind})`), thrown: false };
  }
  return { line: Line.of("muted", result.kind), thrown: false };
}
