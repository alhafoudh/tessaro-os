// agent/client/src/describe/screen.rs: what the Screen page says about the
// connected displays (their EDID), the TV over HDMI-CEC and the VNC mirror.

import type { Schemas } from "../api/client";
import { Line } from "../text/line";

type CecPower = Schemas["CecPower"];

const LABEL = 9;

function indent(): Line {
  return Line.plain("    ");
}

function field(label: string): Line {
  return indent().pad("label", label, LABEL).text(" ");
}

function none(): Line {
  return Line.of("muted", "(none)");
}

/**
 * Who made a display and what it calls itself: `Samsung SAMSUNG`, `DEL
 * DELL U2720Q` for a maker the client does not know by name.
 */
export function displayName(display: Schemas["DisplayIdentity"]): string {
  const maker = display.vendor ?? display.vendor_id;
  return display.model != null ? `${maker} ${display.model}` : maker;
}

/** When it was made: `2023, week 12`. */
function made(display: Schemas["DisplayIdentity"]): string | null {
  if (display.year == null) return null;
  return display.week != null ? `${display.year}, week ${display.week}` : `${display.year}`;
}

/**
 * `HDMI 2` for a physical address `2.0.0.0`: the TV's own HDMI input the
 * device is plugged into, when it is plugged straight into the TV.
 */
function input(address: string): Line {
  const [first = "0", ...rest] = address.split(".");
  const direct = rest.every((part) => part === "0");
  if (direct && first !== "0") {
    return Line.plain(`HDMI ${first}`).text(" ").add("muted", `(physical address ${address})`);
  }
  return Line.plain(`physical address ${address}`);
}

/** One connector: what is plugged into it and what it can show. */
export function connector(connector: Schemas["Connector"]): Line[] {
  const lines = [Line.of("heading", connector.name)];
  const display = connector.display ?? null;
  if (display !== null) {
    lines.push(
      field("display")
        .text(displayName(display))
        .text(" ")
        .add("muted", `(${display.vendor_id} ${display.product_code.toString(16).padStart(4, "0")})`),
    );
    lines.push(field("serial").join(display.serial != null ? Line.plain(display.serial) : none()));
    const when = made(display);
    if (when !== null) lines.push(field("made").text(when));
    if (display.width_cm != null && display.height_cm != null) {
      lines.push(field("size").text(`${display.width_cm} x ${display.height_cm} cm`));
    }
    if (display.hdmi_address != null) lines.push(field("input").join(input(display.hdmi_address)));
  } else {
    lines.push(field("display").add("muted", "(it does not say what it is)"));
  }
  const preferred = connector.modes[0];
  lines.push(
    field("modes").join(
      preferred !== undefined
        ? Line.plain(`${connector.modes.length}, preferred ${preferred}`)
            .text(" ")
            .add("muted", "- tessaro-ctl screen modes lists them")
        : none(),
    ),
  );
  return lines;
}

/** A power state in words: `turning on` for `turning-on`. */
function powerName(power: CecPower): string {
  return power.replace("-", " ");
}

/** What a device's power reads as. */
export function power(power: CecPower): Line {
  const tone = power === "on" ? "ok" : power === "standby" ? "muted" : "warn";
  return Line.of(tone, powerName(power));
}

/** The TV's power and whether it shows the device: `on, showing this device`. */
function tvLine(state: CecPower | null, showing: boolean): Line {
  let line = state !== null ? power(state) : Line.of("muted", "(not answered yet)");
  if (state === "on") {
    line = showing ? line.text(", showing this device") : line.text(", ").add("warn", "showing another input");
  }
  return line;
}

/** The VNC mirror in one line: `mirroring, 1 viewer`, or `off`. */
export function vnc(status: Schemas["VncStatus"]): Line {
  if (!status.sharing) {
    return Line.of("muted", "off")
      .text(" ")
      .add("muted", "- `tessaro-ctl screen vnc` mirrors the screen while its tunnel is open");
  }
  const viewers = status.viewers === 0 ? "no viewer" : status.viewers === 1 ? "1 viewer" : `${status.viewers} viewers`;
  return Line.of("warn", "mirroring").text(`, ${viewers}`);
}

/** The TV in `device status`: `on, showing this device (Samsung TV)`. */
export function tv(status: Schemas["TvStatus"]): Line {
  const line = tvLine(status.power ?? null, status.showing);
  return status.name != null ? line.text(" ").add("muted", `(${status.name})`) : line;
}

/**
 * One device on the bus, in columns: address, kind, name, maker, physical
 * address, power.
 */
function busDevice(device: Schemas["CecDevice"]): Line {
  let line = new Line()
    .pad("plain", `${device.address}`, 3)
    .pad("label", device.kind, 10)
    .pad("plain", device.name ?? "-", 15)
    .pad("plain", device.vendor ?? "-", 14)
    .pad("muted", device.physical ?? "-", 9);
  if (device.power != null) line = line.join(power(device.power));
  return line;
}

function addressLine(logical: number | null, physical: string | null): Line {
  if (logical !== null && physical !== null) return Line.plain(`logical ${logical}, physical ${physical}`);
  const why =
    logical === null && physical === null
      ? "- the TV gives none: off, or dropping hot-plug in standby"
      : `- logical ${logical !== null ? `${logical}` : "none"}, physical ${physical ?? "none"}`;
  return Line.of("warn", "none yet").text(" ").add("muted", why);
}

/**
 * One adapter: where it is, what the device is on its bus, the TV and
 * everything else that answered.
 */
export function adapter(adapter: Schemas["CecAdapter"]): Line[] {
  let heading = Line.of("heading", "HDMI-CEC").text(" ").add("muted", adapter.device);
  if (adapter.connector != null) heading = heading.add("muted", ` on ${adapter.connector}`);
  const lines = [heading];
  if (adapter.problem != null) lines.push(field("problem").add("bad", adapter.problem));
  lines.push(field("name").text(adapter.name));
  lines.push(field("address").join(addressLine(adapter.address ?? null, adapter.physical ?? null)));
  lines.push(field("tv").join(tvLine(adapter.tv ?? null, adapter.active)));
  const devices = adapter.devices ?? [];
  if (devices.length === 0) {
    lines.push(field("bus").join(none()));
  } else {
    devices.forEach((device, at) => {
      lines.push(field(at === 0 ? "bus" : "").join(busDevice(device)));
    });
  }
  return lines;
}

/**
 * `screen show`: every connected display, then the HDMI-CEC bus, or how to
 * switch it on.
 */
export function show(show: Schemas["ScreenShow"]): Line[] {
  const lines: Line[] = [];
  const adapters = show.adapters ?? [];
  if (show.connectors.length === 0) lines.push(Line.of("warn", "no display is connected"));
  for (const one of show.connectors) {
    lines.push(...connector(one));
    lines.push(new Line());
  }
  if (!show.cec) {
    lines.push(
      Line.of("heading", "HDMI-CEC")
        .text(" ")
        .add("muted", "off -")
        .text(" ")
        .add("cmd", "tessaro-ctl config set screen.cec.enable=1")
        .text(" ")
        .add("muted", "talks to the TV"),
    );
  } else if (adapters.length === 0) {
    lines.push(
      Line.of("heading", "HDMI-CEC")
        .text(" ")
        .add("warn", "on, but no adapter on a display connector")
        .text(" ")
        .add("muted", "- this hardware has no CEC, or the TV is not plugged in"),
    );
  }
  adapters.forEach((one, at) => {
    if (at > 0) lines.push(new Line());
    lines.push(...adapter(one));
  });
  return lines;
}
