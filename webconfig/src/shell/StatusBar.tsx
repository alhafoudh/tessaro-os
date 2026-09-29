// The GUI's status bar (device.rs status_bar): the link, the device, the
// image, the revision, whether the browser answers, the modes that are on,
// load, and a change on probation counting down on the right.

import { useEffect, useState } from "react";

import { memUsedPercent, previousOrDefault } from "../describe/common";
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
    <footer className="flex flex-wrap items-center gap-x-3 border-t border-border bg-chrome px-2 py-[0.1875rem] text-sm">
      <span className={tone}>{link}</span>
      {status && (
        <>
          <span>
            {status.node.name} ({status.node.id})
          </span>
          {(status.image_version ?? status.os) && (
            <span className="text-muted">{status.image_version ?? status.os}</span>
          )}
          <span className="text-muted">revision {status.revision}</span>
          {status.browser_answering ? (
            <span>browser answering</span>
          ) : (
            <span className="text-danger">browser not answering</span>
          )}
          {status.maintenance && <span className="text-warning">maintenance</span>}
          {status.debug_screen && <span className="text-warning">debug screen</span>}
          {status.cpu_percent != null && <span className="text-muted">CPU {status.cpu_percent}%</span>}
          {status.memory && <span className="text-muted">RAM {memUsedPercent(status.memory)}%</span>}
          {status.data && status.data.size > 0 && (
            <span className="text-muted">/data {Math.floor((status.data.used * 100) / status.data.size)}% used</span>
          )}
          {status.pending && left !== null && (
            <span className="ms-auto text-warning">
              {status.pending.key}={status.pending.value} reverts to {previousOrDefault(status.pending.previous)} in{" "}
              {left}s
            </span>
          )}
        </>
      )}
    </footer>
  );
}
