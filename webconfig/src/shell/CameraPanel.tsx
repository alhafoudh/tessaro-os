// The camera panel beside every page, as the GUI's (device.rs
// camera_panel_view): the snapshot of one camera, as `tessaro-ctl camera
// snapshot` saves it, on Take or every second while Live. Double-clicking a
// camera on the Camera page opens it on that camera, Close closes it; live
// snapshots are taken only while it is open. While presence detection
// watches the camera, the faces it sees are boxed over the picture, green
// while near.

import { useQuery } from "@tanstack/react-query";
import { createContext, useCallback, useContext, useEffect, useState } from "react";

import { answer, client } from "../api/client";
import { estimate } from "../describe/camera";
import { fixed } from "../describe/common";
import { useDevice } from "../device/DeviceContext";
import { Button } from "../ui/controls";
import { LIVE_FRAME_MS, saveShot, takeCameraSnapshot, useAge, useShots } from "./useScreenshot";

/** Opens the camera panel on a camera, by its node (`video0`). */
export const CameraPanelContext = createContext<(device: string) => void>(() => {});

export function useOpenCamera(): (device: string) => void {
  return useContext(CameraPanelContext);
}

/** Another camera is another panel, keyed by its device. */
export function CameraPanel({ device, onClose }: { device: string; onClose: () => void }) {
  const { status, link, log } = useDevice();
  const online = link === "online";
  const [live, setLive] = useState(false);
  const take = useCallback(() => takeCameraSnapshot(device), [device]);
  const { shot, error, take: takeOne } = useShots(take, LIVE_FRAME_MS, live, online);
  const age = useAge(shot?.at);
  // A snapshot as it opens, and again when the device answers once more.
  useEffect(() => {
    if (online) void takeOne();
  }, [online, takeOne]);
  // The Camera page's list, for the camera's name.
  const shown = useQuery({
    queryKey: ["camera"],
    queryFn: () => answer(client.GET("/api/v1/camera")),
  });
  const title = shown.data?.cameras.find((one) => one.device === device)?.name ?? "Camera";
  const name = status?.node.name ?? "camera";
  // The faces presence detection sees, drawn over the picture when it
  // watches this camera: its status names the camera, the panel the node.
  const presence = useQuery({
    queryKey: ["camera.presence"],
    queryFn: () => answer(client.GET("/api/v1/camera/presence")),
    retry: false,
    refetchInterval: live && online ? LIVE_FRAME_MS : false,
  });
  const watched = presence.data?.enabled && presence.data.running && presence.data.camera === title;
  const faces = watched ? (presence.data?.frame?.faces ?? []) : [];

  return (
    <aside aria-label="Camera" className="flex min-h-0 flex-1 flex-col gap-1 p-1.5">
      <div className="flex flex-wrap items-center gap-1.5">
        <span className="text-sm font-bold">{title}</span>
        <span className="text-sm text-muted">/dev/{device}</span>
        <Button className="ms-auto" onClick={onClose} aria-label="Close the camera panel">
          Close
        </Button>
      </div>
      <div className="flex flex-wrap items-center gap-1.5">
        <Button disabled={!online} onClick={() => void takeOne()}>
          Take
        </Button>
        <Button kind={live ? "primary" : "tool"} aria-pressed={live} onClick={() => setLive((on) => !on)}>
          {live ? "Live (1s): on" : "Live (1s)"}
        </Button>
        <Button disabled={!shot} onClick={() => shot && log(`saved ${saveShot(shot, `${name}-${device}`)}`, "ok")}>
          Save
        </Button>
        {error ? (
          <span className="text-sm text-danger">{error}</span>
        ) : (
          shot && (
            <span className="text-sm text-muted">
              taken {age}s ago{shot.ageMs === undefined ? "" : `, frame ${shot.ageMs} ms old`}
            </span>
          )
        )}
      </div>
      <div className="flex min-h-40 flex-1 items-start justify-center overflow-auto border border-border bg-background p-1">
        {shot ? (
          <div className="relative inline-block max-h-full max-w-full">
            <img src={shot.url} alt={`What ${device} sees`} className="block max-h-full max-w-full object-contain" />
            {faces.length > 0 && (
              <svg
                viewBox="0 0 1 1"
                preserveAspectRatio="none"
                className="pointer-events-none absolute inset-0 h-full w-full"
                aria-label="Faces presence detection sees"
              >
                {faces.map((face) => (
                  <rect
                    key={face.id}
                    x={face.box.x}
                    y={face.box.y}
                    width={face.box.w}
                    height={face.box.h}
                    fill="none"
                    strokeWidth={2}
                    vectorEffect="non-scaling-stroke"
                    className={face.near ? "stroke-success" : "stroke-primary"}
                  >
                    <title>
                      {`#${face.id} ${fixed(face.distance, 1)} m${face.facing ? ", facing" : ""}${
                        face.demographics ? `, ${estimate(face.demographics)}` : ""
                      }`}
                    </title>
                  </rect>
                ))}
              </svg>
            )}
          </div>
        ) : (
          <span className="self-center text-sm text-muted">no snapshot of {device} yet</span>
        )}
      </div>
    </aside>
  );
}
