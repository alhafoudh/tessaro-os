// Opening a camera mirror, shared by the camera and presence sections. The
// page never sees a camera itself: tessaro-camera republishes each as
// virtual cameras of the same name, which the device's origins are granted
// by policy, so there is no prompt (docs/camera.md).

import { useCallback, useEffect, useRef, useState } from "react";

export interface CameraState {
  cameras: MediaDeviceInfo[];
  chosen: string;
  stream: MediaStream | null;
  settings: string | null;
  error: string | null;
}

export function useCamera() {
  const [state, setState] = useState<CameraState>({
    cameras: [],
    chosen: "",
    stream: null,
    settings: null,
    error: null,
  });
  const stream = useRef<MediaStream | null>(null);

  const list = useCallback(async () => {
    try {
      const all = await navigator.mediaDevices.enumerateDevices();
      const cameras = all.filter((one) => one.kind === "videoinput");
      setState((old) => ({
        ...old,
        cameras,
        chosen: cameras.some((one) => one.deviceId === old.chosen) ? old.chosen : (cameras[0]?.deviceId ?? ""),
      }));
    } catch (error) {
      setState((old) => ({ ...old, error: String(error) }));
    }
  }, []);

  const stop = useCallback(() => {
    stream.current?.getTracks().forEach((track) => track.stop());
    stream.current = null;
    setState((old) => ({ ...old, stream: null, settings: null }));
  }, []);

  const start = useCallback(
    async (deviceId?: string) => {
      stop();
      // 1080p, the one mode a mirror offers: without a size Chromium asks
      // for 640 x 480 and scales the mirror down.
      const video: MediaTrackConstraints = { width: { ideal: 1920 }, height: { ideal: 1080 } };
      if (deviceId) video.deviceId = { exact: deviceId };
      try {
        const opened = await navigator.mediaDevices.getUserMedia({ video, audio: false });
        stream.current = opened;
        const track = opened.getVideoTracks()[0];
        const settings = track?.getSettings();
        track?.addEventListener("ended", stop);
        setState((old) => ({
          ...old,
          chosen: deviceId ?? old.chosen,
          stream: opened,
          error: null,
          settings: settings
            ? `${settings.width} x ${settings.height}${
                settings.frameRate ? ` at ${Math.round(settings.frameRate)} fps` : ""
              }`
            : null,
        }));
        // The labels are there once one camera is open.
        void list();
      } catch (error) {
        const failure = error as DOMException;
        setState((old) => ({ ...old, error: `${failure.name}: ${failure.message}` }));
      }
    },
    [list, stop],
  );

  useEffect(() => {
    void list();
    navigator.mediaDevices?.addEventListener("devicechange", list);
    // Nobody is in front of a kiosk to close a forgotten preview, and an
    // open reader keeps the mirror busy.
    return () => {
      navigator.mediaDevices?.removeEventListener("devicechange", list);
      stream.current?.getTracks().forEach((track) => track.stop());
    };
  }, [list]);

  return { ...state, list, start, stop };
}
