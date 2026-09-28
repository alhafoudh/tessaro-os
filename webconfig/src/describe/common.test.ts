import { describe, expect, it } from "vitest";

import { fixed } from "./common";

describe("fixed", () => {
  it("writes decimals as Rust's {:.N} does", () => {
    expect(fixed(1.25, 1)).toBe("1.2");
    expect(fixed(1.35, 1)).toBe("1.4");
    expect(fixed(0.5, 0)).toBe("0");
    expect(fixed(1.5, 0)).toBe("2");
    expect(fixed(2.5, 0)).toBe("2");
    expect(fixed(1.234, 1)).toBe("1.2");
    expect(fixed(0.15, 1)).toBe("0.1");
    expect(fixed(9.95, 1)).toBe("9.9");
    expect(fixed(9.75, 1)).toBe("9.8");
    expect(fixed(-1.25, 1)).toBe("-1.2");
    expect(fixed(800, 1)).toBe("800.0");
  });
});
