// The demo's building blocks. Big, legible, one look.

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";

import { readRefusal } from "../bridge/refusal";
import { badgeOf, bridgeEnable, type Enable, type FeatureStatus, type Tone } from "../features/status";
import { Icon } from "./icons";

export function Badge({ tone, children }: { tone: Tone; children: ReactNode }) {
  return <span className={`badge badge-${tone}`}>{children}</span>;
}

export function StatusBadge({ status }: { status: FeatureStatus }) {
  const { label, tone } = badgeOf(status);
  return <Badge tone={tone}>{label}</Badge>;
}

export function Panel({
  title,
  aside,
  children,
  className = "",
}: {
  title?: ReactNode;
  aside?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section className={`panel flex min-w-0 flex-col gap-4 ${className}`}>
      {(title || aside) && (
        <header className="flex items-center justify-between gap-3">
          {title && <h2 className="m-0 text-[1.15rem] font-semibold tracking-tight">{title}</h2>}
          {aside}
        </header>
      )}
      {children}
    </section>
  );
}

/** A short line of explanation under a heading. */
export function Hint({ children }: { children: ReactNode }) {
  return <p className="m-0 max-w-[60ch] text-[0.88rem] leading-relaxed text-dim">{children}</p>;
}

export function KV({ rows }: { rows: [ReactNode, ReactNode][] }) {
  return (
    <dl className="kv">
      {rows.map(([key, value], index) => (
        <div key={index} className="contents">
          <dt>{key}</dt>
          <dd>{value ?? "-"}</dd>
        </div>
      ))}
    </dl>
  );
}

/** A number that matters, large. */
export function Stat({ label, value, unit, tone }: { label: string; value: ReactNode; unit?: string; tone?: Tone }) {
  const color = tone === "ok" ? "text-ok" : tone === "warn" ? "text-warn" : tone === "bad" ? "text-bad" : "text-fg";
  return (
    <div className="flex min-w-0 flex-col gap-1">
      <span className="eyebrow">{label}</span>
      <span className={`text-[2rem] leading-none font-semibold tabular-nums ${color}`}>
        {value}
        {unit && <span className="ml-1.5 text-[0.95rem] font-medium text-dim">{unit}</span>}
      </span>
    </div>
  );
}

/** A ring that fills to `value` of 1. */
export function Gauge({ value, label, caption }: { value: number | null; label: string; caption?: string }) {
  const share = value === null ? 0 : Math.max(0, Math.min(1, value));
  const radius = 42;
  const length = 2 * Math.PI * radius;
  const tone = share > 0.85 ? "#ff7a7a" : share > 0.65 ? "#ffc66b" : "url(#gauge-accent)";
  return (
    <div className="flex flex-col items-center gap-2">
      <svg viewBox="0 0 100 100" className="h-[7.5rem] w-[7.5rem] -rotate-90">
        <defs>
          <linearGradient id="gauge-accent" x1="0" y1="0" x2="1" y2="1">
            <stop offset="0" stopColor="#5cc8ff" />
            <stop offset="1" stopColor="#9d8cff" />
          </linearGradient>
        </defs>
        <circle cx="50" cy="50" r={radius} fill="none" stroke="rgba(255,255,255,0.08)" strokeWidth="9" />
        <circle
          cx="50"
          cy="50"
          r={radius}
          fill="none"
          stroke={tone}
          strokeWidth="9"
          strokeLinecap="round"
          strokeDasharray={length}
          strokeDashoffset={length * (1 - share)}
          style={{ transition: "stroke-dashoffset 600ms ease" }}
        />
        <text
          x="50"
          y="50"
          textAnchor="middle"
          dominantBaseline="central"
          className="rotate-90 fill-fg"
          style={{ transformOrigin: "50px 50px", fontSize: 20, fontWeight: 650 }}
        >
          {value === null ? "-" : `${Math.round(share * 100)}%`}
        </text>
      </svg>
      <span className="eyebrow">{label}</span>
      {caption && <span className="text-[0.8rem] text-dim">{caption}</span>}
    </div>
  );
}

/** Seconds left until `until` (ms since the epoch), ticking. */
export function useCountdown(until: number | null): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (until === null) return;
    const timer = setInterval(() => setNow(Date.now()), 250);
    return () => clearInterval(timer);
  }, [until]);
  return until === null ? 0 : Math.max(0, Math.ceil((until - now) / 1000));
}

type ActionState =
  | { state: "idle" }
  | { state: "busy" }
  | { state: "done"; note?: string }
  | { state: "wait"; until: number; message: string }
  | { state: "failed"; message: string };

/**
 * A button that calls the device. It shows that it is working, that it
 * worked, and when the agent refused it, how long until it may ask again.
 */
