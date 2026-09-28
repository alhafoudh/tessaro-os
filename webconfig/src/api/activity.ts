// When someone last touched the page, so a read made because of it counts
// as use of the session and a background refresh does not (client.ts).

/** How long after an input or a navigation a read still counts as its. */
const WINDOW_MS = 2000;

let last = Number.NEGATIVE_INFINITY;

export function touched(): void {
  last = performance.now();
}

export function recentlyActive(): boolean {
  return performance.now() - last < WINDOW_MS;
}

/** Count every real input on the page as use. */
export function watchActivity(target: Window): () => void {
  const events = ["pointerdown", "keydown", "wheel", "touchstart"] as const;
  for (const event of events) {
    target.addEventListener(event, touched, { passive: true, capture: true });
  }
  return () => {
    for (const event of events) {
      target.removeEventListener(event, touched, { capture: true });
    }
  };
}
