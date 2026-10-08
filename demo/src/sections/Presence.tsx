// Presence detection as a page gets it: the device watches its camera
// itself (tessaro-vision on a hidden mirror) and tells the page who is
// there - an event when someone arrives, leaves, comes near or steps back,
// and every frame's faces while the page watches (docs/presence.md). With
// camera.presence.demographics on, each face also gets an estimated age and
// gender once they settle, with a classified event. The page never sees a
// picture from it; the preview here is the page's own mirror, drawn under
// the faces the device found.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { getBridge, usePoll, useTessaroEvent } from "../bridge/bridge";
import type { Face, FacesDetail, PresenceDetail } from "../bridge/types";
import type { SectionProps } from "../features/registry";
import { Hint, Log, Panel, Stat, useLog } from "../shell/ui";
import { useCamera } from "./useCamera";

const SCALE_METERS = 3;

const GREETINGS: Partial<Record<PresenceDetail["event"], string>> = {
  arrived: "Hello there!",
  near: "Nice to see you up close",
  far: "Step closer, there is more to see",
  left: "See you soon",
};

/** A face's settled age and gender, `female, 34`, or nothing before it settles. */
export function estimate(face: Face): string | null {
  if (face.gender === undefined || face.age === undefined) return null;
  return `${face.gender === "unknown" ? "gender unknown" : face.gender}, ${face.age}`;
}

function Overlay({ frame }: { frame: FacesDetail | null }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const node = canvas.current;
    if (!node) return;
    const dpr = window.devicePixelRatio || 1;
    const rect = node.getBoundingClientRect();
    node.width = Math.round(rect.width * dpr);
    node.height = Math.round(rect.height * dpr);
    const c = node.getContext("2d")!;
    c.setTransform(dpr, 0, 0, dpr, 0, 0);
    c.clearRect(0, 0, rect.width, rect.height);
    if (!frame) return;
    const w = rect.width;
    const h = rect.height;
    for (const face of frame.faces) {
      const { x, y, w: fw, h: fh } = face.box;
      const gradient = c.createLinearGradient(x * w, y * h, (x + fw) * w, (y + fh) * h);
      gradient.addColorStop(0, "#5cc8ff");
      gradient.addColorStop(1, "#9d8cff");
      c.strokeStyle = gradient;
      c.lineWidth = 4;
      c.shadowColor = "rgba(92,200,255,0.8)";
      c.shadowBlur = 18;
      const r = 18;
      c.beginPath();
      c.roundRect(x * w, y * h, fw * w, fh * h, r);
      c.stroke();
      c.shadowBlur = 0;
      c.fillStyle = "#eaf0f8";
      for (const point of Object.values(face.keypoints)) {
        c.beginPath();
        c.arc(point[0] * w, point[1] * h, 4.5, 0, Math.PI * 2);
        c.fill();
      }
      const label = `#${face.id}${face.distance !== null ? `  ${face.distance.toFixed(1)} m` : ""}${
        face.near ? "  near" : ""
      }${estimate(face) ? `  ${estimate(face)}` : ""}`;
      c.font = "600 18px system-ui, sans-serif";
      const width = c.measureText(label).width + 20;
      c.fillStyle = "rgba(10,13,20,0.8)";
      c.beginPath();
      c.roundRect(x * w, y * h - 36, width, 30, 10);
      c.fill();
      c.fillStyle = face.near ? "#5fe0a6" : "#5cc8ff";
      c.fillText(label, x * w + 10, y * h - 15);
    }
  }, [frame]);
  return <canvas ref={canvas} className="pointer-events-none absolute inset-0 h-full w-full" />;
}

function DistanceMeter({ faces, near }: { faces: Face[]; near: number | null }) {
  return (
    <div className="flex flex-col gap-2">
      <div className="relative h-[3.4rem] rounded-full border border-line bg-[rgba(0,0,0,0.35)]">
        {near !== null && (
          <div
            className="absolute inset-y-0 left-0 rounded-l-full bg-[rgba(95,224,166,0.14)]"
            style={{ width: `${(near / SCALE_METERS) * 100}%` }}
          />
        )}
        {faces.map((face) =>
          face.distance === null ? null : (
            <div
              key={face.id}
              className="absolute top-1/2 grid h-[2.6rem] w-[2.6rem] -translate-x-1/2 -translate-y-1/2 place-items-center rounded-full bg-[linear-gradient(120deg,#5cc8ff,#9d8cff)] text-[0.8rem] font-bold text-ink shadow-[0_0_1.4rem_rgba(92,200,255,0.6)] transition-[left] duration-300"
              style={{ left: `${Math.min(1, face.distance / SCALE_METERS) * 100}%` }}
            >
              {face.id}
            </div>
          ),
        )}
      </div>
      <div className="flex justify-between text-[0.75rem] text-dim tabular-nums">
        <span>at the screen</span>
        {near !== null && <span className="text-ok">near within {near.toFixed(1)} m</span>}
        <span>{SCALE_METERS} m</span>
      </div>
    </div>
  );
}

