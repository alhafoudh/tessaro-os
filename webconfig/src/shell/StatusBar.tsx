// The GUI's status bar (device.rs status_bar): the link, the device, the
// image, the revision, whether the browser answers, the modes that are on,
// load, and a change on probation counting down on the right. A phone keeps
// it to one line: the link, the name, and what is wrong or on.

import { useEffect, useState } from "react";

import { memUsedPercent } from "../describe/common";
import { reverting } from "../describe/device";
import { useDevice } from "../device/DeviceContext";

/** Seconds a probation has left now, counted down between polls. */
export function useSecondsLeft(): number | null {
  const { status, statusAt } = useDevice();
  const [now, setNow] = useState(() => Date.now());
  const pending = status?.pending;
  useEffect(() => {
    if (!pending) return;
    const timer = setInterval(() => setNow(Date.now()), 250);
    return () => clearInterval(timer);
  }, [pending]);
  if (!pending) return null;
  return Math.max(0, pending.seconds_left - Math.floor((now - statusAt) / 1000));
}

export function StatusBar() {
  const { status, link } = useDevice();
  const left = useSecondsLeft();
  const tone = link === "online" ? "text-success" : link === "connecting" ? "text-warning" : "text-danger";
  return (
    <footer className="flex flex-wrap items-center gap-x-3 border-t border-border bg-chrome px-2 py-[0.1875rem] text-sm max-md:flex-nowrap max-md:gap-x-2 max-md:overflow-hidden max-md:py-1 max-md:whitespace-nowrap">
      <span className={tone}>{link}</span>
      {status && (
        <>
          <span className="max-md:min-w-0 max-md:truncate">
            {status.node.name}
            <span className="max-md:hidden"> ({status.node.id})</span>
          </span>
          {(status.image_version ?? status.os) && (
            <span className="text-muted max-md:hidden">{status.image_version ?? status.os}</span>
          )}
          <span className="text-muted max-md:hidden">revision {status.revision}</span>
          {status.browser_answering ? (
            <span className="max-md:hidden">browser answering</span>
          ) : (
            <span className="text-danger">browser not answering</span>
          )}
          {status.maintenance && <span className="text-warning">maintenance</span>}
          {status.debug_screen && <span className="text-warning">debug screen</span>}
          {status.cpu_percent != null && <span className="text-muted max-md:hidden">CPU {status.cpu_percent}%</span>}
          {status.memory && <span className="text-muted max-md:hidden">RAM {memUsedPercent(status.memory)}%</span>}
          {status.data && status.data.size > 0 && (
            <span className="text-muted max-md:hidden">
              /data {Math.floor((status.data.used * 100) / status.data.size)}% used
            </span>
          )}
          {status.pending && left !== null && (
            <span className="ms-auto text-warning">
              <span className="max-md:hidden">{reverting(status.pending, left)}</span>
              <span className="md:hidden">reverts in {left}s</span>
            </span>
          )}
        </>
      )}
    </footer>
  );
}
