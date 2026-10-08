// agent/client/src/describe/camera.rs, line for line.

import type { Schemas } from "../api/client";
import { Line } from "../text/line";
import { fixed } from "./common";

/** protocol::keys::CAMERA_MIRRORS_MAX: the most virtual cameras one camera gets. */
export const CAMERA_MIRRORS_MAX = 8;

/** `mjpeg 1280x720 @ 30 fps`. */
export function mode(mode: Schemas["CameraMode"]): string {
  return `${mode.format} ${mode.width}x${mode.height} @ ${mode.fps} fps`;
}

/** What a device without a camera shows, and how one appears. */
export function none(): Line {
  return Line.of("warn", "no cameras;").text(" ").add("muted", "plug in a USB camera and it shows here");
}

/**
 * `camera list`: a block per camera - what it is called, its node and each virtual camera readers
 * open, what its mirror captures and why that is not what the settings say, every mode it has -
 * then the saved settings.
 */
export function list(list: Schemas["CameraList"]): Line[] {
  const lines: Line[] = [];
  if (list.cameras.length === 0) {
    lines.push(none());
  }
  for (const camera of list.cameras) {
    lines.push(...one(camera));
  }
  lines.push(new Line());
  lines.push(
    new Line()
      .pad("label", "saved", 9)
      .text(" ")
      .add("label", "format")
      .text(` ${list.format}  `)
      .add("label", "size")
      .text(` ${list.size}  `)
      .add("label", "mirrors")
      .text(` ${list.mirrors}`),
  );
  lines.push(
    Line.of("muted", "change them with")
      .text(" ")
      .add("cmd", "tessaro-ctl camera format auto|mjpeg|yuyv")
      .add("muted", ",")
      .text(" ")
      .add("cmd", "tessaro-ctl camera size auto|WIDTHxHEIGHT")
      .text(" ")
      .add("muted", "and")
      .text(" ")
      .add("cmd", `tessaro-ctl camera mirrors 1-${CAMERA_MIRRORS_MAX}`),
  );
  return lines;
}

function one(camera: Schemas["CameraInfo"]): Line[] {
  const indent = () => Line.plain("    ");
  const lines = [Line.of("heading", camera.name).text(" ").add("muted", `(${camera.bus})`)];
  lines.push(indent().pad("label", "device", 9).text(` /dev/${camera.device}`));
  const mirrors = camera.mirrors ?? [];
  if (mirrors.length === 0) {
    lines.push(indent().pad("label", "mirror", 9).text(" ").add("muted", "(no virtual camera)"));
  }
  const width = Math.max(0, ...mirrors.map((mirror) => mirror.device.length));
  for (const mirror of mirrors) {
    lines.push(indent().pad("label", "mirror", 9).text(" ").pad("ok", mirror.device, width).text(`  ${mirror.name}`));
  }
  if (camera.mode) {
    lines.push(
      indent()
        .pad("label", "captures", 9)
        .text(` ${mode(camera.mode)}`),
    );
  }
  if (camera.fallback) {
    lines.push(indent().add("warn", camera.fallback));
  }
  if (camera.error) {
    lines.push(indent().add("bad", camera.error));
  }
  const modes = camera.modes ?? [];
  const formats: string[] = [];
  for (const each of modes) {
    if (!formats.includes(each.format)) {
      formats.push(each.format);
    }
  }
  for (const format of formats) {
    const sizes = modes
      .filter((each) => each.format === format)
      .map((each) => `${each.width}x${each.height}@${each.fps}`);
    lines.push(indent().pad("label", format, 9).text(" ").add("muted", sizes.join(" ")));
  }
  return lines;
}

/**
 * `camera presence`: whether anyone is there and who, how the detection runs, the last event, then
 * the saved distances and how to change them.
 */
