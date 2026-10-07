// What the agent's refusals say, read back into something the demo can show
// as a countdown instead of an error. The wording is the agent's
// (control/bridge.rs: disrupt, the speed test gap, the print and script
// bursts); these patterns follow it.

export type Refusal =
  | { kind: "wait"; seconds: number; message: string }
  | { kind: "needs-actions"; message: string }
  | { kind: "error"; message: string };

const WAIT = [/try again in (\d+)s/, /the next one in (\d+)s/, /in the last (\d+)s/];

export function readRefusal(error: unknown): Refusal {
  const message = error instanceof Error ? error.message : String(error);
  if (/needs browser\.bridge\.mode actions/.test(message)) {
    return { kind: "needs-actions", message };
  }
  for (const pattern of WAIT) {
    const found = pattern.exec(message);
    if (found?.[1]) {
      return { kind: "wait", seconds: Number(found[1]), message };
    }
  }
  return { kind: "error", message };
}
