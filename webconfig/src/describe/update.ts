// agent/client/src/update.rs: what an update will lose, and where one is.

import type { Schemas } from "../api/client";
import { Line } from "../text/line";
import { mb, percent } from "./transfer";

/** update::COME_BACK: how long a device may take to come back, seconds. */
export const COME_BACK = 20 * 60;

/** Plan::warning: what is lost for good, as the end of "This will ...". */
export function warning(name: string, wipeData: boolean, repartition: boolean): string | null {
  if (repartition) {
    return (
      `rewrite the whole disk with ${name} (partition table, boot, root and /data: ` +
      "every setting, the claim and the identity go, and a power cut while it " +
      "writes needs a physical reflash)"
    );
  }
  if (wipeData) {
    return (
      `write ${name} and erase /data - every setting, the claim, the browser ` + "profile and the device's identity"
    );
  }
  return null;
}

/** update::status_lines: where an update is, and how the last one went. */
export function statusLines(status: Schemas["UpdateStatus"]): Line[] {
  const name = () => Line.of("heading", status.name ?? "");
  let first: Line;
  switch (status.phase) {
    case "idle":
      first = Line.of("muted", "no update under way");
      break;
    case "receiving":
      first = Line.of("warn", "receiving")
        .text(" ")
        .join(name())
        .text(`: ${mb(status.received)} of ${mb(status.size)} (${percent(status.received, status.size)}%) - run `)
        .add("cmd", "`update send`")
        .text(" again to resume");
      break;
    case "verifying":
      first = Line.of("warn", "verifying")
        .text(" ")
        .join(name())
        .text(`: ${mb(status.verified)} of ${mb(status.size)} (${percent(status.verified, status.size)}%)`);
      break;
    case "preparing":
      first = Line.of("warn", "preparing")
        .text(" ")
        .join(name())
        .text(
          `: ${mb(status.prepared)} of ${mb(status.to_prepare)} checked (${percent(status.prepared, status.to_prepare)}%)`,
        );
      break;
    case "ready":
      first = name().text(" ").add("ok", "is staged, not committed");
      break;
    case "pending": {
      const line = name().text(" ").add("ok", "is applied at the next boot");
      first = status.repartition
        ? line.add("warn", ", rewriting the whole disk")
        : status.wipe_data
          ? line.add("warn", ", and /data is wiped")
          : line;
      break;
    }
    case "failed":
      first = Line.of("bad", "failed:").text(` ${status.error ?? "no reason given"}`);
      break;
  }
  const lines = [first];
  if (status.last) {
    let line = new Line().pad("label", "last update", 12).text(` ${status.last.message} (`);
    line = status.last.applied ? line.add("ok", "applied") : line.add("bad", "not applied");
    if (status.last.wiped_data) {
      line = line.add("warn", ", /data wiped");
    }
    lines.push(line.text(")"));
  }
  return lines;
}

/** transfer::bmap_for: the block map's name beside the image's. */
export function bmapFor(image: string): string {
  const base = image.endsWith(".zst") ? image.slice(0, -4) : image.endsWith(".bz2") ? image.slice(0, -4) : image;
  return `${base}.bmap`;
}
