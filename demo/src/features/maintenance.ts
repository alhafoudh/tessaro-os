// Maintenance mode for a few seconds, from the page. The page cannot come
// back by itself once maintenance puts another page on screen, so the
// maintenance page is the demo's own: maintenance on with this demo's
// #/maintenance-return as its URL, which counts down and switches it off
// again. The agent answers the bridge on the maintenance URL's origin, which
// is this one (docs/bridge.md, "Who may call").
//
// Switching off loads browser.url, normally the welcome page. The demo
// leaves a note in localStorage, which the welcome page shares, naming
// where to come back to (RETURN_KEY, docs/demo.md).

import { readRefusal } from "../bridge/refusal";
import type { Tessaro } from "../bridge/types";

export const MAINTENANCE_SECONDS = 5;
export const RETURN_KEY = "tessaro.demo.return";
/** How long the note to come back is good for, in ms. */
export const RETURN_FOR = 120_000;

export function maintenanceUrl(at: Location = location): string {
  return `${at.origin}${at.pathname}#/maintenance-return`;
}

/** Note where to come back to once maintenance is off. */
export function noteReturn(section: string, at: Location = location, storage: Storage = localStorage) {
  storage.setItem(RETURN_KEY, JSON.stringify({ url: `${at.origin}${at.pathname}#/${section}`, at: Date.now() }));
}

export type EndStep = { kind: "waiting"; seconds: number } | { kind: "done" };

/**
 * Switch maintenance off, asking again while the agent refuses it for being
 * too soon after the last page start; `step` hears each wait.
 */
export async function endMaintenance(
  bridge: Pick<Tessaro, "browser">,
  step: (state: EndStep) => void,
  sleep: (ms: number) => Promise<void> = (ms) => new Promise((done) => setTimeout(done, ms)),
  attempts = 120,
): Promise<void> {
  if (!bridge.browser) throw new Error("browser.maintenance needs browser.bridge.mode actions");
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    try {
      await bridge.browser.maintenance(false);
      step({ kind: "done" });
      return;
    } catch (error) {
      const refusal = readRefusal(error);
      if (refusal.kind !== "wait") throw error;
      step({ kind: "waiting", seconds: refusal.seconds });
      await sleep(1000);
    }
  }
  throw new Error("the agent kept refusing to end maintenance");
}