export function presence(status: Schemas["PresenceStatus"]): Line[] {
  if (!status.enabled) {
    return [
      Line.of("muted", "presence detection is off;")
        .text(" ")
        .add("muted", "switch it on with")
        .text(" ")
        .add("cmd", "tessaro-ctl camera presence on"),
    ];
  }
  let state = new Line().pad("label", "presence", 9).text(" ");
  if (!status.running) {
    state = state.add("warn", "not running");
  } else if (status.present && status.near) {
    state = state.add("ok", "someone is there, near");
  } else if (status.present) {
    state = state.add("ok", "someone is there");
  } else {
    state = state.add("muted", "nobody is there");
  }
  const lines = [state];
  for (const face of status.frame?.faces ?? []) {
    lines.push(faceLine(face));
  }
  let camera = new Line()
    .pad("label", "camera", 9)
    .text(` ${status.camera ?? "(none)"}  `)
    .add("label", "model")
    .text(` ${status.model}`);
  if (status.fps != null) {
    camera = camera.add("muted", `  ${fixed(status.fps, 1)} fps`);
  }
  if (status.inference_ms != null) {
    camera = camera.add("muted", `  ${fixed(status.inference_ms, 0)} ms a frame`);
  }
  lines.push(camera);
  if (status.demographics) {
    const genders = { male: 0, female: 0, unknown: 0 };
    for (const face of status.frame?.faces ?? []) {
      if (face.demographics) genders[face.demographics.gender] += 1;
    }
    let estimate = new Line()
      .pad("label", "estimate", 9)
      .text(" age and gender  ")
      .add("plain", `${genders.male} male, ${genders.female} female, ${genders.unknown} unknown`);
    if (status.classify_ms != null) {
      estimate = estimate.add("muted", `  ${fixed(status.classify_ms, 0)} ms a look`);
    }
    lines.push(estimate);
  }
  if (status.error) {
    lines.push(Line.plain("          ").add("bad", status.error));
  }
  if (status.last) {
    lines.push(new Line().pad("label", "last", 9).text(` ${status.last.event} at ${status.last.at.local}`));
  }
  const near = status.near_m != null ? ` ${fixed(status.near_m, 1)} m  ` : " off  ";
  lines.push(new Line());
  lines.push(
    new Line()
      .pad("label", "saved", 9)
      .text(" ")
      .add("label", "near")
      .text(near)
      .add("label", "fov")
      .text(` ${fixed(status.fov, 0)}°`),
  );
  lines.push(
    Line.of("muted", "change them with")
      .text(" ")
      .add("cmd", "tessaro-ctl config set camera.presence.near=METERS")
      .add("muted", ",")
      .text(" ")
      .add("cmd", "tessaro-ctl camera calibrate --distance 1"),
  );
  return lines;
}

/** One face: `face     #3 1.2 m near facing (0.93)`, then `female, about 34` once settled. */
function faceLine(face: Schemas["Face"]): Line {
  let line = Line.plain("    ")
    .pad("label", "face", 5)
    .text(` #${face.id} ${fixed(face.distance, 1)} m`);
  if (face.near) {
    line = line.text(" ").add("ok", "near");
  }
  line = line
    .text(" ")
    .add("plain", face.facing ? "facing" : "turned away")
    .add("muted", ` (${fixed(face.score, 2)})`);
  if (face.demographics) {
    line = line.text("  ").add("plain", estimate(face.demographics));
  }
  return line;
}

/**
 * A face's settled age and gender: `female, about 34`, `gender unknown, about 52`. The camera
 * panel labels its face boxes with it too.
 */
export function estimate(estimate: Schemas["Demographics"]): string {
  const gender = estimate.gender === "unknown" ? "gender unknown" : estimate.gender;
  return `${gender}, about ${estimate.age}`;
}

/** What `camera calibrate` saved. */
export function calibrated(calibrated: Schemas["Calibrated"]): Line {
  return Line.of("ok", `camera.presence.fov is ${fixed(calibrated.fov, 0)}°`).add(
    "muted",
    ` (a face ${fixed(calibrated.width, 3)} of the frame wide at ${fixed(calibrated.distance, 1)} m)`,
  );
}
