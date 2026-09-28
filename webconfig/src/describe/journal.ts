// agent/client/src/journal.rs: one journal entry, read the same way by
// every client.

export interface Entry {
  /** Microseconds since the Unix epoch, as a string: past 2^53. */
  time: string | null;
  /** `SYSLOG_IDENTIFIER`, else the unit, else `?`. */
  source: string;
  /** syslog priority: 0 is emerg, 3 err, 4 warning, 7 debug. */
  priority: number | null;
  message: string;
}

function field(event: Record<string, unknown>, name: string): string | null {
  const value = event[name];
  return typeof value === "string" ? value : null;
}

export function parse(event: Record<string, unknown>): Entry {
  const raw = event.MESSAGE;
  let message: string;
  if (typeof raw === "string") {
    message = raw;
  } else if (Array.isArray(raw)) {
    // Non-UTF-8 messages come as a byte array.
    const bytes = new Uint8Array(raw.filter((byte): byte is number => typeof byte === "number"));
    message = new TextDecoder().decode(bytes);
  } else if (raw !== undefined) {
    message = JSON.stringify(raw);
  } else {
    message = JSON.stringify(event);
  }
  const time = field(event, "__REALTIME_TIMESTAMP");
  const priority = field(event, "PRIORITY");
  const parsed = priority !== null && /^\d+$/.test(priority) ? Number(priority) : null;
  return {
    time: time !== null && /^\d+$/.test(time) ? time : null,
    source: field(event, "SYSLOG_IDENTIFIER") ?? field(event, "_SYSTEMD_UNIT") ?? "?",
    priority: parsed !== null && parsed <= 255 ? parsed : null,
    message,
  };
}

/** `HH:MM:SS`, UTC: the device and the browser need not share a zone. */
export function clock(entry: Entry): string {
  if (entry.time === null) return "";
  const seconds = Number((BigInt(entry.time) / 1_000_000n) % 86_400n);
  const two = (value: number) => String(value).padStart(2, "0");
  return `${two(Math.floor(seconds / 3600))}:${two(Math.floor(seconds / 60) % 60)}:${two(seconds % 60)}`;
}
