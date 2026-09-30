// Pictures polled from the device, as JPEGs: the screen from
// `screen/screenshot`, a camera from `camera/{device}/snapshot`. One on
// demand, or a new one every so often while `live` (worker.rs LIVE_SHOT and
// LIVE_FRAME). A picture is never asked for while the last is still on its
// way, nor while the tab is hidden or the device offline.

import { useCallback, useEffect, useRef, useState } from "react";

import { answer, answered, client, failure } from "../api/client";

/** worker.rs LIVE_SHOT: how often a live view takes a screenshot. */
export const LIVE_MS = 3000;

/** worker.rs LIVE_FRAME: how often a live camera preview takes a snapshot. */
export const LIVE_FRAME_MS = 1000;

/** protocol::api::HEADER_FRAME_AGE: how old a camera's frame is, in ms. */
export const HEADER_FRAME_AGE = "x-tessaro-frame-age";

/** What a fetch of a picture brings: the JPEG, and its age where known. */
export interface Taken {
  blob: Blob;
  /** How old the picture was when the device answered, in ms. */
  ageMs?: number;
}

export interface Shot {
  /** An object URL of the JPEG; only the newest is kept. */
  url: string;
  at: number;
  ageMs?: number;
}

/** What the device's screen shows. */
export async function takeScreenshot(): Promise<Taken> {
  return { blob: await answer(client.GET("/api/v1/screen/screenshot", { parseAs: "blob" })) };
}

/** The newest frame of camera `device` (`video0`). */
export async function takeCameraSnapshot(device: string): Promise<Taken> {
  const { data, response } = await answered(
    client.GET("/api/v1/camera/{device}/snapshot", { params: { path: { device } }, parseAs: "blob" }),
  );
  const header = response.headers.get(HEADER_FRAME_AGE);
  const age = header === null ? NaN : Number(header);
  return { blob: data, ageMs: Number.isFinite(age) ? age : undefined };
}

/**
 * The newest picture `take` fetched, taken again every `everyMs` while
 * `live`. `take` must keep its identity (a module function, or
 * `useCallback`); a view of another camera is another component, keyed by
 * the camera, so it starts with no picture.
 */
export function useShots(take: () => Promise<Taken>, everyMs: number, live: boolean, online: boolean) {
  const [shot, setShot] = useState<Shot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const taking = useRef(false);

  useEffect(
    () => () => {
      if (shot) URL.revokeObjectURL(shot.url);
    },
    [shot],
  );

  const once = useCallback(async () => {
    if (taking.current) return;
    taking.current = true;
    try {
      const taken = await take();
      setShot({ url: URL.createObjectURL(taken.blob), at: Date.now(), ageMs: taken.ageMs });
      setError(null);
    } catch (problem) {
      setError(failure(problem).message);
    } finally {
      taking.current = false;
    }
  }, [take]);

  useEffect(() => {
    if (!live || !online) return;
    const tick = () => {
      if (document.visibilityState === "visible") void once();
    };
    tick();
    const timer = setInterval(tick, everyMs);
    document.addEventListener("visibilitychange", tick);
    return () => {
      clearInterval(timer);
      document.removeEventListener("visibilitychange", tick);
    };
  }, [live, online, once, everyMs]);

  return { shot, error, take: once };
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
