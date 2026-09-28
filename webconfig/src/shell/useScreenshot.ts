// What the device's screen shows, as a JPEG from `screen/screenshot`: one
// on demand, or a new one every 3 s while `live` (worker.rs LIVE_SHOT). A
// shot is never asked for while the last is still on its way, nor while the
// tab is hidden or the device offline.

import { useCallback, useEffect, useRef, useState } from "react";

import { answer, client, failure } from "../api/client";

/** worker.rs LIVE_SHOT: how often a live view takes a screenshot. */
export const LIVE_MS = 3000;

export interface Shot {
  /** An object URL of the JPEG; only the newest is kept. */
  url: string;
  at: number;
}

export function useScreenshot(live: boolean, online: boolean) {
  const [shot, setShot] = useState<Shot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const taking = useRef(false);

  useEffect(
    () => () => {
      if (shot) URL.revokeObjectURL(shot.url);
    },
    [shot],
  );

  const take = useCallback(async () => {
    if (taking.current) return;
    taking.current = true;
    try {
      const blob = await answer(client.GET("/api/v1/screen/screenshot", { parseAs: "blob" }));
      setShot({ url: URL.createObjectURL(blob), at: Date.now() });
      setError(null);
    } catch (problem) {
      setError(failure(problem).message);
    } finally {
      taking.current = false;
    }
  }, []);

  useEffect(() => {
    if (!live || !online) return;
    const tick = () => {
      if (document.visibilityState === "visible") void take();
    };
    tick();
    const timer = setInterval(tick, LIVE_MS);
    document.addEventListener("visibilitychange", tick);
    return () => {
      clearInterval(timer);
      document.removeEventListener("visibilitychange", tick);
    };
  }, [live, online, take]);

  return { shot, error, take };
}

/** Seconds since `at`, counted every second. */
export function useAge(at: number | undefined): number | null {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);
  return at === undefined ? null : Math.max(0, Math.floor((now - at) / 1000));
}

/** Download `shot` as `<name>-<unix>.jpg`; the file's name. */
export function saveShot(shot: Shot, name: string): string {
  const file = `${name}-${Math.floor(Date.now() / 1000)}.jpg`;
  const link = document.createElement("a");
  link.href = shot.url;
  link.download = file;
  link.click();
  return file;
}
