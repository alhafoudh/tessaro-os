import { beforeEach, describe, expect, it } from "vitest";

import { createCecBus, parseData } from "./mock-cec";
import { readRefusal } from "./refusal";
import type { CecDetail } from "./types";

let events: CecDetail[] = [];

beforeEach(() => {
  const target = new EventTarget();
  target.addEventListener("tessaro:cec", (event) => events.push((event as CustomEvent<CecDetail>).detail));
  Object.assign(globalThis, { window: target });
  events = [];
});

function bus(options: { enabled?: boolean; screenOn?: boolean } = {}) {
  return createCecBus({
    enabled: () => options.enabled ?? true,
    pageEvents: () => true,
    screenOn: () => options.screenOn ?? true,
  });
}

describe("the pretend HDMI-CEC bus", () => {
  it("answers a send with the reply it waited for", async () => {
    const acted = await bus().actions.send("8f", 0, 0x90);
    const adapter = acted.adapters[0]!;
    expect(adapter.sent).toEqual([{ to: 0, data: "8f", acked: true }]);
    expect(adapter.reply).toMatchObject({ direction: "in", from: 0, to: 4, data: "90 00" });
  });

  it("fires every message as a message event, named by its opcode", async () => {
    await bus().actions.key("volume-up", 5);
    const messages = events.filter((one) => one.event === "message");
    expect(messages.map((one) => (one.event === "message" ? one.name : null))).toEqual([
      "user-control-pressed",
      "user-control-released",
    ]);
  });

  it("keeps a log read after a seq", async () => {
    const { actions } = bus();
    await actions.standby();
    await actions.source();
    const all = await actions.messages();
    const later = await actions.messages(all.messages[0]!.seq);
    expect(later.messages).toHaveLength(all.messages.length - 1);
    expect(later.next).toBe(all.next);
  });

  it("reports who answered a scan", async () => {
    const acted = await bus().actions.scan();
    expect(acted.adapters[0]!.answered).toEqual([0, 5]);
  });

  it("refuses the page's 21st action in 10s as something to wait out", async () => {
    const { actions } = bus();
    for (let i = 0; i < 20; i += 1) await actions.source();
    const refused = await actions.source().catch((error: unknown) => error);
    expect(readRefusal(refused)).toMatchObject({ kind: "wait", seconds: 10 });
  });

  it("refuses to wake the TV while the screen is off, and everything while CEC is off", async () => {
    await expect(bus({ screenOn: false }).actions.wake()).rejects.toThrow(/screen is off/);
    await expect(bus({ enabled: false }).actions.scan()).rejects.toThrow(/HDMI-CEC is off/);
  });

  it("reads hex the way the device does", () => {
    expect(parseData("0x44:0x41")).toEqual([0x44, 0x41]);
    expect(parseData("4441")).toEqual([0x44, 0x41]);
    expect(() => parseData("zz")).toThrow(/not hex bytes/);
  });
});
