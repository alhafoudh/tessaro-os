// agent/client/src/describe/camera.rs, line for line.

import type { Schemas } from "../api/client";
import { Line } from "../text/line";

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
