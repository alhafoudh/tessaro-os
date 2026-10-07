// What maintenance mode shows while the demo tries it: a maintenance screen
// in the look of maintenance.html, a ring counting down the seconds, then
// maintenance switched off again (features/maintenance.ts).

import { useEffect, useState } from "react";

import { getBridge } from "../bridge/bridge";
import { endMaintenance, MAINTENANCE_SECONDS } from "../features/maintenance";
import { Mark } from "../shell/Mark";

type Phase =
  | { kind: "counting"; left: number }
  | { kind: "ending" }
  | { kind: "waiting"; seconds: number }
  | { kind: "done" }
  | { kind: "failed"; message: string };

export function MaintenanceReturn() {
  const [phase, setPhase] = useState<Phase>({ kind: "counting", left: MAINTENANCE_SECONDS });

  useEffect(() => {
    let live = true;
    const started = Date.now();
    const timer = setInterval(() => {
      const left = Math.max(0, MAINTENANCE_SECONDS - Math.floor((Date.now() - started) / 1000));
      if (!live) return;
      if (left > 0) {
        setPhase({ kind: "counting", left });
        return;
      }
      clearInterval(timer);
      setPhase({ kind: "ending" });
      const bridge = getBridge();
      if (!bridge) {
        setPhase({ kind: "failed", message: "The page bridge is not here to end maintenance." });
        return;
      }
      endMaintenance(bridge, (step) => {
        if (!live) return;
        setPhase(step.kind === "done" ? { kind: "done" } : { kind: "waiting", seconds: step.seconds });
      }).catch((error) => live && setPhase({ kind: "failed", message: String(error?.message ?? error) }));
    }, 200);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, []);

  const share = phase.kind === "counting" ? phase.left / MAINTENANCE_SECONDS : 0;
  const radius = 46;
  const length = 2 * Math.PI * radius;

  return (
    <div className="relative grid h-full place-items-center">
      <div className="backdrop">
        <div className="glow a" />
        <div className="glow b" />
      </div>
      <div className="relative z-10 flex flex-col items-center gap-8 text-center">
        <div className="relative grid h-[15rem] w-[15rem] place-items-center">
          <svg viewBox="0 0 100 100" className="absolute inset-0 -rotate-90">
            <defs>
              <linearGradient id="ring" x1="0" y1="0" x2="1" y2="1">
                <stop offset="0" stopColor="#5cc8ff" />
                <stop offset="1" stopColor="#9d8cff" />
              </linearGradient>
            </defs>
            <circle cx="50" cy="50" r={radius} fill="none" stroke="rgba(255,255,255,0.08)" strokeWidth="3" />
            <circle
              cx="50"
              cy="50"
              r={radius}
              fill="none"
              stroke="url(#ring)"
              strokeWidth="3"
              strokeLinecap="round"
              strokeDasharray={length}
              strokeDashoffset={length * (1 - share)}
              style={{ transition: "stroke-dashoffset 900ms linear" }}
            />
          </svg>
          {phase.kind === "counting" ? (
            <span className="text-[5rem] font-semibold tabular-nums">{phase.left}</span>
          ) : (
            <Mark className="h-[7rem] w-[7rem] animate-pulse" />
          )}
        </div>
        <span className="eyebrow">Maintenance mode</span>
        <h1 className="title-gradient m-0 text-[clamp(2.4rem,7vmin,5.4rem)] leading-tight font-[650]">
          Back in a moment
        </h1>
        <p className="m-0 max-w-[40ch] text-[1.15rem] text-dim">
          {phase.kind === "counting" &&
            "This is what the screen shows while the device is looked after. The demo switches it off again by itself."}
          {phase.kind === "ending" && "Switching maintenance off..."}
          {phase.kind === "waiting" &&
            `The device asks for a pause between page restarts; switching off in ${phase.seconds}s.`}
          {phase.kind === "done" && "Maintenance is off. The kiosk page is on its way back."}
          {phase.kind === "failed" && phase.message}
        </p>
      </div>
    </div>
  );
}
