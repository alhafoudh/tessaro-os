// agent/client/src/tags.rs: a device's tags, and the reserved `unclaimed`
// every client adds to a device nobody has claimed. `colour` is kept equal to
// the Rust by the golden fixtures, so a tag has the same colour here as in
// tessaro-gui.

export const UNCLAIMED_TAG = "unclaimed";

/** How many colours a tag's badge is picked from; `ui/Badge.tsx` has them. */
export const COLOURS = 8;

/** Tags from a comma-separated value, as device.tags keeps them. */
export function split(value: string): string[] {
  return value
    .split(",")
    .map((tag) => tag.trim())
    .filter((tag) => tag.length > 0);
}

/** `unclaimed` first when nobody has claimed the device, then its own. */
export function effective(tags: string[], claimed: boolean | null): string[] {
  const own = tags.filter((tag) => tag !== UNCLAIMED_TAG);
  return claimed === false ? [UNCLAIMED_TAG, ...own] : own;
}

export function isReserved(tag: string): boolean {
  return tag === UNCLAIMED_TAG;
}

/** Which of the `COLOURS` a tag's badge has: FNV-1a over its bytes. */
export function colour(tag: string): number {
  let hash = 0x811c9dc5;
  for (const byte of new TextEncoder().encode(tag)) {
    hash ^= byte;
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash % COLOURS;
}
