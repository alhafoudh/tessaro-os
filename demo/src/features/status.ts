// What a section can show right now. Every section works out its own from
// the bridge and the browser (detect.ts), and the home page's tile badges
// and the section's banner both draw it.

import type { Mode } from "../bridge/types";

/** How to switch a feature on: the operator's commands, never the page's. */
export interface Enable {
  commands: string[];
  /** Where the same switch is in Webconfig, in words. */
  webconfig?: string;
}

export type FeatureStatus =
  | { kind: "checking" }
  | { kind: "ready"; note?: string }
  | { kind: "limited"; note: string; enable?: Enable }
  | { kind: "off"; note: string; enable: Enable }
  | { kind: "needs-bridge"; need: Mode; have: "absent" | Mode }
  | { kind: "no-hardware"; note: string; enable?: Enable }
  | { kind: "offline"; note: string; enable?: Enable };

export type Tone = "ok" | "warn" | "bad" | "dim" | "info";

export function badgeOf(status: FeatureStatus): { label: string; tone: Tone } {
  switch (status.kind) {
    case "checking":
      return { label: "Checking", tone: "dim" };
    case "ready":
      return { label: "Ready", tone: "ok" };
    case "limited":
      return { label: "Partly", tone: "warn" };
    case "off":
      return { label: "Off", tone: "warn" };
    case "needs-bridge":
      return status.have === "absent" ? { label: "Bridge off", tone: "bad" } : { label: "Read-only", tone: "warn" };
    case "no-hardware":
      return { label: "Nothing plugged in", tone: "dim" };
    case "offline":
      return { label: "Offline", tone: "bad" };
  }
}

/** The command that gives the page what a call needs. */
export function bridgeEnable(need: Mode): Enable {
  return {
    commands: [`tessaro-ctl browser bridge ${need}`],
    webconfig: "Webconfig, Browser page",
  };
}
