// The live view beside every page, where the GUI has its VNC panel
// (device.rs, the Vnc panel): what the device's screen shows, a new
// screenshot every 3 s for as long as the panel is open. A browser has no
// VNC client, so it is polled pictures, not a stream; it follows the screen
// closely enough to watch a page change, a mode go on probation, the debug
// screen come up. The title bar's Screen toggles it, and whether it is open
// is remembered in this browser.

import { useEffect, useState } from "react";

import { useDevice } from "../device/DeviceContext";
import { Button } from "../ui/controls";
import { saveShot, useAge, useScreenshot } from "./useScreenshot";

const KEY = "tessaro-webconfig.screen-panel";

/** Whether the panel is open, kept across reloads in this browser. */
export function useScreenPanel(): [boolean, (open: boolean) => void] {
  const [open, setOpen] = useState(() => {
    try {
      return window.localStorage.getItem(KEY) === "1";
    } catch {
      return false;
    }
  });
  useEffect(() => {
    try {
      window.localStorage.setItem(KEY, open ? "1" : "0");
    } catch {
      // A browser that keeps nothing opens it closed next time.
    }
  }, [open]);
  return [open, setOpen];
}

export function ScreenPanel({ onClose }: { onClose: () => void }) {
  const { status, link, log } = useDevice();
  const online = link === "online";
  const [paused, setPaused] = useState(false);
  const { shot, error, take } = useScreenshot(!paused, online);
  const age = useAge(shot?.at);
  const name = status?.node.name ?? "screen";

  return (
    <aside
      aria-label="Screen"
      className="flex max-h-[45vh] min-h-0 flex-col gap-1 border-t border-border bg-panel p-1.5 md:max-h-none md:w-[40%] md:max-w-[900px] md:min-w-[280px] md:border-t-0 md:border-l"
    >
      <div className="flex flex-wrap items-center gap-1.5">
        <span className="text-sm font-bold">Screen</span>
        <Button kind={paused ? "tool" : "primary"} aria-pressed={!paused} onClick={() => setPaused((now) => !now)}>
          {paused ? "Live (3s)" : "Live (3s): on"}
        </Button>
        <Button disabled={!online} onClick={() => void take()}>
          Take
        </Button>
        <Button disabled={!shot} onClick={() => shot && log(`saved ${saveShot(shot, name)}`, "ok")}>
          Save
        </Button>
        <Button className="ms-auto" onClick={onClose} aria-label="Close the screen panel">
          Close
        </Button>
      </div>
      <div className="text-sm">
        {!online ? (
          <span className="text-warning">the device is not answering; the last picture stays</span>
        ) : error ? (
          <span className="text-danger">{error}</span>
        ) : shot ? (
          <span className="text-muted">
            taken {age}s ago{paused ? ", paused" : ""}
          </span>
        ) : (
          <span className="text-muted">taking the first screenshot ...</span>
        )}
      </div>
      <div className="flex min-h-40 flex-1 items-start justify-center overflow-auto border border-border bg-background p-1">
        {shot ? (
          <img
            src={shot.url}
            alt="What the device's screen shows"
            className={`max-h-full max-w-full object-contain ${online ? "" : "opacity-60"}`}
          />
        ) : (
          <span className="self-center text-sm text-muted">no screenshot yet</span>
        )}
      </div>
    </aside>
  );
}
