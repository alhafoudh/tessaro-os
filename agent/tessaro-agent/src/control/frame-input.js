// Input in a frame of the player page: registered by the agent in every
// document of the page and of each frame in a process of its own, before the
// document's own scripts (control/bridge.rs, docs/playlists.md). The player
// cannot see touches inside a frame from another origin, so the frame says
// so itself, through the player's binding, which it takes away from the page
// in it first.
(() => {
  "use strict";
  if (window === window.top) {
    return;
  }
  const binding = window.__tessaroPlayerEvent;
  try {
    delete window.__tessaroPlayerEvent;
  } catch (_) {}
  if (typeof binding !== "function") {
    return;
  }
  let last = 0;
  const seen = () => {
    const now = Date.now();
    if (now - last < 1000) {
      return;
    }
    last = now;
    try {
      binding('{"event":"input"}');
    } catch (_) {}
  };
  for (const type of ["pointerdown", "touchstart", "keydown", "wheel"]) {
    window.addEventListener(type, seen, { capture: true, passive: true });
  }
})();
