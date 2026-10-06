// Unit tests for player-core.js: `mise run player:test`. Not installed into
// the image; the recipe names the files it ships.

import { test } from "node:test";
import assert from "node:assert/strict";

import * as core from "./player-core.js";

function playlist(id, items) {
  return core.normalizePlaylist({ id, name: id, items });
}

const image = (position, extra = {}) => ({ position, kind: "image", src: `/media-cache/${position}.png`, ...extra });
const video = (position, extra = {}) => ({ position, kind: "video", src: `/media-cache/${position}.mp4`, ...extra });
const url = (position, extra = {}) => ({ position, kind: "url", src: `https://example.test/${position}`, ...extra });

test("normalizePlaylist sorts by position and fills every field", () => {
  const pl = playlist("a", [image(2), { position: 1, kind: "video", src: "v.mp4" }]);
  assert.deepEqual(
    pl.items.map((item) => item.position),
    [1, 2],
  );
  const [v, i] = pl.items;
  assert.equal(v.duration_s, null, "a video is never timed");
  assert.equal(i.duration_s, 10);
  assert.equal(i.fit, "contain");
  assert.equal(i.transition, "cut");
  assert.equal(i.background, "#000000");
  assert.equal(i.source, "/media-cache/2.png", "source falls back to src");
  assert.equal(v.volume, 100);
});

test("normalizePlaylist clamps and rejects values out of range", () => {
  const [item] = playlist("a", [
    image(1, { volume: 250, fit: "tile", transition: "spin", transition_ms: -5, duration_s: 0 }),
  ]).items;
  assert.equal(item.volume, 100);
  assert.equal(item.fit, "contain");
  assert.equal(item.transition, "cut");
  assert.equal(item.transition_ms, 0);
  assert.equal(item.duration_s, 10, "a zero duration would skip the item at once");
});

test("normalizePlaylist refuses what is not a playlist", () => {
  assert.equal(core.normalizePlaylist(null), null);
  assert.equal(core.normalizePlaylist({ items: [] }), null);
  assert.deepEqual(core.normalizePlaylist({ id: "x" }).items, []);
});

test("nextIndex walks the list and wraps", () => {
  assert.equal(core.nextIndex(-1, 3), 0);
  assert.equal(core.nextIndex(0, 3), 1);
  assert.equal(core.nextIndex(2, 3), 0);
  assert.equal(core.nextIndex(0, 1), 0);
  assert.equal(core.nextIndex(0, 0), -1);
});

test("indexAfterPosition carries on after the item on screen", () => {
  const items = playlist("a", [image(1), image(3), image(5)]).items;
  assert.equal(core.indexAfterPosition(items, null), 0);
  assert.equal(core.indexAfterPosition(items, 1), 1);
  assert.equal(core.indexAfterPosition(items, 2), 1, "a position the new list does not have");
  assert.equal(core.indexAfterPosition(items, 5), 0, "wraps after the last");
  assert.equal(core.indexAfterPosition([], 1), -1);
});

test("shouldAdvance waits out the duration", () => {
  const base = { durationS: 10, interactive: false, idleS: 30, sinceInputMs: Infinity };
  assert.equal(core.shouldAdvance({ ...base, elapsedMs: 9_999 }), false);
  assert.equal(core.shouldAdvance({ ...base, elapsedMs: 10_000 }), true);
});

test("shouldAdvance keeps an interactive item while someone uses it", () => {
  const base = { durationS: 10, interactive: true, idleS: 30 };
  assert.equal(core.shouldAdvance({ ...base, elapsedMs: 60_000, sinceInputMs: 5_000 }), false);
  assert.equal(core.shouldAdvance({ ...base, elapsedMs: 60_000, sinceInputMs: 30_000 }), true);
  assert.equal(
    core.shouldAdvance({ ...base, elapsedMs: 5_000, sinceInputMs: Infinity }),
    false,
    "the duration is still the least it stays",
  );
});

test("shouldAdvance never times a video", () => {
  assert.equal(
    core.shouldAdvance({ durationS: null, interactive: false, idleS: 0, elapsedMs: 1e9, sinceInputMs: Infinity }),
    false,
  );
});

test("trimWindow plays the trimmed part, or to the end", () => {
  const [whole, trimmed, pastEnd, empty, late] = playlist("a", [
    video(1),
    video(2, { trim_start_ms: 2_000, trim_end_ms: 5_000 }),
    video(3, { trim_start_ms: 1_000, trim_end_ms: 99_000 }),
    video(4, { trim_start_ms: 5_000, trim_end_ms: 5_000 }),
    video(5, { trim_start_ms: 20_000 }),
  ]).items;
  assert.deepEqual(core.trimWindow(whole, 12), { start: 0, end: 12 });
  assert.deepEqual(core.trimWindow(trimmed, 12), { start: 2, end: 5 });
  assert.deepEqual(core.trimWindow(pastEnd, 12), { start: 1, end: 12 });
  assert.equal(core.trimWindow(empty, 12), null);
  assert.equal(core.trimWindow(late, 12), null, "starts after the video ends");
  assert.deepEqual(core.trimWindow(whole, Infinity), { start: 0, end: Infinity }, "a live stream");
});

