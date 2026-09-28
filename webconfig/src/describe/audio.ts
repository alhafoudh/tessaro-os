// agent/client/src/describe/audio.rs, the parts the web pages use.

import type { Schemas } from "../api/client";
import { Line } from "../text/line";
import { fixed } from "./common";

/** What a change of the audio.* keys did. */
export function outcome(text: string): Line[] {
  if (text.startsWith("saved,")) {
    return [Line.of("warn", text)];
  }
  return text.split("; ").map((side) => Line.of("ok", side));
}

/** `muted`, `off`, or the volume. */
export function level(side: Schemas["AudioSide"]): Line {
  if (side.setting === "off") return Line.of("warn", "off");
  if (side.muted) return Line.of("warn", "muted");
  return Line.plain(`${side.volume}%`);
}

/** `hdmi 80%`, and why when it is not what the setting says. */
export function summary(status: Schemas["AudioStatus"]): Line {
  if (!status.running) {
    return Line.of("bad", "sound server not answering");
  }
  const output = status.output;
  const line = (output.using ? Line.plain(output.using.kind) : Line.of("warn", "no output"))
    .text(" ")
    .join(level(output));
  return output.fallback ? line.text(" ").add("muted", `(${output.fallback})`) : line;
}

/** `audio show`: each side's setting, what it resolved to, and why. */
export function show(status: Schemas["AudioStatus"]): Line[] {
  const lines: Line[] = [];
  if (status.error) {
    lines.push(Line.of("bad", "the sound server is not answering:").text(` ${status.error}`));
    lines.push(new Line());
  }
  const sides: [string, Schemas["AudioSide"]][] = [
    ["output", status.output],
    ["input", status.input],
  ];
  for (const [label, side] of sides) {
    const using = side.using
      ? Line.of("heading", side.using.description).text(" ").add("muted", `(${side.using.kind})`)
      : status.running
        ? Line.of("warn", "(none)")
        : Line.of("muted", "(unknown)");
    lines.push(
      new Line()
        .pad("label", label, 8)
        .text(` ${side.setting} `)
        .add("muted", "->")
        .text(" ")
        .join(using)
        .text("  ")
        .join(level(side)),
    );
    if (side.fallback) {
      lines.push(new Line().pad("label", "", 8).text(" ").add("warn", side.fallback));
    }
  }
  lines.push(new Line());
  lines.push(Line.of("muted", "`tessaro-ctl audio outputs` and `audio inputs` list what is plugged in."));
  return lines;
}

/** Why a device may not do what its name says. */
export function note(device: Schemas["AudioDevice"]): Line {
  const notes: string[] = [];
  if (device.available === false) notes.push("nothing plugged in");
  if (device.needs_profile) notes.push("switches its sound card over");
  return notes.length === 0 ? new Line() : Line.plain("  ").add("muted", `(${notes.join(", ")})`);
}

/** What `audio test` did: the tone played, or how loud the recording was. */
export function test(tested: Schemas["AudioTested"]): Line[] {
  const lines = [Line.of("ok", tested.message)];
  if (tested.peak_dbfs != null && tested.rms_dbfs != null) {
    lines.push(
      Line.of("label", "peak")
        .text(` ${fixed(tested.peak_dbfs, 1)} dBFS  `)
        .add("label", "average")
        .text(` ${fixed(tested.rms_dbfs, 1)} dBFS`),
    );
  }
  if (tested.saved) {
    lines.push(Line.of("muted", "listen with").text(" ").add("cmd", `tessaro-ctl files download ${tested.saved}`));
  }
  return lines;
}
