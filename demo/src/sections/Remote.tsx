// The TV over HDMI-CEC: whether it is on and showing this device, and its
// remote. Every key comes to the page as a tessaro:cec event, and with
// screen.cec.keys as an ordinary key press too, which is how this whole demo
// can be driven from the sofa (docs/cec.md). In the bridge's actions mode the
// page also acts on the bus (tessaro.screen.cec): the TV woken, put to
// standby or switched to this input, its keys sent, a scan, any message,
// and every message either way as a `message` event.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { getBridge, usePoll, useTessaroEvent } from "../bridge/bridge";
import { readRefusal } from "../bridge/refusal";
import type { CecActed, CecActions } from "../bridge/types";
import type { SectionProps } from "../features/registry";
import { bridgeEnable } from "../features/status";
import { Badge, CommandCard, Hint, KV, Log, Panel, Spinner, Stat, useCountdown, useLog } from "../shell/ui";

const COLOURS: [string, string][] = [
  ["ColorF0Red", "#ff5d6c"],
  ["ColorF1Green", "#4fd18b"],
  ["ColorF2Yellow", "#ffd34f"],
  ["ColorF3Blue", "#4f9dff"],
];

function useLit() {
  const [lit, setLit] = useState<string | null>(null);
  useEffect(() => {
    if (!lit) return;
    const timer = setTimeout(() => setLit(null), 350);
    return () => clearTimeout(timer);
  }, [lit]);
  return [lit, setLit] as const;
}

function Pad({ lit }: { lit: string | null }) {
  const key = (name: string, label: string, extra = "") => (
    <div
      className={`grid place-items-center rounded-[1rem] border text-[1.2rem] font-semibold transition-all duration-150 ${extra} ${
        lit === name
          ? "scale-95 border-transparent bg-[linear-gradient(120deg,#5cc8ff,#9d8cff)] text-ink shadow-[0_0_2rem_rgba(92,200,255,0.7)]"
          : "border-line bg-raised text-fg"
      }`}
    >
      {label}
    </div>
  );
  return (
    <div className="mx-auto flex w-[19rem] flex-col items-center gap-5 rounded-[3rem] border border-line bg-[linear-gradient(180deg,rgba(255,255,255,0.07),rgba(255,255,255,0.02))] px-8 py-8">
      <div className="grid grid-cols-3 grid-rows-3 gap-2" style={{ width: "13rem", height: "13rem" }}>
        <span />
        {key("ArrowUp", "▲")}
        <span />
        {key("ArrowLeft", "◀")}
        {key("Enter", "OK", "rounded-full")}
        {key("ArrowRight", "▶")}
        <span />
        {key("ArrowDown", "▼")}
        <span />
      </div>
      <div className="grid w-full grid-cols-4 gap-2">
        {COLOURS.map(([name, colour]) => (
          <div
            key={name}
            className="h-[1.6rem] rounded-full transition-all duration-150"
            style={{
              background: colour,
              opacity: lit === name ? 1 : 0.45,
              boxShadow: lit === name ? `0 0 1.4rem ${colour}` : "none",
            }}
          />
        ))}
      </div>
      <div className="grid w-full grid-cols-3 gap-2">
        {key("Escape", "Back", "h-[2.6rem] text-[0.85rem]")}
        {key("MediaPlayPause", "⏯", "h-[2.6rem]")}
        {key("ContextMenu", "Menu", "h-[2.6rem] text-[0.85rem]")}
      </div>
    </div>
  );
}

