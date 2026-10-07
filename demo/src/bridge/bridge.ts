// The demo's one way to the device: window.tessaro, the page bridge
// (docs/bridge.md). Nothing here talks to the device otherwise - the demo
// shows exactly what any kiosk page could do.

import { useEffect, useState, useSyncExternalStore } from "react";

import type { Mode, Tessaro, TessaroEvents } from "./types";

export type BridgeMode = "absent" | Mode;

export function getBridge(): Tessaro | null {
  return typeof window !== "undefined" && window.tessaro ? window.tessaro : null;
}

export function bridgeMode(bridge: Tessaro | null = getBridge()): BridgeMode {
  return bridge ? bridge.mode : "absent";
}

/** A config flag as the agent writes it: 1 or 0, or empty for the default. */
export function flag(config: Readonly<Record<string, string>> | undefined, key: string, fallback: boolean): boolean {
  const value = (config?.[key] ?? "").trim().toLowerCase();
  if (value === "") return fallback;
  return ["1", "true", "on", "yes"].includes(value);
}

// tessaro.config is replaced, not mutated, when a setting changes, and the
// tessaro:config event says so: a store over that event.
function subscribeConfig(notify: () => void): () => void {
  window.addEventListener("tessaro:config", notify);
  return () => window.removeEventListener("tessaro:config", notify);
}

function configSnapshot(): Readonly<Record<string, string>> | undefined {
  return getBridge()?.config;
}

/** The bridge, its mode and its config, current with every tessaro:config. */
export function useBridge(): {
  bridge: Tessaro | null;
  mode: BridgeMode;
  config: Readonly<Record<string, string>>;
} {
  const config = useSyncExternalStore(subscribeConfig, configSnapshot);
  const bridge = getBridge();
  return { bridge, mode: bridgeMode(bridge), config: config ?? EMPTY };
}

const EMPTY: Readonly<Record<string, string>> = Object.freeze({});

/** Listen to one of the bridge's events while mounted. */
export function useTessaroEvent<K extends keyof TessaroEvents>(name: K, handle: (event: TessaroEvents[K]) => void) {
  useEffect(() => {
    const listener = (event: Event) => handle(event as TessaroEvents[K]);
    window.addEventListener(name, listener);
    return () => window.removeEventListener(name, listener);
  }, [name, handle]);
}

export type Loaded<T> =
  { state: "loading" } | { state: "ready"; value: T; at: number } | { state: "failed"; error: string };

/** A read from the bridge, repeated every `every` ms when given. */
export function usePoll<T>(read: (() => Promise<T>) | null, every?: number): Loaded<T> {
  const [loaded, setLoaded] = useState<Loaded<T>>({ state: "loading" });
  useEffect(() => {
    if (!read) {
      setLoaded({ state: "failed", error: "the page bridge is not here" });
      return;
    }
    let live = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const tick = async () => {
      try {
        const value = await read();
        if (live) setLoaded({ state: "ready", value, at: Date.now() });
      } catch (error) {
        if (live) setLoaded({ state: "failed", error: error instanceof Error ? error.message : String(error) });
      }
      if (live && every) timer = setTimeout(tick, every);
    };
    void tick();
    return () => {
      live = false;
      if (timer) clearTimeout(timer);
    };
  }, [read, every]);
  return loaded;
}
