import { describe, expect, it } from "vitest";

import { readRefusal } from "./refusal";

describe("readRefusal", () => {
  it("reads the gap after a page restart as a countdown", () => {
    const refusal = readRefusal(new Error("refused: the page was started over 12s ago; try again in 48s"));
    expect(refusal).toMatchObject({ kind: "wait", seconds: 48 });
  });

  it("reads the speed test's gap", () => {
    const refusal = readRefusal(new Error("a speed test ran 30s ago; the next one in 570s"));
    expect(refusal).toMatchObject({ kind: "wait", seconds: 570 });
  });

  it("reads a burst limit as the window to wait out", () => {
    const refusal = readRefusal(new Error("refused: the page printed 5 documents in the last 60s"));
    expect(refusal).toMatchObject({ kind: "wait", seconds: 60 });
  });

  it("tells a call that needs actions mode apart", () => {
    expect(readRefusal(new Error("browser.reload needs browser.bridge.mode actions")).kind).toBe("needs-actions");
  });

  it("passes anything else through", () => {
    expect(readRefusal("printing is off")).toEqual({ kind: "error", message: "printing is off" });
  });
});