export function RemoteSection(_: SectionProps) {
  const bridge = getBridge();
  const read = useMemo(() => (bridge ? () => bridge.screen.show() : null), [bridge]);
  const shown = usePoll(read, 5000);
  const statusRead = useMemo(() => (bridge ? () => bridge.device.status() : null), [bridge]);
  const status = usePoll(statusRead, 5000);
  const [lit, setLit] = useLit();
  const [pressed, setPressed] = useState(0);
  const { lines, add } = useLog(80);

  useTessaroEvent(
    "tessaro:cec",
    useCallback(
      (event) => {
        const detail = event.detail;
        // Every message on the bus is the bus log's, below.
        if (detail.event === "message") return;
        if (detail.key) {
          if (detail.pressed) {
            setLit(detail.key);
            setPressed((count) => count + 1);
          }
          add(`key ${detail.key}${detail.pressed ? "" : " released"}${detail.repeat ? " (held)" : ""}`);
        } else {
          add(
            `${detail.event}: TV ${detail.tv ?? "unknown"}, ${
              detail.showing ? "showing this device" : "on another input"
            }`,
          );
        }
      },
      [add, setLit],
    ),
  );

  // The keys as key presses, for the remote and a keyboard alike.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => setLit(event.key);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [setLit]);

  const tv = status.state === "ready" ? status.value.tv : null;
  const adapter = shown.state === "ready" ? shown.value.adapters[0] : undefined;
  const set = shown.state === "ready" ? shown.value.adapters.flatMap((one) => one.devices) : [];
  const cec = bridge?.screen.cec;

  return (
    <div className="flex flex-col gap-[1.3rem]">
      <div className="grid grid-cols-[minmax(20rem,1fr)_minmax(0,1.6fr)] gap-[1.3rem]">
        <Panel title="Press a key on the TV remote">
          <Pad lit={lit} />
          <Hint>Each key lights up as it arrives. The keyboard's arrows, Enter and Escape light the same keys.</Hint>
        </Panel>
        <div className="flex flex-col gap-[1.3rem]">
          <Panel title="The TV">
            <div className="flex flex-wrap gap-x-10 gap-y-4">
              <Stat label="Power" value={tv?.power ?? "-"} tone={tv?.power === "on" ? "ok" : undefined} />
              <Stat label="Showing us" value={tv ? (tv.showing ? "yes" : "no") : "-"} />
              <Stat label="Keys pressed" value={pressed} />
            </div>
            <KV
              rows={[
                ["TV", tv?.name ?? set.find((one) => one.kind === "tv")?.name ?? "-"],
                ["Adapter", adapter ? `${adapter.device}${adapter.connector ? ` on ${adapter.connector}` : ""}` : "-"],
                ["Our name on the TV", adapter?.name ?? "-"],
                [
                  "Also on the bus",
                  set
                    .filter((one) => one.kind !== "tv")
                    .map((one) => one.name ?? one.kind)
                    .join(", ") || "-",
                ],
              ]}
            />
          </Panel>
          <Panel title="Events from the TV">
            <Log lines={lines} className="h-[12rem]" empty="Switch the TV's input, or press a key." />
          </Panel>
        </div>
      </div>
      {cec ? (
        <Bus cec={cec} />
      ) : (
        <Panel title="Act on the bus">
          <Hint>
            Waking the TV, putting it to standby, switching it to this input, sending its keys, a scan of the bus and
            raw messages are tessaro.screen.cec, which the page has only in the bridge's actions mode. This page can
            follow the TV and its remote, but not act.
          </Hint>
          <CommandCard enable={bridgeEnable("actions")} />
        </Panel>
      )}
    </div>
  );
}

/** The logical addresses, by what the standard reserves each for. */
const ADDRESSES: [number, string][] = [
  [0, "TV"],
  [1, "Recorder 1"],
  [2, "Recorder 2"],
  [3, "Tuner 1"],
  [4, "Playback 1"],
  [5, "Audio system"],
  [6, "Tuner 2"],
  [7, "Tuner 3"],
  [8, "Playback 2"],
  [9, "Recorder 3"],
  [10, "Tuner 4"],
  [11, "Playback 3"],
  [12, "Backup 1"],
  [13, "Backup 2"],
  [14, "Specific use"],
  [15, "Broadcast"],
];

function who(address: number): string {
  const name = ADDRESSES.find(([at]) => at === address)?.[1];
  return name ? `${address} (${name})` : String(address);
}

/** Messages the TV and a sound bar answer, with the opcode of the answer. */
const PRESETS: [string, string, string][] = [
  ["Power status", "8f", "90"],
  ["Its name", "46", "47"],
  ["Vendor", "8c", "87"],
  ["CEC version", "9f", "9e"],
];

/** Remote keys by protocol::cec::KEYS name. */
const VOLUME: [string, string][] = [
  ["volume-down", "Vol -"],
  ["mute", "Mute"],
  ["volume-up", "Vol +"],
];

/** How many messages the bus log keeps on screen. */
const BUS_LOG = 200;
/** How much of the device's own message log comes with it on the way in. */
const HISTORY = 30;

type Outcome =
  | { state: "idle" }
  | { state: "busy"; label: string }
  | { state: "done"; label: string; lines: string[] }
  | { state: "wait"; label: string; until: number; message: string }
  | { state: "failed"; label: string; message: string };

function describeActed(acted: CecActed, askedReply: boolean): string[] {
  const lines: string[] = [];
  for (const adapter of acted.adapters) {
    const where = `${adapter.device}${adapter.connector ? ` on ${adapter.connector}` : ""}`;
    if (adapter.error) lines.push(`${where}: ${adapter.error}`);
    for (const sent of adapter.sent) {
      const took = sent.to === 15 ? "broadcast" : sent.acked ? "acknowledged" : "not acknowledged";
      lines.push(`Sent ${sent.data || "a poll"} to ${who(sent.to)}: ${took}`);
    }
    if (acted.action === "scan") {
      lines.push(`Answered: ${adapter.answered.length ? adapter.answered.map(who).join(", ") : "nobody"}`);
    }
    if (adapter.reply) {
      lines.push(`Reply from ${who(adapter.reply.from)} at ${adapter.reply.time}: ${adapter.reply.data}`);
    } else if (askedReply) {
      lines.push("No reply within 2s.");
    }
    if (adapter.tv) lines.push(`The TV is ${adapter.tv}.`);
  }
  return lines;
}

