// agent/client/src/storage.rs: what growing /data would do, and the lines
// its job reads as.

import type { Schemas } from "../api/client";
import { fact, Line, type Fact } from "../text/line";
import { sizeLabel } from "./common";

/** protocol::GROW_MIN: the free space after /data worth growing into. */
export const GROW_MIN = 64 * 1024 * 1024;

/** What to tell the user when a grow stops half way. */
export const STOPPED_HINT = "running `tessaro-ctl storage grow` again picks up where this one stopped";

type Plan = Extract<Schemas["StorageGrowEvent"], { phase: "plan" }>;

export function isPlan(event: Schemas["StorageGrowEvent"]): event is Plan {
  return event.phase === "plan";
}

/** Whether the grow would change anything. */
export function grows(plan: Plan): boolean {
  return plan.partition_to > plan.partition_from || plan.filesystem_to > plan.filesystem_from;
}

/** The plan as facts, and the line to add when there is nothing to grow. */
export function planFacts(plan: Plan): { facts: Fact[]; nothing: Line | null } {
  const change = (from: number, to: number) =>
    to > from
      ? Line.plain(`${sizeLabel(from)} -> `).add("ok", sizeLabel(to))
      : Line.plain(`${sizeLabel(from)} `).add("muted", "(unchanged)");
  return {
    facts: [
      fact(
        "partition",
        Line.of("heading", plan.partition).text(" ").join(change(plan.partition_from, plan.partition_to)),
      ),
      fact("filesystem", change(plan.filesystem_from, plan.filesystem_to)),
    ],
    nothing: grows(plan) ? null : Line.of("ok", "nothing to grow: /data already fills the disk"),
  };
}

/** A step of the grow, as a line; the plan it starts with has none. */
export function eventLine(step: Schemas["StorageGrowEvent"]): Line | null {
  switch (step.phase) {
    case "plan":
      return null;
    case "step":
      return Line.plain(`${step.what} `).add("muted", `(${step.command})`);
    case "grown":
      return Line.of("ok", `grew /data to ${sizeLabel(step.filesystem)}`)
        .text(" ")
        .add("muted", `(partition ${sizeLabel(step.partition)})`);
  }
}
