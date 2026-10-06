// The player page: plays the playlist tessaro-agent writes to
// /run/tessaro-kiosk/playlist.json (served at /playlist.json), one item
// after another, with nothing on screen ever going blank or half-drawn.
//
// The screen is a stack of layers. An item is prepared in a layer of its own
// underneath the one on screen, and only once it can be drawn complete -
// the image decoded, the video's first frame decoded, the page loaded - is
// it raised and faded or slid in on the compositor. The next item is
// prepared as soon as the current one is shown, so it is normally ready
// long before it is due; when it is not, the current one stays.
//
// What decides anything (what comes next, when to move on, the trim window,
// what a new playlist.json means) is in player-core.js, which is
// unit-tested. This file is the DOM around it.
//
// The agent reaches the page through window.__tessaroPlayer and hears from
// it through window.__tessaroPlayerEvent, a CDP binding. Opened on a desktop
// with ?src=other.json there is no binding, and the events go to the
// console instead.

import * as core from "./player-core.js";

const stage = document.getElementById("stage");

const params = new URLSearchParams(location.search);
const SRC = params.get("src") || "/playlist.json";
// The agent calls reload() whenever it writes the file. A developer's
// ?src= has no agent behind it, so the page looks for changes itself.
const POLL_MS = params.has("src") ? 5_000 : 0;

// --- events -------------------------------------------------------------

function send(json) {
  if (typeof window.__tessaroPlayerEvent === "function") {
    try {
      window.__tessaroPlayerEvent(json);
      return;
    } catch (error) {
      console.warn("player: event binding failed", error);
    }
  }
  console.log("player event", json);
}

// --- input --------------------------------------------------------------

// When someone last touched the screen, for interactive items. Touches
// inside a cross-origin iframe never reach this document; the agent sees
// those and calls __tessaroPlayer.input() for them.
let lastInput = -Infinity;

function input() {
  lastInput = performance.now();
}

for (const type of ["pointerdown", "touchstart", "keydown", "wheel"]) {
  window.addEventListener(type, input, { capture: true, passive: true });
}

// --- small helpers ------------------------------------------------------

function delay(ms, token) {
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, ms);
    token?.promise.then(() => {
      clearTimeout(timer);
      resolve();
    });
  });
}

// A cancellation token: `cancelled` to check, `promise` to race against.
// A child is cancelled with its parent.
function makeToken(parent) {
  let cancel;
  const token = { cancelled: false, promise: new Promise((resolve) => (cancel = resolve)) };
  token.cancel = () => {
    token.cancelled = true;
    cancel();
  };
  parent?.promise.then(token.cancel);
  return token;
}

function once(target, type) {
  return new Promise((resolve) => target.addEventListener(type, resolve, { once: true }));
}

// Rejects when the event fires. Marked handled up front: an error that
// comes after the prepare gave up on the element must not surface as an
// unhandled rejection, and whoever races it still sees the rejection.
function failOn(target, type, reason) {
  const failed = new Promise((_, reject) =>
    target.addEventListener(type, () => reject(new Error(reason)), { once: true }),
  );
  failed.catch(() => {});
  return failed;
}

function withTimeout(promise, ms, reason) {
  let timer;
  const timeout = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error(reason)), ms);
  });
  return Promise.race([promise, timeout]).finally(() => clearTimeout(timer));
}

// Two frames, so a style set before this has been committed to the
// compositor and a transition set after it really animates from there.
function nextFrames() {
  return new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
}

function cssFit(fit) {
  return fit === "stretch" ? "fill" : fit;
}

// --- layers -------------------------------------------------------------

// A layer starts transparent and below whatever is on screen, and takes no
// input until it is shown.
function newLayer(item) {
  const layer = document.createElement("div");
  layer.className = "layer";
  layer.style.backgroundColor = item.background;
  stage.appendChild(layer);
  return layer;
}

// Each prepare resolves to a slide: the layer, how to start it once it is on
// screen (start returns a promise that resolves when the item is done), and
// how to free it. Everything it set up is released by teardown, which is
// safe to call twice.
function slide(item, layer, { start = () => new Promise(() => {}), teardown = () => {} } = {}) {
  let gone = false;
  return {
    item,
    layer,
    start,
    teardown() {
      if (gone) return;
      gone = true;
      teardown();
      layer.remove();
    },
  };
}

