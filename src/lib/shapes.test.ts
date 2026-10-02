// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
import { describe, expect, it } from "vitest";

import {
  chaikinOpen,
  decimatePoints,
  ellipseShape,
  lineShape,
  polygonFillCells,
  polygonOutlineCells,
  rectShape,
  roundCorners,
  roundedRectShape,
  roundedTriangleShape,
  shapeCells,
  smoothPathCells,
  triangleShape,
} from "./shapes";
import type { StrokeCell } from "./types";

const ids = (cells: StrokeCell[]) => new Set(cells.map((c) => `${c.x},${c.y}`));

describe("rectShape", () => {
  it("draws only the rim unless asked to fill", () => {
    const rim = ids(rectShape({ x: 1, y: 1 }, { x: 4, y: 3 }));
    expect(rim.size).toBe(10); // 4 列 x 3 行 = 12，减去 2x1 的内部
    expect(rim.has("2,2")).toBe(false);
    const solid = ids(rectShape({ x: 1, y: 1 }, { x: 4, y: 3 }, true));
    expect(solid.size).toBe(12);
    expect(solid.has("2,2")).toBe(true);
  });

  it("survives dragging backwards from the anchor", () => {
    const up = ids(rectShape({ x: 4, y: 3 }, { x: 1, y: 1 }));
    expect(up.has("1,1")).toBe(true);
    expect(up.has("4,3")).toBe(true);
  });
});

describe("ellipseShape", () => {
  it("inscribes in the drag box and closes the rim", () => {
    const rim = ids(ellipseShape({ x: 0, y: 0 }, { x: 6, y: 4 }));
    expect(rim.has("0,2")).toBe(true); // 最左一列的中线
    expect(rim.has("6,2")).toBe(true); // 最右
    expect(rim.has("3,0")).toBe(true); // 最上
    expect(rim.has("3,4")).toBe(true); // 最下
    // 中心必须是空的：描边不是实心。
    expect(rim.has("3,2")).toBe(false);
  });

  it("fills the interior when asked", () => {
    const solid = ids(ellipseShape({ x: 0, y: 0 }, { x: 6, y: 4 }, true));
    expect(solid.has("3,2")).toBe(true);
  });

  it("degenerates to a dot instead of dividing by zero", () => {
    const cells = ellipseShape({ x: 3, y: 3 }, { x: 3, y: 3 });
    expect(cells.length).toBeGreaterThan(0);
    expect(cells.every((c) => Number.isFinite(c.x) && Number.isFinite(c.y))).toBe(true);
  });
});

describe("triangleShape", () => {
  it("puts the apex on the top edge midpoint", () => {
    const rim = ids(triangleShape({ x: 0, y: 0 }, { x: 6, y: 4 }));
    expect(rim.has("3,0")).toBe(true);
    expect(rim.has("0,4")).toBe(true);
    expect(rim.has("6,4")).toBe(true);
    // 三条边都接了头：顶角和两个底角各出现一次连通过程。
    for (const corner of ["3,0", "0,4", "6,4"]) {
      const near = touchesNeighborhood(rim, corner);
      expect(near).toBe(true);
    }
  });

  it("fills with the even-odd scanline", () => {
    const solid = ids(triangleShape({ x: 0, y: 0 }, { x: 6, y: 4 }, true));
    expect(solid.has("3,2")).toBe(true);
    // 越往下越宽，底边那一行必须整条填满。
    expect(solid.has("0,4")).toBe(true);
    expect(solid.has("6,4")).toBe(true);
  });
});

// 角点值不值的断言写不出信息量，换个角度：每个角至少有一个上下左右邻居。
function touchesNeighborhood(set: Set<string>, at: string): boolean {
  const [x, y] = at.split(",").map(Number);
  const deltas = [
    [1, 0],
    [-1, 0],
    [0, 1],
    [0, -1],
    [1, 1],
    [1, -1],
    [-1, 1],
    [-1, -1],
  ];
  return deltas.some(([dx, dy]) => set.has(`${x + dx},${y + dy}`));
}

