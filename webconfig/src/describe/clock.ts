// agent/client/src/clock.rs: the words for what a device's clock reports.
// Formatting only: every number is the device's.

import { fixed } from "./common";

/** Microseconds in the largest unit that keeps them readable. */
export function span(usec: number): string {
  if (usec < 1_000) return `${usec}µs`;
  if (usec < 1_000_000) return `${fixed(usec / 1_000, 1)}ms`;
  if (usec < 60_000_000) return `${fixed(usec / 1_000_000, 2)}s`;
  const seconds = Math.floor(usec / 1_000_000);
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor(seconds / 60) % 60;
  const rest = seconds % 60;
  if (hours === 0 && rest === 0) return `${minutes}min`;
  if (hours === 0) return `${minutes}min ${rest}s`;
  return `${hours}h ${minutes}min`;
}

/** A clock's offset from its NTP server, signed: `+1.2ms` is behind it. */
export function offset(usec: number): string {
  return `${usec < 0 ? "-" : "+"}${span(Math.abs(usec))}`;
}

/** How far the hardware clock is from the system clock. */
export function offsetBetween(rtc: number, now: number): string {
  return rtc >= now ? `${span(rtc - now)} ahead` : `${span(now - rtc)} behind`;
}

/** Seconds east of UTC as `UTC+02:00`. */
export function utcOffset(seconds: number): string {
  const minutes = Math.floor(Math.abs(seconds) / 60);
  const two = (value: number) => String(value).padStart(2, "0");
  return `UTC${seconds < 0 ? "-" : "+"}${two(Math.floor(minutes / 60))}:${two(minutes % 60)}`;
}

/** NTP's precision, a power of two seconds: `-25` as `2^-25, 30ns`. */
export function precision(log2: number): string {
  const nanos = 2 ** log2 * 1e9;
  const readable = nanos < 1_000 ? `${fixed(nanos, 0)}ns` : span(Math.round(nanos / 1_000));
  return `2^${log2}, ${readable}`;
}

/** NTP's leap indicator. */
export function leap(code: number): string {
  switch (code) {
    case 0:
      return "none";
    case 1:
      return "a leap second is added at the end of the day";
    case 2:
      return "a leap second is removed at the end of the day";
    default:
      return "the server says it is not synchronized";
  }
}

/** timesyncd's frequency correction as drift: `-12.345 ppm`. */
export function drift(ppm: number): string {
  const text = fixed(ppm, 3);
  return `${text.startsWith("-") ? text : `+${text}`} ppm`;
}