export function PresenceSection(_: SectionProps) {
  const bridge = getBridge();
  const camera = useCamera();
  const video = useRef<HTMLVideoElement>(null);
  const [frame, setFrame] = useState<FacesDetail | null>(null);
  const [greeting, setGreeting] = useState<string | null>(null);
  const { lines, add } = useLog(80);
  const read = useMemo(() => (bridge ? () => bridge.presence.status() : null), [bridge]);
  const status = usePoll(read, 4000);
  const value = status.state === "ready" ? status.value : null;

  // Faces only come while the page watches; the lease runs out by itself if
  // the page goes away without saying so.
  useEffect(() => {
    if (!bridge) return;
    bridge.presence.watch().catch(() => {});
    return () => void bridge.presence.unwatch().catch(() => {});
  }, [bridge]);

  useEffect(() => {
    if (camera.cameras.length && !camera.stream && !camera.error) void camera.start(camera.cameras[0]?.deviceId);
    // Start once a camera is known; the hook's own state changes are not a reason to start again.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [camera.cameras.length]);

  useEffect(() => {
    const node = video.current;
    if (!node) return;
    node.srcObject = camera.stream;
    if (camera.stream) node.play().catch(() => {});
  }, [camera.stream]);

  useTessaroEvent(
    "tessaro:faces",
    useCallback((event) => setFrame(event.detail), []),
  );
  useTessaroEvent(
    "tessaro:presence",
    useCallback(
      (event) => {
        const detail = event.detail;
        const settled = detail.face && estimate(detail.face);
        add(
          settled
            ? `classified: #${detail.face?.id} ${settled}`
            : `${detail.event}: ${detail.count} ${detail.count === 1 ? "person" : "people"}${detail.near ? ", near" : ""}`,
        );
        setGreeting(GREETINGS[detail.event] ?? null);
      },
      [add],
    ),
  );
  useEffect(() => {
    if (!greeting) return;
    const timer = setTimeout(() => setGreeting(null), 4000);
    return () => clearTimeout(timer);
  }, [greeting]);

  const faces = frame?.faces ?? value?.faces ?? [];
  const present = faces.length > 0 || Boolean(value?.present);
  const aspect = frame ? `${frame.width} / ${frame.height}` : "16 / 9";

  return (
    <>
      <div className="grid grid-cols-[minmax(0,2fr)_minmax(19rem,1fr)] gap-[1.3rem]">
        <Panel title="What the device sees">
          <div
            className="relative overflow-hidden rounded-[1rem] border border-line bg-black"
            style={{ aspectRatio: aspect }}
          >
            <video ref={video} className="absolute inset-0 h-full w-full object-fill opacity-90" muted playsInline />
            {!camera.stream && (
              <div className="absolute inset-0 grid place-items-center p-6 text-center text-dim">
                {camera.cameras.length
                  ? "Opening a camera mirror for the preview..."
                  : "No camera preview; the faces are still drawn as the device reports them."}
              </div>
            )}
            <Overlay frame={frame} />
            {greeting && (
              <div className="rise absolute inset-x-0 bottom-[8%] mx-auto w-fit rounded-full border border-[rgba(92,200,255,0.4)] bg-[rgba(10,13,20,0.9)] px-[2rem] py-[1rem] text-[clamp(1.6rem,4.4vmin,3rem)] font-semibold">
                <span className="title-gradient">{greeting}</span>
              </div>
            )}
          </div>
        </Panel>

        <div className="flex flex-col gap-[1.3rem]">
          <Panel title="Right now">
            <span className={`text-[2.4rem] leading-tight font-semibold ${present ? "title-gradient" : "text-dim"}`}>
              {present ? (faces.length > 1 ? `${faces.length} people here` : "Someone is here") : "Nobody here"}
            </span>
            <div className="flex flex-wrap gap-x-8 gap-y-3">
              <Stat label="Faces" value={faces.length} />
              <Stat label="Near" value={faces.some((one) => one.near) || value?.near ? "yes" : "no"} />
              <Stat label="Facing" value={faces.filter((one) => one.facing).length} />
              {value?.demographics && (
                <>
                  <Stat label="Women" value={faces.filter((one) => one.gender === "female").length} />
                  <Stat label="Men" value={faces.filter((one) => one.gender === "male").length} />
                </>
              )}
            </div>
          </Panel>
          <Panel title="How far">
            <DistanceMeter faces={faces} near={value?.nearMeters ?? null} />
          </Panel>
        </div>
      </div>

      <div className="grid grid-cols-[repeat(auto-fit,minmax(24rem,1fr))] gap-[1.3rem]">
        <Panel title="Events">
          <Hint>Walk up to the screen, come close, step back and leave: each is an event a page can act on.</Hint>
          <Log lines={lines} className="h-[11rem]" empty="Waiting for someone to arrive." />
        </Panel>
        <Panel title="Privacy">
          <Hint>
            No picture leaves the device and the page gets none from it: only boxes, distances and a track number that
            is never an identity, and an estimated age and gender only where the owner switched that on. Whether the
            device watches at all is the operator's switch, tessaro-ctl camera presence on or off - a page cannot turn
            it on.
          </Hint>
        </Panel>
      </div>
    </>
  );
}
