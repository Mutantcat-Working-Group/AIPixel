import { describe, expect, it } from "vitest";

import { appendStroke, lineCells } from "./stroke";

describe("lineCells", () => {
  it("connects two cells in a straight run", () => {
    expect(lineCells({ x: 1, y: 3 }, { x: 4, y: 3 })).toEqual([
      { x: 1, y: 3 },
      { x: 2, y: 3 },
      { x: 3, y: 3 },
      { x: 4, y: 3 },
    ]);
  });

  it("moves diagonally without leaving a gap", () => {
    expect(lineCells({ x: 0, y: 0 }, { x: 2, y: 2 })).toEqual([
      { x: 0, y: 0 },
      { x: 1, y: 1 },
      { x: 2, y: 2 },
    ]);
  });

  it("covers both axes when the slope is uneven", () => {
    const cells = lineCells({ x: 0, y: 0 }, { x: 3, y: 1 });
    expect(cells[0]).toEqual({ x: 0, y: 0 });
    expect(cells[cells.length - 1]).toEqual({ x: 3, y: 1 });
    // 每一步都在向终点靠近，不回头。
    for (let i = 1; i < cells.length; i += 1) {
      expect(cells[i].x).toBeGreaterThan(cells[i - 1].x);
      expect(Math.abs(cells[i].y - cells[i - 1].y)).toBeLessThanOrEqual(1);
    }
  });

  it("handles a cell clicked once", () => {
    expect(lineCells({ x: 5, y: 5 }, { x: 5, y: 5 })).toEqual([{ x: 5, y: 5 }]);
  });
});

describe("appendStroke", () => {
  it("skips the seam cell so a drag does not double-paint it", () => {
    const cells = appendStroke([{ x: 0, y: 0 }], lineCells({ x: 0, y: 0 }, { x: 0, y: 2 }));
    expect(cells).toEqual([{ x: 0, y: 0 }, { x: 0, y: 1 }, { x: 0, y: 2 }]);
  });

  it("keeps every cell when the new run touches nothing painted", () => {
    const cells = appendStroke([{ x: 3, y: 3 }], lineCells({ x: 7, y: 7 }, { x: 7, y: 8 }));
    expect(cells).toEqual([
      { x: 3, y: 3 },
      { x: 7, y: 7 },
      { x: 7, y: 8 },
    ]);
  });

  it("never grows unbounded when the pointer idles in place", () => {
    let cells: { x: number; y: number }[] = [];
    for (let i = 0; i < 50; i += 1) {
      cells = appendStroke(cells, lineCells({ x: 2, y: 2 }, { x: 2, y: 2 }));
    }
    expect(cells).toEqual([{ x: 2, y: 2 }]);
  });
});
