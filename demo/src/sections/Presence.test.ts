import { describe, expect, it } from "vitest";

import type { Face } from "../bridge/types";
import { estimate } from "./Presence";

const face: Face = {
  id: 3,
  box: { x: 0.4, y: 0.3, w: 0.1, h: 0.18 },
  score: 0.9,
  distance: 1.2,
  near: true,
  facing: true,
  keypoints: {
    rightEye: [0.43, 0.36],
    leftEye: [0.47, 0.36],
    nose: [0.45, 0.4],
    mouth: [0.45, 0.43],
    rightEar: [0.41, 0.37],
    leftEar: [0.49, 0.37],
  },
};

describe("a face's age and gender on screen", () => {
  it("says nothing until they settle", () => {
    expect(estimate(face)).toBeNull();
  });

  it("names the gender, or says it is unknown", () => {
    expect(estimate({ ...face, age: 34, gender: "female", male: 0.12 })).toBe("female, 34");
    expect(estimate({ ...face, age: 52, gender: "unknown", male: 0.5 })).toBe("gender unknown, 52");
  });
});