interface BusLine {
  seq: number;
  line: string;
}

function busLine(
  time: string,
  message: { direction: "in" | "out"; from: number; to: number; data: string; acked?: boolean | null },
  name?: string | null,
): string {
  const took = message.direction === "out" ? (message.to === 15 ? "" : message.acked ? "  ack" : "  no ack") : "";
  return `${time}  ${message.direction.padEnd(3)} ${String(message.from).padStart(2)} > ${String(message.to).padEnd(2)}  ${
    message.data || "(poll)"
  }${name ? `  ${name}` : ""}${took}`;
}

function Bus({ cec }: { cec: CecActions }) {
  const [outcome, setOutcome] = useState<Outcome>({ state: "idle" });
  const left = useCountdown(outcome.state === "wait" ? outcome.until : null);
  const [keyTo, setKeyTo] = useState(0);
  const [data, setData] = useState("8f");
  const [to, setTo] = useState(0);
  const [reply, setReply] = useState("90");
  const [bus, setBus] = useState<BusLine[]>([]);
  const live = useRef(true);
  useEffect(() => {
    live.current = true;
    return () => {
      live.current = false;
    };
  }, []);

  const merge = useCallback((more: BusLine[]) => {
    setBus((old) => {
      const seen = new Set(old.map((one) => one.seq));
      const fresh = more.filter((one) => !seen.has(one.seq));
      if (!fresh.length) return old;
      return [...old, ...fresh].sort((a, b) => a.seq - b.seq).slice(-BUS_LOG);
    });
  }, []);

  // What the device kept from before the page came, then every message as
  // it happens.
  useEffect(() => {
    cec
      .messages(0)
      .then((page) => {
        if (!live.current) return;
        merge(page.messages.slice(-HISTORY).map((one) => ({ seq: one.seq, line: busLine(one.time, one) })));
      })
      .catch(() => undefined);
  }, [cec, merge]);

  useTessaroEvent(
    "tessaro:cec",
    useCallback(
      (event) => {
        const detail = event.detail;
        if (detail.event !== "message") return;
        const time = new Date().toLocaleTimeString([], { hour12: false });
        merge([{ seq: detail.seq, line: busLine(time, detail, detail.name) }]);
      },
      [merge],
    ),
  );

  const blocked = outcome.state === "busy" || (outcome.state === "wait" && left > 0);

  const act = useCallback(async (label: string, run: () => Promise<CecActed>, askedReply = false) => {
    setOutcome({ state: "busy", label });
    try {
      const acted = await run();
      if (live.current) setOutcome({ state: "done", label, lines: describeActed(acted, askedReply) });
    } catch (error) {
      if (!live.current) return;
      const refusal = readRefusal(error);
      if (refusal.kind === "wait") {
        setOutcome({ state: "wait", label, until: Date.now() + refusal.seconds * 1000, message: refusal.message });
      } else {
        setOutcome({ state: "failed", label, message: refusal.message });
      }
    }
  }, []);

  const replyCode = reply.trim() === "" ? undefined : parseInt(reply.trim().replace(/^0x/i, ""), 16);
  const replyBad = replyCode !== undefined && (!/^(0x)?[0-9a-f]{1,2}$/i.test(reply.trim()) || Number.isNaN(replyCode));

  const button = (label: string, run: () => Promise<CecActed>, extra = "", face: string = label) => (
    <button type="button" className={`btn btn-sm ${extra}`} disabled={blocked} onClick={() => void act(label, run)}>
      {outcome.state === "busy" && outcome.label === label && <Spinner />}
      {face}
    </button>
  );
  const key = (name: string, face: string) => button(`the key ${name}`, () => cec.key(name, keyTo), "", face);

  return (
    <div className="grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)] gap-[1.3rem]">
      <div className="flex flex-col gap-[1.3rem]">
        <Panel title="Act on the bus">
          <Hint>
            tessaro.screen.cec, as the operator's tessaro-ctl screen cec has it. The device allows a page 20 of these in
            10s, so a page in a loop cannot flood a bus other people's equipment shares.
          </Hint>
          <div className="flex flex-wrap gap-3">
            {button("waking the TV", () => cec.wake(true), "btn-primary", "Wake")}
            {button("the TV's standby", () => cec.standby(false), "", "Standby")}
            {button("switching the TV to this input", () => cec.source(), "", "This input")}
            {button("the scan", () => cec.scan(), "", "Scan")}
          </div>
          <span className="eyebrow">Keys, sent to</span>
          <div className="flex flex-wrap gap-2">
            {[0, 5].map((address) => (
              <button
                key={address}
                type="button"
                className={`btn btn-sm ${keyTo === address ? "btn-selected" : ""}`}
                onClick={() => setKeyTo(address)}
              >
                {address === 0 ? "The TV" : "The sound bar"}
              </button>
            ))}
          </div>
          <div className="flex flex-wrap items-start gap-6">
            <div className="grid grid-cols-3 gap-2">
              <span />
              {key("up", "▲")}
              <span />
              {key("left", "◀")}
              {key("select", "OK")}
              {key("right", "▶")}
              <span />
              {key("down", "▼")}
              <span />
            </div>
            <div className="flex flex-wrap gap-2">
              {VOLUME.map(([name, face]) => (
                <span key={name}>{key(name, face)}</span>
              ))}
            </div>
          </div>
        </Panel>
        <Panel title="Send a message">
          <Hint>
            The opcode and its operands in hex, to a logical address, from the device's own. With a reply opcode the
            device waits up to 2s for that answer from the same address.
          </Hint>
          <div className="flex flex-wrap gap-2">
            {PRESETS.map(([label, opcode, answer]) => (
              <button
                key={label}
                type="button"
                className="btn btn-sm"
                onClick={() => {
                  setData(opcode);
                  setReply(answer);
                }}
              >
                {label}
              </button>
            ))}
          </div>
          <div className="grid grid-cols-[minmax(0,1.4fr)_minmax(0,1.2fr)_minmax(0,0.8fr)] items-end gap-3">
            <label className="flex flex-col gap-1.5 text-[0.82rem] text-dim">
              Bytes
              <input
                className="field font-mono"
                value={data}
                onChange={(event) => setData(event.target.value)}
                placeholder="44 41"
              />
            </label>
            <label className="flex flex-col gap-1.5 text-[0.82rem] text-dim">
              To
              <select className="field" value={to} onChange={(event) => setTo(Number(event.target.value))}>
                {ADDRESSES.map(([address]) => (
                  <option key={address} value={address}>
                    {who(address)}
                  </option>
                ))}
              </select>
            </label>
            <label className="flex flex-col gap-1.5 text-[0.82rem] text-dim">
              Reply
              <input
                className="field font-mono"
                value={reply}
                onChange={(event) => setReply(event.target.value)}
                placeholder="none"
              />
            </label>
          </div>
          {replyBad && <span className="text-[0.85rem] text-bad">The reply is one opcode in hex, like 90.</span>}
          <div>
            <button
              type="button"
              className="btn btn-primary"
              disabled={blocked || replyBad || data.trim() === ""}
              onClick={() => void act("the message", () => cec.send(data, to, replyCode), replyCode !== undefined)}
            >
              {outcome.state === "busy" && outcome.label === "the message" && <Spinner />}
              Send
            </button>
          </div>
        </Panel>
      </div>
      <div className="flex flex-col gap-[1.3rem]">
        <Panel title="What the bus said">
          <Answer outcome={outcome} left={left} />
        </Panel>
        <Panel title="Every message on the bus" aside={<Badge tone="dim">last {BUS_LOG}</Badge>}>
          <Log
            lines={bus.map((one) => one.line)}
            className="h-[22rem]"
            empty="Nothing on the bus yet. Press a key on the TV remote, or act above."
          />
          <Hint>
            Messages either way: in is to the device, out is from it, with whether the other end acknowledged it.
          </Hint>
        </Panel>
      </div>
    </div>
  );
}

function capital(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
}

function Answer({ outcome, left }: { outcome: Outcome; left: number }) {
  switch (outcome.state) {
    case "idle":
      return <Hint>Act on the bus, and what each adapter sent, and who took it, shows here.</Hint>;
    case "busy":
      return (
        <span className="flex items-center gap-2 text-dim">
          <Spinner /> {capital(outcome.label)}...
        </span>
      );
    case "done":
      return (
        <div className="flex flex-col gap-1.5">
          <span className="text-[0.95rem] text-ok">{capital(outcome.label)}: done.</span>
          <div className="log">{outcome.lines.join("\n") || "Nothing went out."}</div>
        </div>
      );
    case "wait":
    case "failed":
      return (
        <div className="flex flex-col gap-1.5 rounded-[1rem] border border-[rgba(255,122,122,0.45)] bg-[rgba(255,122,122,0.08)] p-4">
          <span className="font-semibold text-bad">The device refused {outcome.label}.</span>
          <span className="text-[0.9rem] select-text">{outcome.message}</span>
          {outcome.state === "wait" && (
            <span className="text-[0.85rem] text-warn">
              {left > 0 ? `You can try again in ${left}s.` : "You can try again now."}
            </span>
          )}
        </div>
      );
  }
}
