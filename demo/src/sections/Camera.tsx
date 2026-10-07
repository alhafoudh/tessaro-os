// The camera from the page: pick a mirror, preview it live, take snapshots,
// and play with filters drawn by the browser itself.

import { useEffect, useRef, useState } from "react";

import type { SectionProps } from "../features/registry";
import { Hint, KV, Panel } from "../shell/ui";
import { useCamera } from "./useCamera";

const FILTERS: [string, string][] = [
  ["Natural", "none"],
  ["Mono", "grayscale(1) contrast(1.15)"],
  ["Warm", "sepia(0.45) saturate(1.4)"],
  ["Pop", "saturate(2.2) contrast(1.2)"],
  ["Night", "hue-rotate(180deg) invert(1) contrast(1.2)"],
];

export function CameraSection(_: SectionProps) {
  const camera = useCamera();
  const video = useRef<HTMLVideoElement>(null);
  const [filter, setFilter] = useState("none");
  const [shots, setShots] = useState<string[]>([]);

  useEffect(() => {
    const node = video.current;
    if (!node) return;
    node.srcObject = camera.stream;
    if (camera.stream) node.play().catch(() => {});
  }, [camera.stream]);

  const snap = () => {
    const node = video.current;
    if (!node || !node.videoWidth) return;
    const canvas = document.createElement("canvas");
    canvas.width = 640;
    canvas.height = Math.round((640 * node.videoHeight) / node.videoWidth);
    const context = canvas.getContext("2d")!;
    context.filter = filter;
    context.drawImage(node, 0, 0, canvas.width, canvas.height);
    const url = canvas.toDataURL("image/jpeg", 0.85);
    setShots((old) => [url, ...old].slice(0, 6));
  };

  return (
    <>
      <div className="grid grid-cols-[minmax(0,2fr)_minmax(18rem,1fr)] gap-[1.3rem]">
        <Panel title="Live preview">
          <div className="relative overflow-hidden rounded-[1rem] border border-line bg-black">
            <video
              ref={video}
              className="block aspect-video w-full bg-black object-cover"
              style={{ filter }}
              muted
              playsInline
            />
            {!camera.stream && (
              <div className="absolute inset-0 grid place-items-center p-6 text-center text-[1.1rem] text-dim">
                {camera.error ?? (camera.cameras.length ? "Pick a camera to start." : "No camera is plugged in.")}
              </div>
            )}
          </div>
          <div className="flex flex-wrap gap-2">
            {FILTERS.map(([name, css]) => (
              <button
                key={name}
                type="button"
                className={`btn btn-sm ${filter === css ? "btn-primary" : ""}`}
                onClick={() => setFilter(css)}
              >
                {name}
              </button>
            ))}
          </div>
        </Panel>

        <div className="flex flex-col gap-[1.3rem]">
          <Panel title="Cameras">
            <div className="flex flex-col gap-2">
              {camera.cameras.map((one, index) => (
                <button
                  key={one.deviceId || index}
                  type="button"
                  className={`btn w-full justify-start ${
                    camera.stream && camera.chosen === one.deviceId ? "btn-selected" : ""
                  }`}
                  onClick={() => camera.start(one.deviceId || undefined)}
                >
                  <span className="truncate">{one.label || `Camera ${index + 1}`}</span>
                </button>
              ))}
              {!camera.cameras.length && <Hint>None yet. Plug one in; it shows up here by itself.</Hint>}
            </div>
            {camera.stream && (
              <div className="flex flex-wrap gap-3">
                <button type="button" className="btn btn-primary" onClick={snap}>
                  Take a snapshot
                </button>
                <button type="button" className="btn" onClick={camera.stop}>
                  Stop
                </button>
              </div>
            )}
            <KV rows={[["Capturing", camera.settings ?? "-"]]} />
          </Panel>
          <Panel title="Snapshots">
            {shots.length ? (
              <div className="grid grid-cols-2 gap-2">
                {shots.map((shot, index) => (
                  <img key={index} src={shot} alt="" className="w-full rounded-[0.6rem] border border-line" />
                ))}
              </div>
            ) : (
              <Hint>Snapshots stay in this page; nothing is uploaded.</Hint>
            )}
          </Panel>
        </div>
      </div>
      <Hint>
        Each entry is a mirror: tessaro-camera captures the real camera and copies every frame into its mirrors, one
        reader each, so this preview and presence detection watch the same camera at once. tessaro-ctl camera list says
        what each captures.
      </Hint>
    </>
  );
}
