// Fingers and the screen: a multi-touch canvas that paints every finger in
// its own colour, where pointer events come from, nested scrolling, the
// connected display's own identity, and the screen switched off for a
// moment and on again.

import { useCallback, useEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";

import { getBridge, usePoll } from "../bridge/bridge";
import type { SectionProps } from "../features/registry";
import { ActionButton, Hint, KV, Panel, Stat, useCountdown } from "../shell/ui";

const SCREEN_OFF_SECONDS = 10;

interface Stroke {
  hue: number;
  points: { x: number; y: number }[];
}

function FingerPaint() {
  const canvas = useRef<HTMLCanvasElement>(null);
  const strokes = useRef<Stroke[]>([]);
  const live = useRef(new Map<number, Stroke & { type: string }>());
  const [down, setDown] = useState(0);
  const [most, setMost] = useState(0);
  const hue = useRef(190);

  const draw = useCallback(() => {
    const node = canvas.current;
    if (!node) return;
    const context = node.getContext("2d")!;
    const rect = node.getBoundingClientRect();
    context.clearRect(0, 0, rect.width, rect.height);
    context.lineCap = "round";
    context.lineJoin = "round";
    for (const stroke of strokes.current) {
      context.strokeStyle = `hsl(${stroke.hue} 90% 66%)`;
      context.shadowColor = `hsl(${stroke.hue} 90% 60%)`;
      context.shadowBlur = 14;
      context.lineWidth = 9;
      context.beginPath();
      stroke.points.forEach((point, index) =>
        index ? context.lineTo(point.x, point.y) : context.moveTo(point.x, point.y),
      );
      context.stroke();
    }
    context.shadowBlur = 0;
    let index = 0;
    for (const [id, stroke] of live.current) {
      const last = stroke.points[stroke.points.length - 1];
      if (!last) continue;
      context.strokeStyle = `hsl(${stroke.hue} 90% 70%)`;
      context.lineWidth = 3;
      context.beginPath();
      context.arc(last.x, last.y, 38, 0, Math.PI * 2);
      context.stroke();
      context.fillStyle = "#eaf0f8";
      context.font = "600 15px system-ui, sans-serif";
      context.textAlign = "center";
      context.fillText(`${stroke.type} ${id}`, last.x, last.y - 48);
      index += 1;
    }
    if (!strokes.current.length && !index) {
      context.fillStyle = "#8e9bb0";
      context.font = "500 22px system-ui, sans-serif";
      context.textAlign = "center";
      context.fillText("Draw here with one finger, or with all ten", rect.width / 2, rect.height / 2);
    }
  }, []);

  useEffect(() => {
    const node = canvas.current!;
    const resize = () => {
      const dpr = window.devicePixelRatio || 1;
      const rect = node.getBoundingClientRect();
      node.width = Math.round(rect.width * dpr);
      node.height = Math.round(rect.height * dpr);
      node.getContext("2d")!.setTransform(dpr, 0, 0, dpr, 0, 0);
      draw();
    };
    const observer = new ResizeObserver(resize);
    observer.observe(node);
    resize();
    return () => observer.disconnect();
  }, [draw]);

  const at = (event: ReactPointerEvent) => {
    const rect = canvas.current!.getBoundingClientRect();
    return { x: event.clientX - rect.left, y: event.clientY - rect.top };
  };

  const update = () => {
    setDown(live.current.size);
    setMost((old) => Math.max(old, live.current.size));
    draw();
  };

  return (
    <Panel
      title="Multi-touch"
      aside={
        <div className="flex items-center gap-6">
          <Stat label="Down now" value={down} />
          <Stat label="Most at once" value={most} tone={most >= 2 ? "ok" : undefined} />
          <button
            type="button"
            className="btn btn-sm"
            onClick={() => {
              strokes.current = [];
              draw();
            }}
          >
            Clear
          </button>
        </div>
      }
    >
      <Hint>
        Every finger gets its own colour and ring. One ring under several fingers means the panel is read as a single
        pointer: the kernel's multitouch driver is missing.
      </Hint>
      <canvas
        ref={canvas}
        className="block h-[clamp(16rem,46vh,34rem)] w-full touch-none rounded-[1rem] border border-line bg-[rgba(0,0,0,0.35)]"
        onPointerDown={(event) => {
          canvas.current!.setPointerCapture(event.pointerId);
          hue.current = (hue.current + 67) % 360;
          const stroke = { hue: hue.current, points: [at(event)], type: event.pointerType };
          live.current.set(event.pointerId, stroke);
          strokes.current.push(stroke);
          update();
        }}
        onPointerMove={(event) => {
          const stroke = live.current.get(event.pointerId);
          if (!stroke) return;
          stroke.points.push(at(event));
          draw();
        }}
        // Only up and cancel end a finger: leaving the canvas while still
        // down is exactly what a multi-touch test invites.
        onPointerUp={(event) => {
          live.current.delete(event.pointerId);
          update();
        }}
        onPointerCancel={(event) => {
          live.current.delete(event.pointerId);
          update();
        }}
      />
    </Panel>
  );
}

function PointerSources() {
  const [counts, setCounts] = useState<Record<string, number>>({});
  const [wheel, setWheel] = useState(0);
  useEffect(() => {
    const onPointer = (event: PointerEvent) =>
      setCounts((old) => ({ ...old, [event.pointerType]: (old[event.pointerType] ?? 0) + 1 }));
    const onWheel = () => setWheel((old) => old + 1);
    window.addEventListener("pointerdown", onPointer, { passive: true });
    window.addEventListener("wheel", onWheel, { passive: true });
    return () => {
      window.removeEventListener("pointerdown", onPointer);
      window.removeEventListener("wheel", onWheel);
    };
  }, []);
  const seen = Object.keys(counts);
  return (
    <Panel title="Where input comes from">
      <Hint>
        Touch the screen anywhere. "touch" means the touchscreen reaches the browser as touch; only "mouse" on a touch
        panel means browser.touch or the kernel driver is wrong.
      </Hint>
      <div className="flex flex-wrap gap-x-10 gap-y-4">
        <Stat label="Touch" value={counts.touch ?? 0} tone={counts.touch ? "ok" : undefined} />
        <Stat label="Mouse" value={counts.mouse ?? 0} />
        <Stat label="Pen" value={counts.pen ?? 0} />
        <Stat label="Wheel" value={wheel} />
      </div>
      <KV
        rows={[
          ["Seen", seen.length ? seen.join(", ") : "nothing yet"],
          ["Touch events", "ontouchstart" in window ? "on" : "off (browser.touch)"],
          ["Touch points", String(navigator.maxTouchPoints)],
        ]}
      />
    </Panel>
  );
}

function Scrollers() {
  const [top, setTop] = useState(0);
  const [left, setLeft] = useState(0);
  return (
    <Panel title="Scrolling">
      <Hint>Swipe inside each box: it scrolls on its own, without moving the page.</Hint>
      <div className="grid grid-cols-[1fr_1fr] gap-4">
        <div className="flex flex-col gap-2">
          <div
            className="scroll-area h-[13rem] rounded-[0.8rem] border border-line bg-[rgba(0,0,0,0.25)] p-3"
            onScroll={(event) => setTop(Math.round(event.currentTarget.scrollTop))}
          >
            {Array.from({ length: 40 }, (_, i) => (
              <div key={i} className="border-b border-line py-2 text-[0.95rem]">
                Row {i + 1}
              </div>
            ))}
          </div>
          <span className="mono text-[0.75rem] text-dim">scrollTop {top}</span>
        </div>
        <div className="flex flex-col gap-2">
          <div
            className="flex h-[13rem] items-center gap-3 overflow-x-auto rounded-[0.8rem] border border-line bg-[rgba(0,0,0,0.25)] p-3"
            onScroll={(event) => setLeft(Math.round(event.currentTarget.scrollLeft))}
          >
            {Array.from({ length: 20 }, (_, i) => (
              <div
                key={i}
                className="grid h-[8rem] w-[8rem] shrink-0 place-items-center rounded-[0.9rem] text-[1.6rem] font-semibold"
                style={{ background: `hsl(${200 + i * 9} 60% 30% / 0.6)` }}
              >
                {i + 1}
              </div>
            ))}
          </div>
          <span className="mono text-[0.75rem] text-dim">scrollLeft {left}</span>
        </div>
      </div>
    </Panel>
  );
}

function Displays() {
  const bridge = getBridge();
  const read = useMemo(() => (bridge ? () => bridge.screen.show() : null), [bridge]);
  const shown = usePoll(read);
  const [offUntil, setOffUntil] = useState<number | null>(null);
  const left = useCountdown(offUntil);
  const on = bridge?.screen.on;
  const off = bridge?.screen.off;

  useEffect(() => {
    if (offUntil !== null && left === 0) {
      setOffUntil(null);
      on?.().catch(() => {});
    }
  }, [left, offUntil, on]);

  // Leaving the section while the screen is dark must not leave it dark.
  const dark = useRef(false);
  dark.current = offUntil !== null;
  useEffect(
    () => () => {
      if (dark.current) on?.().catch(() => {});
    },
    [on],
  );

  const connectors = shown.state === "ready" ? shown.value.connectors : [];
  return (
    <Panel title="The screen">
      {connectors.length === 0 && (
        <Hint>{shown.state === "failed" ? shown.error : "No display identity reported."}</Hint>
      )}
      {connectors.map((connector) => {
        const display = connector.display;
        const inches =
          display?.width_cm && display.height_cm
            ? Math.round(Math.hypot(display.width_cm, display.height_cm) / 2.54)
            : null;
        return (
          <div key={connector.name} className="flex flex-col gap-3">
            <div className="flex flex-wrap gap-x-10 gap-y-3">
              <Stat label="Display" value={display?.model ?? display?.vendor ?? "Unknown"} />
              {inches && <Stat label="Size" value={inches} unit="inch" />}
              <Stat label="Connector" value={connector.name} />
            </div>
            <KV
              rows={[
                ["Maker", display ? `${display.vendor ?? display.vendor_id}` : "-"],
                ["Made", display?.year ? `${display.year}${display.week ? `, week ${display.week}` : ""}` : "-"],
                ["Best modes", connector.modes.slice(0, 3).join(", ") || "-"],
              ]}
            />
          </div>
        );
      })}
      <div className="flex flex-wrap items-center gap-4">
        <ActionButton
          run={
            off
              ? async () => {
                  await off();
                  setOffUntil(Date.now() + SCREEN_OFF_SECONDS * 1000);
                }
              : undefined
          }
          disabled={offUntil !== null}
        >
          Switch the screen off for {SCREEN_OFF_SECONDS}s
        </ActionButton>
        {offUntil !== null && <span className="text-[1rem] text-warn">Back on in {left}s</span>}
      </div>
    </Panel>
  );
}

export function TouchSection(_: SectionProps) {
  return (
    <>
      <FingerPaint />
      <div className="grid grid-cols-[repeat(auto-fit,minmax(24rem,1fr))] gap-[1.3rem]">
        <PointerSources />
        <Displays />
      </div>
      <Scrollers />
    </>
  );
}
