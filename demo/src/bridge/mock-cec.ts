// The pretend device's HDMI-CEC bus: a TV at address 0 and a sound bar at 5,
// the device itself at 4. It answers tessaro.screen.cec.* like the agent
// (control/cec.rs, control/bridge.rs), refusals included, keeps a message
// log like cec::log, and fires every message as a `message` event.

import type { CecActed, CecActions, CecAdapterActed, CecMessage, CecMessageDetail, CecPower } from "./types";

const DEVICE = "/dev/cec0";
const CONNECTOR = "HDMI-A-1";
const US = 4;
const TV = 0;
const AUDIO = 5;
const BROADCAST = 15;
/** cec::log::KEPT */
const KEPT = 500;
/** CEC_BURST in CEC_WINDOW, control/bridge.rs */
const BURST = 20;
const WINDOW_MS = 10_000;
/** REPLY_WAIT */
const REPLY_WAIT_MS = 2000;
/** protocol::cec::DATA_MAX */
const DATA_MAX = 15;

/** The names of the opcodes the pretend bus uses (protocol::cec::OPCODES). */
const OPCODES: Record<number, string> = {
  0x00: "feature-abort",
  0x04: "image-view-on",
  0x0d: "text-view-on",
  0x36: "standby",
  0x44: "user-control-pressed",
  0x45: "user-control-released",
  0x46: "give-osd-name",
  0x47: "set-osd-name",
  0x71: "give-audio-status",
  0x7a: "report-audio-status",
  0x82: "active-source",
  0x83: "give-physical-address",
  0x84: "report-physical-address",
  0x85: "request-active-source",
  0x87: "device-vendor-id",
  0x8c: "give-device-vendor-id",
  0x8f: "give-device-power-status",
  0x90: "report-power-status",
  0x9d: "inactive-source",
  0x9e: "cec-version",
  0x9f: "get-cec-version",
};

/** Some of protocol::cec::KEYS, by name. */
const KEYS: Record<string, number> = {
  select: 0x00,
  up: 0x01,
  down: 0x02,
  left: 0x03,
  right: 0x04,
  exit: 0x0d,
  power: 0x40,
  "volume-up": 0x41,
  "volume-down": 0x42,
  mute: 0x43,
  play: 0x44,
  stop: 0x45,
  blue: 0x71,
  red: 0x72,
  green: 0x73,
  yellow: 0x74,
};

const POWER_CODE: Record<CecPower, number> = { on: 0, standby: 1, "turning-on": 2, "turning-off": 3 };

function hex(bytes: number[]): string {
  return bytes.map((byte) => byte.toString(16).padStart(2, "0")).join(" ");
}

/** The bytes typed as hex, as protocol::cec::parse_data takes them. */
export function parseData(typed: string): number[] {
  const digits = typed
    .split(/[\s:,]+/)
    .map((part) => part.replace(/^0x/i, ""))
    .map((part) => (part.length === 1 ? `0${part}` : part))
    .join("");
  if (!digits) throw new Error("no bytes to send; an opcode in hex, e.g. 8f");
  if (digits.length % 2 !== 0 || !/^[0-9a-fA-F]+$/.test(digits)) {
    throw new Error(`${JSON.stringify(typed.trim())} is not hex bytes, e.g. 44 41`);
  }
  const bytes: number[] = [];
  for (let at = 0; at < digits.length; at += 2) bytes.push(parseInt(digits.slice(at, at + 2), 16));
  if (bytes.length > DATA_MAX) {
    throw new Error(`a message carries at most ${DATA_MAX} bytes after its header, this has ${bytes.length}`);
  }
  return bytes;
}

function ascii(text: string): number[] {
  return [...text.slice(0, 14)].map((ch) => ch.charCodeAt(0) & 0x7f);
}

function wait(ms: number) {
  return new Promise((done) => setTimeout(done, ms));
}

export interface MockBus {
  actions: CecActions;
  receive(from: number, bytes: number[]): void;
  tv(): CecPower;
  showing(): boolean;
}

