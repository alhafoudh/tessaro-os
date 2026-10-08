import { describe, expect, it } from "vitest";

import { visible } from "./Scanner";

describe("a scan on screen", () => {
  it("shows GS1's group separators and the other control characters", () => {
    expect(visible("0108586\u001d17271231\r")).toBe("0108586⟨GS⟩17271231⟨CR⟩");
    expect(visible("a\u0003b")).toBe("a⟨0x03⟩b");
    expect(visible("plain")).toBe("plain");
  });
});