describe("polygonFillCells", () => {
  it("fills a bowtie by the even-odd rule, not by convex hull", () => {
    // 自相交的领结：奇偶规则下中间那一块是「外面」。
    const cells = polygonFillCells([
      { x: 0, y: 0 },
      { x: 4, y: 4 },
      { x: 4, y: 0 },
      { x: 0, y: 4 },
    ]);
    const set = ids(cells);
    expect(set.has("0,0")).toBe(true);
    expect(set.has("4,4")).toBe(true);
    // 交叉点在 (2,2)：奇偶扫描线穿过两次同色，中间那格应判为内部两侧。
    expect(set.has("2,2")).toBe(true);
  });

  it("needs at least three points", () => {
    expect(polygonFillCells([{ x: 0, y: 0 }, { x: 1, y: 1 }])).toEqual([]);
  });
});

describe("smoothing", () => {
  it("drops jitter points but keeps both ends", () => {
    const pts: StrokeCell[] = [
      { x: 0, y: 0 },
      { x: 1, y: 1 },
      { x: 1, y: 2 },
      { x: 6, y: 6 },
      { x: 6, y: 7 },
    ];
    const cut = decimatePoints(pts, 2);
    expect(cut[0]).toEqual({ x: 0, y: 0 });
    expect(cut[cut.length - 1]).toEqual({ x: 6, y: 7 });
    expect(cut.length).toBeLessThan(pts.length);
  });

  it("chaikin keeps the endpoints and softens the corner", () => {
    const pts: StrokeCell[] = [
      { x: 0, y: 0 },
      { x: 4, y: 0 },
      { x: 4, y: 8 },
    ];
    const soft = chaikinOpen(pts, 2);
    expect(soft[0]).toEqual({ x: 0, y: 0 });
    expect(soft[soft.length - 1]).toEqual({ x: 4, y: 8 });
    // 直角被削掉了：折线中点不再只有一条竖边。
    const anyCorner = soft.some((p) => p.x < 4 && p.y > 0 && p.y < 8);
    expect(anyCorner).toBe(true);
  });

  it("short-circuits on two-point paths", () => {
    expect(chaikinOpen([{ x: 0, y: 0 }, { x: 2, y: 2 }])).toHaveLength(2);
    expect(
      smoothPathCells([
        { x: 0, y: 0 },
        { x: 2, y: 2 },
      ]),
    ).toEqual(polygonOutlineCells([{ x: 0, y: 0 }, { x: 2, y: 2 }]));
  });

  it("smooths a wobbly drag into a continuous run of cells", () => {
    const wobble: StrokeCell[] = [];
    for (let i = 0; i <= 20; i += 1) {
      wobble.push({ x: i, y: 10 + (i % 2 === 0 ? 0 : 2) });
    }
    const cells = smoothPathCells(wobble, 3);
    expect(cells.length).toBeGreaterThan(20);
    // 每个相邻格子都必须八邻域相接：一条断掉的曲线在画布上就是虚线。
    for (let i = 1; i < cells.length; i += 1) {
      const dx = Math.abs(cells[i].x - cells[i - 1].x);
      const dy = Math.abs(cells[i].y - cells[i - 1].y);
      expect(dx <= 1 && dy <= 1).toBe(true);
    }
  });
});

describe("lineShape", () => {
  it("forwards to bresenham", () => {
    expect(lineShape({ x: 0, y: 0 }, { x: 2, y: 2 })).toEqual([
      { x: 0, y: 0 },
      { x: 1, y: 1 },
      { x: 2, y: 2 },
    ]);
  });
});