export function ActionButton({
  run,
  children,
  variant = "default",
  disabled,
  done,
  className = "",
  small,
}: {
  run: (() => Promise<unknown>) | undefined;
  children: ReactNode;
  variant?: "default" | "primary" | "danger";
  disabled?: boolean;
  /** What to say once it worked, from what it answered. */
  done?: (value: unknown) => string | undefined;
  className?: string;
  small?: boolean;
}) {
  const [state, setState] = useState<ActionState>({ state: "idle" });
  const left = useCountdown(state.state === "wait" ? state.until : null);
  const live = useRef(true);
  useEffect(() => {
    live.current = true;
    return () => {
      live.current = false;
    };
  }, []);
  useEffect(() => {
    if (state.state === "wait" && left === 0) setState({ state: "idle" });
  }, [state, left]);

  const press = useCallback(async () => {
    if (!run) return;
    setState({ state: "busy" });
    try {
      const value = await run();
      if (live.current) setState({ state: "done", note: done?.(value) });
    } catch (error) {
      if (!live.current) return;
      const refusal = readRefusal(error);
      if (refusal.kind === "wait") {
        setState({ state: "wait", until: Date.now() + refusal.seconds * 1000, message: refusal.message });
      } else {
        setState({ state: "failed", message: refusal.message });
      }
    }
  }, [run, done]);

  const look = variant === "primary" ? "btn-primary" : variant === "danger" ? "btn-danger" : "";
  return (
    <div className={`flex min-w-0 flex-col items-start gap-1.5 ${className}`}>
      <button
        type="button"
        className={`btn ${look} ${small ? "btn-sm" : ""}`}
        disabled={disabled || !run || state.state === "busy" || state.state === "wait"}
        onClick={press}
      >
        {state.state === "busy" && <Spinner />}
        {children}
      </button>
      {state.state === "wait" && (
        <span className="text-[0.78rem] text-warn">Refused for now - you can try again in {left}s.</span>
      )}
      {state.state === "failed" && <span className="text-[0.78rem] text-bad">{state.message}</span>}
      {state.state === "done" && state.note && <span className="text-[0.78rem] text-ok">{state.note}</span>}
    </div>
  );
}

export function Spinner() {
  return (
    <span
      className="inline-block h-[1em] w-[1em] animate-spin rounded-full border-2 border-current border-r-transparent"
      aria-hidden="true"
    />
  );
}

/** How to switch something on: the commands, ready to type. */
export function CommandCard({ enable, note }: { enable: Enable; note?: ReactNode }) {
  return (
    <div className="flex flex-col gap-3 rounded-[1rem] border border-[rgba(255,198,107,0.3)] bg-[rgba(255,198,107,0.06)] p-4">
      {note && <p className="m-0 text-[0.95rem]">{note}</p>}
      <span className="eyebrow">To switch it on, run</span>
      {enable.commands.map((command) => (
        <code
          key={command}
          className="block rounded-[0.6rem] bg-[rgba(0,0,0,0.4)] px-3 py-2 text-[0.82rem] text-accent select-text"
        >
          {command}
        </code>
      ))}
      {enable.webconfig && <span className="text-[0.8rem] text-dim">or use {enable.webconfig}.</span>}
    </div>
  );
}

/** A section's banner when its feature is not all there. */
export function StatusBanner({ status }: { status: FeatureStatus }) {
  switch (status.kind) {
    case "checking":
    case "ready":
      return null;
    case "needs-bridge":
      // Always actions: it is what the whole demo needs, and asking for
      // config first would only send the operator back for more.
      return (
        <CommandCard
          enable={bridgeEnable("actions")}
          note={
            status.have === "absent"
              ? "The page bridge is off, so this page cannot reach the device."
              : "The page bridge is read-only: this page can look, but not act."
          }
        />
      );
    case "off":
      return <CommandCard enable={status.enable} note={status.note} />;
    case "limited":
    case "no-hardware":
    case "offline":
      return status.enable ? (
        <CommandCard enable={status.enable} note={status.note} />
      ) : (
        <div className="flex items-center gap-3 rounded-[1rem] border border-line bg-card p-4 text-[0.95rem]">
          <Icon name="warn" className="h-6 w-6 shrink-0 text-warn" />
          <span>{status.note}</span>
        </div>
      );
  }
}

/** A running list of lines, newest last, kept short. */
export function useLog(limit = 200) {
  const [lines, setLines] = useState<string[]>([]);
  const add = useCallback(
    (line: string) => {
      const stamp = new Date().toLocaleTimeString([], { hour12: false });
      setLines((old) => [...old.slice(-(limit - 1)), `${stamp}  ${line}`]);
    },
    [limit],
  );
  const clear = useCallback(() => setLines([]), []);
  return { lines, add, clear };
}

export function Log({ lines, className = "", empty }: { lines: string[]; className?: string; empty?: string }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const node = ref.current;
    if (node) node.scrollTop = node.scrollHeight;
  }, [lines]);
  return (
    <div ref={ref} className={`log ${className}`}>
      {lines.length ? lines.join("\n") : <span className="text-dim">{empty ?? "Nothing yet."}</span>}
    </div>
  );
}

export function bytes(value: number | null | undefined): string {
  if (value === null || value === undefined) return "-";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let size = value;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit += 1;
  }
  return `${size.toFixed(size >= 10 || unit === 0 ? 0 : 1)} ${units[unit]}`;
}