// An image or a page is done when it has been up for its duration and,
// when interactive, nobody has touched it for idle_s.
function timed(item, single) {
  return (session) =>
    new Promise((resolve) => {
      if (single) return;
      const shownAt = performance.now();
      const timer = setInterval(() => {
        const now = performance.now();
        const advance = core.shouldAdvance({
          elapsedMs: now - shownAt,
          durationS: item.duration_s,
          interactive: item.interactive,
          idleS: item.idle_s,
          sinceInputMs: now - lastInput,
        });
        if (advance) {
          clearInterval(timer);
          resolve({});
        }
      }, 250);
      session.timers.push(() => clearInterval(timer));
    });
}

async function prepareImage(item, layer, single) {
  const img = new Image();
  img.decoding = "async";
  img.draggable = false;
  img.alt = "";
  img.style.objectFit = cssFit(item.fit);
  const failed = failOn(img, "error", "image failed to load");
  img.src = item.src;
  // decode() resolves once the image can be painted without a decode on the
  // frame it first appears in, which is the glitch this whole page avoids.
  await Promise.race([img.decode(), failed]);
  layer.appendChild(img);
  const session = { timers: [] };
  return slide(item, layer, {
    start: () => timed(item, single)(session),
    teardown: () => {
      session.timers.forEach((stop) => stop());
      img.removeAttribute("src");
    },
  });
}

async function prepareUrl(item, layer, single) {
  const frame = document.createElement("iframe");
  if (item.interactive) {
    // The device APIs a kiosk page may use, delegated to the frame. The
    // grants themselves are in the Chromium policy, by origin.
    frame.allow =
      "autoplay; fullscreen; serial; hid; usb; bluetooth; camera; microphone; geolocation; clipboard-read; clipboard-write";
  } else {
    // A page that is only shown takes no touches and no focus, so a tap
    // lands on nothing and the keyboard never wanders into it.
    frame.classList.add("passive");
    frame.tabIndex = -1;
    frame.setAttribute("aria-hidden", "true");
  }
  frame.referrerPolicy = "no-referrer-when-downgrade";
  const loaded = once(frame, "load");
  frame.src = item.src;
  layer.appendChild(frame);
  await loaded;
  // A page that draws after its load event (a single-page app fetching its
  // data) gets ready_delay_ms more before it is shown.
  if (item.ready_delay_ms > 0) await delay(item.ready_delay_ms);
  const session = { timers: [] };
  return slide(item, layer, {
    start: () => {
      if (item.interactive) frame.focus();
      return timed(item, single)(session);
    },
    teardown: () => {
      session.timers.forEach((stop) => stop());
      // about:blank before removing, so the page's unload runs and its
      // timers, sockets and media stop now rather than whenever the frame is
      // collected.
      frame.src = "about:blank";
      frame.remove();
    },
  });
}

async function prepareVideo(item, layer, single) {
  const video = document.createElement("video");
  video.playsInline = true;
  video.preload = "auto";
  video.disablePictureInPicture = true;
  video.muted = !item.sound;
  video.defaultMuted = !item.sound;
  video.volume = item.volume / 100;
  video.style.objectFit = cssFit(item.fit);
  const failed = failOn(video, "error", "video failed to load");
  layer.appendChild(video);
  video.src = item.src;

  await Promise.race([once(video, "loadedmetadata"), failed]);
  const window_ = core.trimWindow(item, video.duration);
  if (!window_) throw new Error("trim window is empty");

  // Seek to the trim start and wait for that frame to be decoded, so the
  // first frame on screen is the right one and is already there. A video
  // that starts at 0 is there already and needs no seek.
  if (video.currentTime !== window_.start) {
    const seeked = once(video, "seeked");
    video.currentTime = window_.start;
    await Promise.race([seeked, failed]);
  }
  if (video.readyState < HTMLMediaElement.HAVE_CURRENT_DATA) {
    await Promise.race([once(video, "canplay"), failed]);
  }
  if ("requestVideoFrameCallback" in video) {
    // A frame callback is the proof the frame reached the compositor. A
    // paused video in a transparent layer may never present one, so the
    // readiness above is enough after a short wait.
    await Promise.race([new Promise((resolve) => video.requestVideoFrameCallback(resolve)), delay(300)]);
  }
  video.pause();

  const session = { timers: [], frameS: 0 };
  return slide(item, layer, {
    start: () => playVideo(item, video, window_, single, session, failed),
    teardown: () => {
      session.timers.forEach((stop) => stop());
      // Pausing is not enough to give the hardware decoder back; dropping
      // the source and loading nothing is.
      video.pause();
      video.removeAttribute("src");
      video.load();
    },
  });
}

