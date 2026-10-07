import { describe, expect, it } from "vitest";

import { directionOf, nearest, type Box } from "./focus";

// A grid of tiles, three to a row, 100 wide with gaps of 20:
//   0 1 2
//   3 4 5
const tile = (column: number, row: number): Box => ({ x: column * 120, y: row * 120, w: 100, h: 100 });
const grid = [tile(0, 0), tile(1, 0), tile(2, 0), tile(0, 1), tile(1, 1), tile(2, 1)];

function from(index: number, direction: "up" | "down" | "left" | "right") {
  const others = grid.filter((_, i) => i !== index);
  const found = nearest(grid[index]!, others, direction);
  return found < 0 ? -1 : grid.indexOf(others[found]!);
}

describe("nearest", () => {
  it("moves along a row", () => {
    expect(from(0, "right")).toBe(1);
    expect(from(2, "left")).toBe(1);
  });

  it("moves down the same column, not diagonally", () => {
    expect(from(1, "down")).toBe(4);
    expect(from(5, "up")).toBe(2);
  });

  it("stops at the edge", () => {
    expect(from(2, "right")).toBe(-1);
    expect(from(0, "up")).toBe(-1);
  });

  it("prefers the row's neighbour to a nearer box below it", () => {
    const wide: Box = { x: 0, y: 300, w: 1000, h: 60 };
    const boxes = [tile(1, 0), wide];
    expect(nearest(tile(0, 0), boxes, "right")).toBe(0);
  });
});

describe("directionOf", () => {
  it("reads the arrows a TV remote sends", () => {
    expect(directionOf("ArrowUp")).toBe("up");
    expect(directionOf("Enter")).toBeNull();
  });
});
