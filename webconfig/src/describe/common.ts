// Small helpers shared by the describe ports: protocol's size_label and
// the used-percent rules, client/src/storage.rs's free_line.

import type { Schemas } from "../api/client";
import { Line, usageLevel } from "../text/line";

/**
 * `value` with `digits` decimals, as Rust's `{:.N}` writes it. Both round
 * the exact decimal value of the double, but a tie - 1.25 to one decimal -
 * goes to the even digit in Rust (1.2) and up in `toFixed` (1.3).
 */
export function fixed(value: number, digits: number): string {
  const exact = value.toFixed(Math.min(100, digits + 40));
  const point = exact.indexOf(".");
  const rest = exact.slice(point + 1 + digits);
  if (!/^50*$/.test(rest)) {
    return value.toFixed(digits);
  }
  // An exact tie: drop the 5, and round up only to make the last digit even.
  const kept = exact.slice(0, point + 1 + digits).replace(/\.$/, "");
  const last = Number(kept.replace(".", "").slice(-1));
  if (last % 2 === 0) {
    return kept;
  }
  const step = 10 ** -digits;
  const away = Math.abs(Number(kept)) + step;
  return `${value < 0 ? "-" : ""}${away.toFixed(digits)}`;
}

/** protocol::size_label: `1.2 GB`, decimal units. */
export function sizeLabel(bytes: number): string {
  if (bytes >= 1e12) return `${fixed(bytes / 1e12, 1)} TB`;
  if (bytes >= 1e9) return `${fixed(bytes / 1e9, 1)} GB`;
  if (bytes >= 1e6) return `${fixed(bytes / 1e6, 1)} MB`;
  if (bytes >= 1e3) return `${fixed(bytes / 1e3, 1)} kB`;
  return `${bytes} B`;
}

/** FsUsage::used_percent, rounded up like df. */
export function fsUsedPercent(fs: Schemas["FsUsage"]): number {
  const seen = fs.used + fs.available;
  return seen === 0 ? 0 : Math.ceil((fs.used * 100) / seen);
}

/** MemUsage::used_percent. */
export function memUsedPercent(memory: Schemas["MemUsage"]): number {
  if (memory.total === 0) return 0;
  const used = Math.max(0, memory.total - memory.available);
  return Math.ceil((used * 100) / memory.total);
}

/** storage::free_line: `1.2 GB free of 4.0 GB (70% used)`. */
export function freeLine(available: number, size: number, percent: number): Line {
  return Line.plain(`${sizeLabel(available)} free of ${sizeLabel(size)} `).add(
    usageLevel(percent),
    `(${percent}% used)`,
  );
}

export function usageLine(fs: Schemas["FsUsage"]): Line {
  return freeLine(fs.available, fs.size, fsUsedPercent(fs));
}

/** protocol::previous_or_default. */
export function previousOrDefault(previous: string | null | undefined): string {
  return previous ?? "the default";
}
