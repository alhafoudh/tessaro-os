// Moving the focus by where things are on screen, so the arrows of a
// keyboard and of a TV remote (screen.cec.keys sends them as ArrowUp and so
// on, agent/protocol/src/cec.rs) walk the demo the way the eye does. Enter
// presses what has the focus, as it does for any button; Escape - the
// remote's exit key - goes back.

export type Direction = "up" | "down" | "left" | "right";

export interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

const ARROWS: Record<string, Direction> = {
  ArrowUp: "up",
  ArrowDown: "down",
  ArrowLeft: "left",
  ArrowRight: "right",
};

export function directionOf(key: string): Direction | null {
  return ARROWS[key] ?? null;
}

/**
 * The index of the box to move to from `from`, or -1. A candidate must lie
 * ahead in that direction; among those the nearest wins, with sideways
 * distance costing more than distance ahead, so the move stays in its row
 * or column when there is one.
 */
export function nearest(from: Box, candidates: Box[], direction: Direction): number {
  const fx = from.x + from.w / 2;
  const fy = from.y + from.h / 2;
  let best = -1;
  let bestScore = Infinity;
  candidates.forEach((box, index) => {
    const cx = box.x + box.w / 2;
    const cy = box.y + box.h / 2;
    let ahead: number;
    let aside: number;
    switch (direction) {
      case "up":
        ahead = from.y - (box.y + box.h);
        aside = Math.abs(cx - fx);
        if (cy >= fy) return;
        break;
      case "down":
        ahead = box.y - (from.y + from.h);
        aside = Math.abs(cx - fx);
        if (cy <= fy) return;
        break;
      case "left":
        ahead = from.x - (box.x + box.w);
        aside = Math.abs(cy - fy);
        if (cx >= fx) return;
        break;
      case "right":
        ahead = box.x - (from.x + from.w);
        aside = Math.abs(cy - fy);
        if (cx <= fx) return;
        break;
    }
    const score = Math.max(ahead, 0) + aside * 2.5;
    if (score < bestScore) {
      bestScore = score;
      best = index;
    }
  });
  return best;
}

const FOCUSABLE = [
  "button:not([disabled])",
  "a[href]",
  "input:not([disabled]):not([type=hidden])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "[tabindex]:not([tabindex='-1'])",
].join(",");

function visible(element: Element) {
  const rect = element.getBoundingClientRect();
  return rect.width > 0 && rect.height > 0;
}

function boxOf(element: Element): Box {
  const rect = element.getBoundingClientRect();
  return { x: rect.left, y: rect.top, w: rect.width, h: rect.height };
}

/** Whether the arrows belong to the element itself: a caret, a slider, a list. */
function ownsArrows(element: Element | null, direction: Direction) {
  if (!element) return false;
  if (element instanceof HTMLTextAreaElement || element instanceof HTMLSelectElement) return true;
  if (element instanceof HTMLInputElement) {
    if (["checkbox", "radio", "button", "submit", "reset", "color", "file", "image"].includes(element.type)) {
      return false;
    }
    // A slider takes left and right; up and down still leave it.
    if (element.type === "range") return direction === "left" || direction === "right";
    return direction === "left" || direction === "right";
  }
  return false;
}

/** Move the focus within `root`; true when it moved. */
export function moveFocus(root: ParentNode, direction: Direction): boolean {
  const all = Array.from(root.querySelectorAll(FOCUSABLE)).filter(visible) as HTMLElement[];
  if (all.length === 0) return false;
  const current = document.activeElement;
  if (!current || current === document.body || !all.includes(current as HTMLElement)) {
    all[0]!.focus();
    return true;
  }
  const others = all.filter((element) => element !== current);
  const index = nearest(boxOf(current), others.map(boxOf), direction);
  if (index < 0) return false;
  const next = others[index]!;
  next.focus();
  next.scrollIntoView({ block: "nearest", inline: "nearest" });
  return true;
}

/** Handle one key for the whole page; true when it was the demo's. */
export function handleKey(event: KeyboardEvent, back: () => void): boolean {
  const target = document.activeElement;
  const typing =
    target instanceof HTMLTextAreaElement ||
    (target instanceof HTMLInputElement && !["checkbox", "radio", "range", "button"].includes(target.type));
  if (event.key === "Escape" || (event.key === "Backspace" && !typing)) {
    if (typing && event.key === "Escape") {
      (target as HTMLElement).blur();
      return true;
    }
    back();
    return true;
  }
  const direction = directionOf(event.key);
  if (!direction || ownsArrows(target, direction)) return false;
  return moveFocus(document, direction);
}
