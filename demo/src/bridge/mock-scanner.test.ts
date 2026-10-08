import { beforeEach, describe, expect, it } from "vitest";

import { base64, createScanner, SAMPLES } from "./mock-scanner";
import type { ScannerDetail } from "./types";

let events: ScannerDetail[] = [];

beforeEach(() => {
  const target = new EventTarget();
  target.addEventListener("tessaro:scanner", (event) => events.push((event as CustomEvent<ScannerDetail>).detail));
  Object.assign(globalThis, { window: target });
  events = [];
});

function scanner(options: { enabled?: boolean; page?: boolean } = {}) {
  return createScanner({
    enabled: () => options.enabled ?? true,
    pageEvents: () => options.page ?? true,
    perChar: 0,
  });
}

describe("the pretend scanner", () => {
  it("fires begin, then end with the text, its bytes and its length", async () => {
    const end = await scanner().scan("Žltá 42");
    expect(events.map((one) => one.event)).toEqual(["begin", "end"]);
    expect(end).toMatchObject({ event: "end", scanner: "counter", text: "Žltá 42", length: 9 });
    if (end.event === "end") expect(atob(end.bytes)).toHaveLength(9);
  });

  it("scans the samples in turn, the GS1 one with its group separators", async () => {
    const one = scanner();
    await one.scan();
    const gs1 = await one.scan();
    expect(gs1.event === "end" && gs1.text).toBe(SAMPLES[1]!.text);
    expect(SAMPLES[1]!.text).toContain("\u001d");
  });

  it("counts its scans in list()", async () => {
    const one = scanner();
    await one.scan();
    const list = await one.list();
    expect(list.scanners[0]).toMatchObject({ state: "reading", scans: 1 });
  });

  it("tells the page nothing while scanner.enable or scanner.page is off", async () => {
    await scanner({ page: false }).scan("x");
    await scanner({ enabled: false }).scan("x");
    expect(events).toEqual([]);
    expect((await scanner({ enabled: false }).list()).scanners[0]!.state).toBe("disabled");
  });

  it("encodes bytes as UTF-8 base64", () => {
    expect(base64("é")).toBe("w6k=");
  });
});