function playVideo(item, video, window_, single, session, failed) {
  return new Promise((resolve) => {
    let finished = false;
    const finish = (result) => {
      if (finished) return;
      finished = true;
      session.timers.forEach((stop) => stop());
      session.timers = [];
      resolve(result);
    };
    const skip = (reason) => finish({ skipped: reason });

    const play = () =>
      video.play().catch((error) => {
        // Only a desktop browser without the kiosk's autoplay policy gets
        // here: sound is refused without a gesture, so play it muted rather
        // than not at all.
        if (!video.muted) {
          console.warn("player: playing muted, autoplay with sound was refused", error);
          video.muted = true;
          return video.play().catch((again) => skip(`video would not play: ${again.message}`));
        }
        skip(`video would not play: ${error.message}`);
      });

    // The end of the window: a single video goes back to the trim start
    // with no transition; otherwise it stops on its last frame, which stays
    // on screen until the next item is ready.
    const reachedEnd = () => {
      if (single) {
        video.currentTime = window_.start;
        play();
        return;
      }
      video.pause();
      finish({});
    };

    if ("requestVideoFrameCallback" in video) {
      let lastMediaTime = null;
      const onFrame = (_now, meta) => {
        if (finished) return;
        if (lastMediaTime !== null && meta.mediaTime > lastMediaTime) {
          session.frameS = meta.mediaTime - lastMediaTime;
        }
        lastMediaTime = meta.mediaTime;
        if (core.atTrimEnd(meta.mediaTime, window_.end, session.frameS)) {
          lastMediaTime = null;
          reachedEnd();
        }
        handle = video.requestVideoFrameCallback(onFrame);
      };
      let handle = video.requestVideoFrameCallback(onFrame);
      session.timers.push(() => video.cancelVideoFrameCallback(handle));
    } else {
      const onTime = () => {
        if (core.atTrimEnd(video.currentTime, window_.end)) reachedEnd();
      };
      video.addEventListener("timeupdate", onTime);
      session.timers.push(() => video.removeEventListener("timeupdate", onTime));
    }

    const onEnded = () => reachedEnd();
    video.addEventListener("ended", onEnded);
    session.timers.push(() => video.removeEventListener("ended", onEnded));

    failed.catch((error) => skip(error.message));

    // A stream that stops delivering leaves the picture frozen forever with
    // no event at all; a playing video whose time has not moved is skipped.
    let lastTime = video.currentTime;
    let stillSince = performance.now();
    const watchdog = setInterval(() => {
      const now = performance.now();
      if (video.paused || video.ended || video.currentTime !== lastTime) {
        lastTime = video.currentTime;
        stillSince = now;
      } else if (now - stillSince >= core.STALL_MS) {
        skip(`video stalled for ${core.STALL_MS / 1000} s`);
      }
    }, 1_000);
    session.timers.push(() => clearInterval(watchdog));

    play();
  });
}

async function prepare(item, single) {
  const layer = newLayer(item);
  const build = { image: prepareImage, video: prepareVideo, url: prepareUrl }[item.kind];
  if (!build) {
    layer.remove();
    throw new Error(`unknown kind "${item.kind}"`);
  }
  // The layer goes with a prepare that failed or timed out; one that
  // finishes after its timeout is freed as soon as it does.
  let timedOut = false;
  const building = build(item, layer, single);
  building.then((late) => timedOut && late.teardown(), () => {});
  try {
    return await withTimeout(building, core.PREPARE_TIMEOUT_MS, `not ready after ${core.PREPARE_TIMEOUT_MS / 1000} s`);
  } catch (error) {
    timedOut = true;
    layer.querySelectorAll("video").forEach((video) => {
      video.pause();
      video.removeAttribute("src");
      video.load();
    });
    layer.querySelectorAll("iframe").forEach((frame) => (frame.src = "about:blank"));
    layer.remove();
    throw error;
  }
}

// --- the screen ---------------------------------------------------------

// What is on screen. Shared by every conductor: a playlist switched away
// from stays up until the new one's first item replaces it.
let current = null;
let showing = Promise.resolve();

// Raise a prepared slide over the current one with the incoming item's
// transition, start it, then free the one it covered. Serialised, so two
// shows never interleave their layers. Resolves once the transition is over,
// to `{ done }`, which resolves when the item has finished: wrapped, so the
// chain waits for the transition and never for the item - a playlist
// switched to would otherwise wait for an item of the old one that never
// ends.
function show(next) {
  const run = showing.then(() => transition(next));
  showing = run.catch(() => {});
  return run;
}

