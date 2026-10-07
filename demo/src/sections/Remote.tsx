// The TV over HDMI-CEC: whether it is on and showing this device, and its
// remote. Every key comes to the page as a tessaro:cec event, and with
// screen.cec.keys as an ordinary key press too, which is how this whole demo
// can be driven from the sofa (docs/cec.md).

import { useCallback, useEffect, useMemo, useState } from "react";

import { getBridge, usePoll, useTessaroEvent } from "../bridge/bridge";
import type { SectionProps } from "../features/registry";
import { Hint, KV, Log, Panel, Stat, useLog } from "../shell/ui";

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

  return (
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
  );
}