test("atTrimEnd stops on the last frame inside the window", () => {
  assert.equal(core.atTrimEnd(4.9, 5, 0), false);
  assert.equal(core.atTrimEnd(4.96, 5, 0.04), true, "the next frame would be past the end");
  assert.equal(core.atTrimEnd(5, 5), true);
  assert.equal(core.atTrimEnd(1e6, Infinity, 0.04), false);
});

test("singleBehaviour: an image or page stays, a video loops", () => {
  assert.equal(core.singleBehaviour(playlist("a", [image(1)])), "stay");
  assert.equal(core.singleBehaviour(playlist("a", [url(1, { interactive: true })])), "stay");
  assert.equal(core.singleBehaviour(playlist("a", [video(1)])), "loop");
  assert.equal(core.singleBehaviour(playlist("a", [image(1), image(2)])), null);
  assert.equal(core.singleBehaviour(null), null);
});

test("reloadOutcome: another playlist switches now", () => {
  const a = playlist("a", [image(1), image(2)]);
  assert.equal(core.reloadOutcome(null, a), "switch");
  assert.equal(core.reloadOutcome(a, playlist("b", [image(1), image(2)])), "switch");
});

test("reloadOutcome: the same playlist changed waits for the boundary", () => {
  const a = playlist("a", [image(1), image(2)]);
  assert.equal(core.reloadOutcome(a, playlist("a", [image(1), image(2, { duration_s: 4 })])), "boundary");
  assert.equal(core.reloadOutcome(a, playlist("a", [image(1)])), "boundary");
});

test("reloadOutcome: nothing changed does nothing", () => {
  const a = playlist("a", [image(1), image(2)]);
  assert.equal(core.reloadOutcome(a, playlist("a", [image(2), image(1)])), "none");
  assert.equal(core.reloadOutcome(a, null), "none", "an unreadable file keeps what plays");
});

test("reloadOutcome: a single item never reaches a boundary, so it switches", () => {
  const a = playlist("url", [url(1, { interactive: true })]);
  assert.equal(core.reloadOutcome(a, playlist("url", [url(1, { interactive: true, src: "https://other.test" })])), "switch");
});

test("failures: a full cycle of them is nothing playable, reported once", () => {
  let failures = core.newFailures();
  let step = core.recordFailure(failures, 3);
  assert.deepEqual([step.report, step.waitMs], [false, 0]);
  step = core.recordFailure(step.failures, 3);
  assert.deepEqual([step.report, step.waitMs], [false, 0]);
  assert.equal(core.nothingPlayable(step.failures, 3), false);
  step = core.recordFailure(step.failures, 3);
  assert.deepEqual([step.report, step.waitMs], [true, core.RETRY_CYCLE_MS]);
  assert.equal(core.nothingPlayable(step.failures, 3), true);

  // The retried cycle fails again: no second report, a wait after each cycle.
  step = core.recordFailure(step.failures, 3);
  assert.deepEqual([step.report, step.waitMs], [false, 0]);
  step = core.recordFailure(step.failures, 3);
  step = core.recordFailure(step.failures, 3);
  assert.deepEqual([step.report, step.waitMs], [false, core.RETRY_CYCLE_MS]);

  failures = core.recordSuccess();
  assert.equal(core.nothingPlayable(failures, 3), false);
  step = core.recordFailure(failures, 1);
  assert.equal(step.report, true, "reported again after something worked");
});

test("failures: one failing item among working ones is only a skip", () => {
  let failures = core.newFailures();
  const step = core.recordFailure(failures, 3);
  failures = core.recordSuccess();
  assert.equal(step.report, false);
  assert.equal(core.nothingPlayable(failures, 3), false);
});

test("events are the JSON the agent reads", () => {
  const [item] = playlist("a", [image(4, { source: "https://cdn.test/x.png" })]).items;
  assert.deepEqual(JSON.parse(core.heartbeatEvent("a", 4)), { event: "heartbeat", playlist: "a", position: 4 });
  assert.deepEqual(JSON.parse(core.heartbeatEvent(null, null)), { event: "heartbeat", playlist: null, position: null });
  assert.deepEqual(JSON.parse(core.startedEvent("a", item)), {
    event: "started",
    playlist: "a",
    position: 4,
    src: "https://cdn.test/x.png",
  });
  assert.deepEqual(JSON.parse(core.skippedEvent("a", item, "image failed to load")), {
    event: "skipped",
    playlist: "a",
    position: 4,
    src: "https://cdn.test/x.png",
    reason: "image failed to load",
  });
  assert.deepEqual(JSON.parse(core.nothingPlayableEvent("a")), { event: "nothing-playable", playlist: "a" });
});

test("samePending keeps the next item only while it is still the next one", () => {
  const items = [
    { position: 1, kind: "image", src: "/a.png" },
    { position: 2, kind: "video", src: "/b.mp4" },
    { position: 3, kind: "image", src: "https://cdn.test/c.png" },
  ];
  const cached = items.map((item) => (item.position === 3 ? { ...item, src: "/media-cache/c.png" } : item));
  assert.equal(core.samePending(items[1], cached, 1), true, "a later item changed");
  const moved = [items[0], items[2], items[1]];
  assert.equal(core.samePending(items[1], moved, 1), false, "another item comes next");
  assert.equal(core.samePending(null, items, 1), false);
});