async function transition(next) {
  const { item, layer } = next;
  const previous = current;
  const out = previous?.layer;
  const ms = item.transition_ms;
  const kind = ms > 0 ? item.transition : "cut";

  layer.classList.add("incoming");
  // Over the offline page too, which goes once this transition is over.
  const coveringOffline = offline !== null;
  if (coveringOffline) layer.classList.add("over-offline");
  if (kind === "slide") layer.style.transform = "translateX(100%)";
  if (kind === "cut") {
    layer.style.opacity = "1";
  } else {
    if (kind === "slide") layer.style.opacity = "1";
    await nextFrames();
    const easing = kind === "fade" ? "linear" : "ease-in-out";
    const property = kind === "fade" ? "opacity" : "transform";
    layer.style.transition = `${property} ${ms}ms ${easing}`;
    if (kind === "fade") layer.style.opacity = "1";
    else layer.style.transform = "translateX(0)";
    if (kind === "slide" && out) {
      out.style.transition = `transform ${ms}ms ${easing}`;
      out.style.transform = "translateX(-100%)";
    }
  }
  // The item on screen moves with the layers, in one step: state() and the
  // heartbeat never name an item that is not the one shown.
  current = next;
  layer.classList.add("shown");
  out?.classList.remove("shown");

  send(core.startedEvent(next.playlistId, item));
  const done = next.start();

  if (kind !== "cut") {
    // transitionend is the signal; the timeout covers a transition the
    // browser skipped (a layer it never composited).
    await Promise.race([once(layer, "transitionend"), delay(ms + 100)]);
  }
  layer.style.transition = "";
  if (coveringOffline) hideOffline();
  layer.classList.remove("incoming", "over-offline");
  previous?.teardown();
  return { done };
}

// Clear the screen to the page's black: only when a playlist has no items.
function blank() {
  current?.teardown();
  current = null;
}

// The device's offline page over everything, while nothing in the playlist
// can be played: a public screen says something is wrong instead of
// freezing on the last item or staying black. nginx serves it at
// /offline.html from the agent's run directory, where the agent keeps the
// page it stages. The cycle keeps being retried behind it, and the next
// item that is ready comes in over it with its own transition.
let offline = null;

function showOffline() {
  if (offline) return;
  const layer = document.createElement("div");
  layer.className = "layer offline";
  const frame = document.createElement("iframe");
  frame.className = "passive";
  frame.tabIndex = -1;
  frame.setAttribute("aria-hidden", "true");
  layer.appendChild(frame);
  stage.appendChild(layer);
  offline = layer;
  // Shown once loaded, so it never flashes in white half-drawn.
  frame.addEventListener(
    "load",
    () => {
      if (offline === layer) layer.style.opacity = "1";
    },
    { once: true },
  );
  frame.src = "/offline.html";
}

function hideOffline() {
  if (!offline) return;
  const frame = offline.querySelector("iframe");
  if (frame) frame.src = "about:blank";
  offline.remove();
  offline = null;
}

// --- the conductor ------------------------------------------------------

// Plays one playlist. A different playlist gets a new conductor and the old
// one is cancelled; the same playlist changed is handed to replace(), which
// takes effect at the next item boundary.
class Conductor {
  constructor(playlist) {
    this.playlist = playlist;
    this.token = makeToken();
    this.failures = core.newFailures();
    this.position = null; // the item on screen, by position
    this.pending = null; // the next item being prepared
  }

  cancel() {
    this.token.cancel();
    this.pending?.token.cancel();
  }

  // The same playlist with a new item list. The item on screen plays on;
  // the item being prepared after it is kept when it is still the next one,
  // else dropped and the next item taken from the new list.
  replace(playlist) {
    this.playlist = playlist;
    this.failures = core.newFailures();
    if (core.samePending(this.pending?.item, playlist.items, this.position)) return;
    this.pending?.token.cancel();
    this.pending = null;
  }

  get single() {
    return core.singleBehaviour(this.playlist) !== null;
  }