describe("roundCorners", () => {
  it("cuts the corner away but keeps every original vertex inside", () => {
    const ring = [
      { x: 0, y: 0 },
      { x: 8, y: 0 },
      { x: 8, y: 8 },
      { x: 0, y: 8 },
    ];
    const rounded = roundCorners(ring, 3);
    // 顶点(0,0)被让掉了：圆弧上不该再出现它自己。
    expect(rounded.some((p) => p.x === 0 && p.y === 0)).toBe(false);
    // 每段弧的两个切点必须落在原来的边上：四个角就是八个切点，
    // 形状才没有被整体削小一圈。弧内部那些点离边都有间距，不该混进来。
    const tangents = rounded.filter((p) => [0, 8].includes(p.x) || [0, 8].includes(p.y));
    expect(tangents).toHaveLength(8);
  });

  it("degenerates to the input when the radius is zero or tiny", () => {
    const ring = [
      { x: 0, y: 0 },
      { x: 6, y: 0 },
      { x: 6, y: 6 },
    ];
    expect(roundCorners(ring, 0)).toEqual(ring);
    expect(roundCorners(ring, -1)).toEqual(ring);
  });

  it("never lets neighbouring corners cross on a very small shape", () => {
    const tiny = [
      { x: 0, y: 0 },
      { x: 2, y: 0 },
      { x: 2, y: 2 },
      { x: 0, y: 2 },
    ];
    // 半径比整条边还大：夹住之后最多退成圆头，不能出现坐标跑到框外的点。
    for (const p of roundCorners(tiny, 10)) {
      expect(p.x).toBeGreaterThanOrEqual(-1e-9);
      expect(p.x).toBeLessThanOrEqual(2 + 1e-9);
      expect(p.y).toBeGreaterThanOrEqual(-1e-9);
      expect(p.y).toBeLessThanOrEqual(2 + 1e-9);
    }
  });
});

describe("roundedRectShape", () => {
  it("falls back to the hard-corner rect at radius zero", () => {
    expect(ids(roundedRectShape({ x: 1, y: 1 }, { x: 4, y: 3 }, false, 0))).toEqual(
      ids(rectShape({ x: 1, y: 1 }, { x: 4, y: 3 })),
    );
  });

  it("removes the corner pixel once the radius is on", () => {
    const hard = ids(rectShape({ x: 0, y: 0 }, { x: 9, y: 9 }));
    const soft = ids(roundedRectShape({ x: 0, y: 0 }, { x: 9, y: 9 }, false, 0.3));
    expect(hard.has("0,0")).toBe(true);
    expect(soft.has("0,0")).toBe(false);
    // 圆弧自身要留下格子，圆角不能圆成一条空边。
    expect(soft.size).toBeGreaterThan(hard.size - 12);
  });

  it("keeps the fill solid after rounding", () => {
    const solid = ids(roundedRectShape({ x: 0, y: 0 }, { x: 9, y: 9 }, true, 0.3));
    expect(solid.has("4,4")).toBe(true);
    expect(solid.has("0,0")).toBe(false); // 圆角处让开了
  });
});

describe("roundedTriangleShape", () => {
  it("rounds the apex as well as the two base corners", () => {
    const hard = ids(triangleShape({ x: 0, y: 0 }, { x: 10, y: 10 }));
    const soft = ids(roundedTriangleShape({ x: 0, y: 0 }, { x: 10, y: 10 }, false, 0.3));
    expect(hard.has("5,0")).toBe(true); // 顶角
    expect(soft.has("5,0")).toBe(false); // 顶角也让圆了
    expect(soft.has("5,3")).toBe(true); // 弧的顶点仍然有格子，没圆成空心的
  });

  it("keeps the same handedness as the hard triangle", () => {
    const soft = ids(roundedTriangleShape({ x: 0, y: 0 }, { x: 10, y: 10 }, true, 0.25));
    expect(soft.has("5,8")).toBe(true);
  });
});

describe("shapeCells", () => {
  it("dispatches every anchor-based shape and passes the radius through", () => {
    expect(shapeCells("line", { x: 0, y: 0 }, { x: 5, y: 0 })).toEqual(
      lineShape({ x: 0, y: 0 }, { x: 5, y: 0 }),
    );
    expect(shapeCells("rect", { x: 0, y: 0 }, { x: 9, y: 9 }, { radius: 0.3 })).toEqual(
      roundedRectShape({ x: 0, y: 0 }, { x: 9, y: 9 }, false, 0.3),
    );
    expect(shapeCells("triangle", { x: 0, y: 0 }, { x: 9, y: 9 }, { filled: true })).toEqual(
      triangleShape({ x: 0, y: 0 }, { x: 9, y: 9 }, true),
    );
    // 平滑曲线不吃锚点，走调用点自己的采样分支。
    expect(shapeCells("smooth", { x: 0, y: 0 }, { x: 3, y: 3 })).toEqual([]);
  });
});