export function createCecBus(options: {
  enabled: () => boolean;
  pageEvents: () => boolean;
  screenOn: () => boolean;
}): MockBus {
  let tv: CecPower = "on";
  let showing = true;
  let volume = 22;
  let seq = 0;
  const log: CecMessage[] = [];
  const acts: number[] = [];
  let waiting: { from: number; opcode: number; done: (message: CecMessage) => void } | null = null;

  const emit = (direction: "in" | "out", from: number, to: number, bytes: number[], acked: boolean | null) => {
    const now = new Date();
    const message: CecMessage = {
      seq: ++seq,
      at_ms: now.getTime(),
      time: `${now.toLocaleTimeString("en-GB", { hour12: false })}.${String(now.getMilliseconds()).padStart(3, "0")}`,
      device: DEVICE,
      direction,
      from,
      to,
      data: hex(bytes),
      acked,
    };
    log.push(message);
    if (log.length > KEPT) log.splice(0, log.length - KEPT);
    if (options.pageEvents()) {
      const opcode = bytes.length ? bytes[0]! : null;
      const detail: CecMessageDetail = {
        event: "message",
        connector: CONNECTOR,
        seq: message.seq,
        direction,
        from,
        to,
        data: message.data,
        opcode,
        name: opcode === null ? null : (OPCODES[opcode] ?? null),
        acked,
      };
      window.dispatchEvent(new CustomEvent("tessaro:cec", { detail }));
    }
    if (direction === "in" && waiting && waiting.from === from && bytes[0] === waiting.opcode) {
      waiting.done(message);
      waiting = null;
    }
    return message;
  };

  /** A change event, as the agent fires them between known states. */
  const change = (event: "tv-on" | "tv-standby" | "source-gained" | "source-lost") => {
    if (!options.pageEvents()) return;
    window.dispatchEvent(new CustomEvent("tessaro:cec", { detail: { event, connector: CONNECTOR, tv, showing } }));
  };

  const setTv = (power: CecPower) => {
    const was = tv;
    tv = power;
    if (power === "on" && was !== "on") change("tv-on");
    if (power === "standby" && was !== "standby") change("tv-standby");
  };
  const setShowing = (now: boolean) => {
    if (now === showing) return;
    showing = now;
    change(now ? "source-gained" : "source-lost");
  };

  /** How the TV and the sound bar answer what the device sent them. */
  const answer = (to: number, bytes: number[]) => {
    const [opcode, ...operands] = bytes;
    if (opcode === undefined) return;
    const from = to;
    const reply = (target: number, data: number[]) => setTimeout(() => emit("in", from, target, data, null), 40);
    switch (opcode) {
      case 0x04:
      case 0x0d:
        if (to === TV && tv !== "on") {
          setTv("turning-on");
          setTimeout(() => setTv("on"), 1500);
        }
        return;
      case 0x36:
        if (to === TV || to === BROADCAST) setTv("standby");
        return;
      case 0x8f:
        reply(US, [0x90, POWER_CODE[to === TV ? tv : "on"]]);
        return;
      case 0x46:
        reply(US, [0x47, ...ascii(to === TV ? "Living room TV" : "Soundbar")]);
        return;
      case 0x83:
        reply(BROADCAST, to === TV ? [0x84, 0x00, 0x00, 0x00] : [0x84, 0x20, 0x00, 0x05]);
        return;
      case 0x8c:
        reply(BROADCAST, [0x87, 0x00, 0x00, 0xf0]);
        return;
      case 0x9f:
        reply(US, [0x9e, 0x05]);
        return;
      case 0x71:
        if (to === AUDIO) reply(US, [0x7a, volume]);
        else reply(US, [0x00, opcode, 0x00]);
        return;
      case 0x44: {
        const key = operands[0];
        if (to === AUDIO || to === TV) {
          if (key === 0x41) volume = Math.min(100, volume + 2);
          if (key === 0x42) volume = Math.max(0, volume - 2);
          if (key === 0x43) volume ^= 0x80;
          if (to === AUDIO && (key === 0x41 || key === 0x42 || key === 0x43)) reply(US, [0x7a, volume]);
        }
        return;
      }
      case 0x45:
      case 0x82:
      case 0x9d:
      case 0x85:
        return;
      default:
        if (to !== BROADCAST) reply(US, [0x00, opcode, 0x00]);
    }
  };

  const present = (to: number) => to === TV || to === AUDIO;

  /** Send from the device: logged, acknowledged by whoever is there. */
  const send = (to: number, bytes: number[]) => {
    const acked = to === BROADCAST ? false : present(to);
    emit("out", US, to, bytes, acked);
    if (acked || to === BROADCAST) answer(to, bytes);
    return { to, data: hex(bytes), acked };
  };

  const acted = (action: string, part: Partial<CecAdapterActed>): CecActed => ({
    action,
    adapters: [{ device: DEVICE, connector: CONNECTOR, sent: [], answered: [], tv, ...part }],
  });

  /** cec_allowed, then the checks of cec_act. */
  const check = () => {
    const now = Date.now();
    while (acts.length && now - acts[0]! > WINDOW_MS) acts.shift();
    if (acts.length >= BURST) {
      throw new Error(`refused: the page acted on the HDMI-CEC bus ${BURST} times in the last ${WINDOW_MS / 1000}s`);
    }
    acts.push(now);
    if (!options.enabled()) {
      throw new Error("HDMI-CEC is off; `tessaro-ctl config set screen.cec.enable=1` turns it on");
    }
  };

  const address = (to: number | undefined, fallback: number) => {
    const value = to ?? fallback;
    if (!Number.isInteger(value) || value < 0 || value > 15) throw new Error(`${value} is not a CEC address: 0 to 15`);
    return value;
  };

  const actions: CecActions = {
    wake: async (source = true) => {
      check();
      if (!options.screenOn()) throw new Error("the screen is off; `tessaro-ctl screen power on` wakes it and the TV");
      await wait(150);
      const sent = [send(TV, [0x04])];
      if (source) {
        sent.push(send(BROADCAST, [0x82, 0x10, 0x00]));
        setShowing(true);
      }
      return acted("wake", { sent });
    },
    standby: async (all = false) => {
      check();
      await wait(150);
      const sent = [send(all ? BROADCAST : TV, [0x36])];
      setShowing(false);
      return acted("standby", { sent });
    },
    source: async () => {
      check();
      await wait(120);
      const sent = [send(BROADCAST, [0x82, 0x10, 0x00])];
      setShowing(true);
      return acted("source", { sent });
    },
    key: async (name, to) => {
      check();
      const code = KEYS[name.trim().toLowerCase()];
      if (code === undefined) {
        throw new Error(
          `${JSON.stringify(name.trim().toLowerCase())} is not a remote key; one of ${Object.keys(KEYS).join(", ")}`,
        );
      }
      const target = address(to, TV);
      await wait(80);
      const sent = [send(target, [0x44, code]), send(target, [0x45])];
      return acted("key", { sent });
    },
    scan: async () => {
      check();
      const sent: CecAdapterActed["sent"] = [];
      const answered: number[] = [];
      for (let to = 0; to < 15; to += 1) {
        if (to === US) continue;
        await wait(25);
        // A scan logs only the polls that were answered.
        if (present(to)) {
          emit("out", US, to, [], true);
          answered.push(to);
          sent.push({ to, data: "", acked: true });
        } else {
          sent.push({ to, data: "", acked: false });
        }
      }
      return acted("scan", { sent, answered });
    },
    send: async (data, to, reply) => {
      check();
      const bytes = parseData(String(data ?? ""));
      const target = address(to, TV);
      const answer =
        reply === undefined || reply === null
          ? null
          : new Promise<CecMessage | null>((done) => {
              const timer = setTimeout(() => {
                waiting = null;
                done(null);
              }, REPLY_WAIT_MS);
              waiting = {
                from: target,
                opcode: reply,
                done: (message) => {
                  clearTimeout(timer);
                  done(message);
                },
              };
            });
      const sent = [send(target, bytes)];
      const got = answer ? await answer : null;
      return acted("send", { sent, reply: got });
    },
    messages: async (after = 0) => {
      const messages = log.filter((one) => one.seq > after);
      return { messages, next: seq };
    },
  };

  /** A message from the TV to the device, answered the way the agent does. */
  const receive = (from: number, bytes: number[]) => {
    emit("in", from, US, bytes, null);
    if (bytes[0] === 0x8f) emit("out", US, from, [0x90, 0x00], present(from));
    if (bytes[0] === 0x46) emit("out", US, from, [0x47, ...ascii("Tessaro")], present(from));
  };

  return { actions, receive, tv: () => tv, showing: () => showing };
}