  // Prepare the item after the one on screen, skipping the ones that fail.
  // Resolves to a ready slide, or null once cancelled.
  startPrepare() {
    const token = makeToken(this.token);
    const playlist = this.playlist;
    const items = playlist.items;
    const single = core.singleBehaviour(playlist) !== null;
    // `item` is the one being prepared now, for replace() to compare.
    const entry = { token, ready: false, promise: null, item: null };
    entry.promise = (async () => {
      let index = core.indexAfterPosition(items, this.position);
      while (!token.cancelled) {
        const item = items[index];
        entry.item = item;
        try {
          const ready = await prepare(item, single);
          if (token.cancelled) {
            ready.teardown();
            return null;
          }
          this.failures = core.recordSuccess();
          ready.playlistId = playlist.id;
          entry.ready = true;
          return ready;
        } catch (error) {
          if (token.cancelled) return null;
          send(core.skippedEvent(playlist.id, item, error.message));
          const step = core.recordFailure(this.failures, items.length);
          this.failures = step.failures;
          if (step.report) send(core.nothingPlayableEvent(playlist.id));
          if (core.nothingPlayable(this.failures, items.length)) showOffline();
          if (step.waitMs) await delay(step.waitMs, token);
          index = core.nextIndex(index, items.length);
        }
      }
      return null;
    })();
    return entry;
  }

  async run() {
    const token = this.token;
    let shown = null; // this conductor's slide on screen
    let done = null; // resolves when it is finished
    while (!token.cancelled) {
      if (this.playlist.items.length === 0) {
        blank();
        this.position = null;
        send(core.nothingPlayableEvent(this.playlist.id));
        showOffline();
        return;
      }
      // One ahead: the next item is prepared while this one plays. A
      // single item has no next; it is prepared again only if it ends,
      // which a single video does when it fails.
      if (!this.single && !this.pending) this.pending = this.startPrepare();

      if (done) {
        const result = await Promise.race([done, token.promise]);
        if (token.cancelled) return;
        if (result?.skipped) send(core.skippedEvent(shown.playlistId, shown.item, result.skipped));
      }

      let ready;
      for (;;) {
        const pending = this.pending ?? (this.pending = this.startPrepare());
        // A preparation replace() dropped is not waited out: a prepare
        // only notices it was cancelled when it finishes.
        ready = await Promise.race([
          pending.promise,
          token.promise,
          pending.token.promise.then(() => null),
        ]);
        if (token.cancelled) {
          ready?.teardown();
          return;
        }
        if (pending === this.pending && ready) break;
        // replace() swapped the preparation while it ran: wait for the new.
        ready?.teardown();
        if (pending === this.pending) this.pending = null;
      }
      this.pending = null;
      this.position = ready.item.position;
      shown = ready;
      ({ done } = await show(ready));
    }
  }
}

let conductor = null;

// --- playlist.json ------------------------------------------------------

async function fetchPlaylist() {
  const response = await fetch(SRC, { cache: "no-store" });
  if (!response.ok) throw new Error(`${SRC}: HTTP ${response.status}`);
  return core.normalizePlaylist(await response.json());
}

async function load() {
  let next;
  try {
    next = await fetchPlaylist();
  } catch (error) {
    // No file yet (the agent has not written one) or a broken one: what
    // plays keeps playing.
    console.warn("player: cannot read the playlist", error);
    return;
  }
  if (!next) {
    console.warn("player: not a playlist", SRC);
    return;
  }
  switch (core.reloadOutcome(conductor?.playlist ?? null, next)) {
    case "switch":
      conductor?.cancel();
      conductor = new Conductor(next);
      conductor.run().catch((error) => console.error("player: conductor failed", error));
      break;
    case "boundary":
      conductor.replace(next);
      break;
    default:
      break;
  }
}

// Reloads one at a time, so two quick calls cannot both start a playlist.
let loading = Promise.resolve();
function reload() {
  loading = loading.then(load, load);
  return loading;
}

function state() {
  const item = current?.item ?? null;
  const playlist = conductor?.playlist ?? null;
  return {
    playlist: playlist?.id ?? null,
    name: playlist?.name ?? null,
    items: playlist?.items.length ?? 0,
    position: item?.position ?? null,
    kind: item?.kind ?? null,
    src: item?.source ?? null,
    nextReady: Boolean(conductor?.pending?.ready),
    nothingPlayable: conductor ? core.nothingPlayable(conductor.failures, playlist.items.length) : false,
    offline: offline !== null,
    sinceInputMs: Number.isFinite(lastInput) ? Math.round(performance.now() - lastInput) : null,
  };
}

window.__tessaroPlayer = { reload, input, state };

setInterval(() => {
  send(core.heartbeatEvent(conductor?.playlist.id ?? null, current?.item.position ?? null));
}, core.HEARTBEAT_MS);

if (POLL_MS) setInterval(reload, POLL_MS);

reload();
