// The time parts of agent/client/src/actions.rs: how `time ntp` and `time
// set` turn what was typed into a request, the way both clients do it.

import type { Schemas } from "../api/client";

export const NTP_ENABLE = "time.ntp.enable";
export const NTP_SERVERS = "time.ntp.servers";

/**
 * The settings `time ntp on|off` changes: time.ntp.enable, and the servers
 * when some are given. Servers go with NTP on only.
 */
export function ntpChange(on: boolean, servers: string[]): Record<string, string> {
  const values: Record<string, string> = { [NTP_ENABLE]: on ? "1" : "0" };
  const given = servers.map((server) => server.trim()).filter((server) => server.length > 0);
  if (given.length > 0) {
    if (!on) throw new Error("NTP servers go with NTP on");
    values[NTP_SERVERS] = given.join(",");
  }
  return values;
}

/** protocol::parse_local_time: `YYYY-MM-DD HH:MM[:SS]`. */
export function parseLocalTime(value: string): number[] {
  const bad = () => new Error(`${value} is not a time; write it as YYYY-MM-DD HH:MM[:SS]`);
  const text = value.trim();
  const at = text.search(/[ T]/);
  if (at < 0) throw bad();
  const date = text.slice(0, at).split("-");
  const time = text
    .slice(at + 1)
    .trim()
    .split(":");
  if (date.length !== 3 || time.length < 2 || time.length > 3) throw bad();
  const number = (part: string) => {
    if (!/^[+-]?\d+$/.test(part)) throw bad();
    return Number(part);
  };
  const [year, month, day] = date.map(number) as [number, number, number];
  const [hour, minute] = [number(time[0]!), number(time[1]!)];
  const second = time.length === 3 ? number(time[2]!) : 0;
  const ok =
    year >= 2000 &&
    year <= 2200 &&
    month >= 1 &&
    month <= 12 &&
    day >= 1 &&
    day <= 31 &&
    hour >= 0 &&
    hour <= 23 &&
    minute >= 0 &&
    minute <= 59 &&
    second >= 0 &&
    second <= 60;
  if (!ok) throw bad();
  return [year, month, day, hour, minute, second];
}

/** The body of `time set`: `local` in the device's timezone, or this computer's clock. */
export function setClock(local: string | null): Schemas["TimeSetBody"] {
  const typed = local?.trim();
  if (typed) {
    parseLocalTime(typed);
    return { usec: null, local: typed };
  }
  return { usec: Date.now() * 1000, local: null };
}
