// What the Playlists page builds from what was typed, as the tests of
// agent/client/src/playlist.rs check it; the words are the fixtures'.

import { describe, expect, it } from "vitest";

import {
  DURATION_DEFAULT_S,
  editItem,
  entryChange,
  entrySpec,
  itemFromFields,
  parseDays,
  type ItemFields,
} from "./playlist";

function typed(kind: string, src: string): ItemFields {
  return { kind, src };
}

describe("an item from what was typed", () => {
  it("takes the trim, the sound and the fit of a video", () => {
    const item = itemFromFields(
      { ...typed("video", "https://cdn.test/promo.mp4"), from: "5s", to: "0:10", volume: "80", fit: "cover" },
      null,
    );
    expect(item.trim_start_ms).toBe(5000);
    expect(item.trim_end_ms).toBe(10_000);
    expect(item.sound).toBe(true);
    expect(item.volume).toBe(80);
    expect(item.fit).toBe("cover");
    expect(item.duration_s).toBeNull();
  });

  it("gives a page or an image the default duration", () => {
    expect(itemFromFields(typed("url", "https://menu.test/"), 1).duration_s).toBe(DURATION_DEFAULT_S);
  });

  it("is refused as the device would", () => {
    const fields = { ...typed("image", "https://cdn.test/a.png"), sound: true };
    expect(() => itemFromFields(fields, null)).toThrow(/^the new item: /);
    expect(() => itemFromFields(fields, 3)).toThrow(/^item 3: /);
    expect(() => itemFromFields(typed("gif", "https://x.test/"), null)).toThrow();
    expect(() => itemFromFields(typed("url", " "), null)).toThrow();
  });

  it("changes only what an edit gives", () => {
    const item = itemFromFields(
      { ...typed("url", "https://menu.test/"), duration: "30s", idle: "45s", bridge: true },
      1,
    );
    expect(item.interactive).toBe(true);
    const edited = editItem(item, { src: "https://menu.test/today", interactive: false }, 1);
    expect(edited.src).toBe("https://menu.test/today");
    expect(edited.duration_s).toBe(30);
    expect(edited.interactive).toBe(false);
    expect(edited.idle_s).toBeNull();
    expect(edited.bridge).toBe(true);
    expect(editItem(item, { duration: "" }, 1).duration_s).toBe(DURATION_DEFAULT_S);
  });

  it("drops what a changed kind no longer has", () => {
    const page = itemFromFields({ ...typed("url", "https://menu.test/"), bridge: true, readyDelay: "500ms" }, 1);
    const video = editItem(page, { kind: "video", src: "https://cdn.test/a.mp4" }, 1);
    expect(video.duration_s).toBeNull();
    expect(video.bridge).toBe(false);
    expect(video.ready_delay_ms).toBeNull();
  });
});

describe("a timetable entry from what was typed", () => {
  it("reads days and times the one way", () => {
    expect(entrySpec("lunch", "mon-fri", "11:30", "14:00", 0, true).days).toEqual(["mon", "tue", "wed", "thu", "fri"]);
    expect(parseDays("fri-mon")).toEqual(["mon", "fri", "sat", "sun"]);
    expect(() => entrySpec("lunch", "", "24:00", "01:00", 0, true)).toThrow();
    expect(() => entrySpec("lunch", "someday", "11:30", "14:00", 0, true)).toThrow();
  });

  it("changes what is given", () => {
    const change = entryChange({ from: "9:05", days: "all" });
    expect(change.from).toBe("09:05");
    expect(change.days).toEqual([]);
    expect(() => entryChange({})).toThrow();
    expect(() => entryChange({ to: "25:00" })).toThrow();
  });
});
