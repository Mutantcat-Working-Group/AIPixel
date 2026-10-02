// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
// 形状与平滑的几何：把指针的两个端点（或一串采样）变成格子链。
// 纯函数、零依赖，采样在组件里做，这里只算形状。
//
// 为什么不用 canvas 的 lineWidth/quadraticCurveTo：那一套是抗锯齿连续坐标的
// 思路，画完拿到的是「一片有浓淡的像素」，而我们要的是「哪些格子落文档」。
// 格子必须能被 Rust 侧原样吃下去，所以只能在这里自己决定每个 (x, y)。
//
// 用户提的需求很朴素：「基础图形有时候太突兀，画细节要能平滑曲线」。所以
// 每一族都留两种口味：硬边（矩形/三角/椭圆）和顺手（Chaikin + Catmull-Rom）。

import type { StrokeCell } from "./types";
import { lineCells } from "./stroke";

/** 形状档位。none = 自由笔，其余都是「按下定锚点、拖动看结果、抬笔落文档」。 */
export type ShapeKind = "line" | "rect" | "ellipse" | "triangle" | "smooth";

const key = (x: number, y: number) => `${x},${y}`;

/**
 * 用一个格子集合反推「轮廓」：集合里但凡有上下左右邻居不在集合内，就算边。
 *
 * 这是所有闭合形状共用的收边办法，比给每种形状单独写 scanline 判定稳：
 * 椭圆先把「实心集合」算出来，再减一次内芯，边就是那圈差集，圆角处
 * 不会出现一行两个端点的锯齿台阶。
 */
function outlineOf(filled: Map<string, StrokeCell>): StrokeCell[] {
  const edge: StrokeCell[] = [];
  for (const cell of filled.values()) {
    const surrounded =
      filled.has(key(cell.x, cell.y - 1)) &&
      filled.has(key(cell.x, cell.y + 1)) &&
      filled.has(key(cell.x - 1, cell.y)) &&
      filled.has(key(cell.x + 1, cell.y));
    if (!surrounded) edge.push(cell);
  }
  return edge;
}

/** 两点之间的轴对齐包围盒。 */
function bounds(a: StrokeCell, b: StrokeCell) {
  return {
    x0: Math.min(a.x, b.x),
    x1: Math.max(a.x, b.x),
    y0: Math.min(a.y, b.y),
    y1: Math.max(a.y, b.y),
  };
}

/** 直线：Bresenham 的责任在 stroke.ts 那边，这里只做转发，调用点少一个 import。 */
export function lineShape(a: StrokeCell, b: StrokeCell): StrokeCell[] {
  return lineCells(a, b);
}

/** 矩形：描边，或实心。像素画布上的方形必须是主轴对齐的，歪的没有意义。 */
export function rectShape(a: StrokeCell, b: StrokeCell, filled = false): StrokeCell[] {
  const { x0, x1, y0, y1 } = bounds(a, b);
  const out: StrokeCell[] = [];
  const push = (x: number, y: number) => out.push({ x, y });
  for (let y = y0; y <= y1; y += 1) {
    for (let x = x0; x <= x1; x += 1) {
      if (filled || x === x0 || x === x1 || y === y0 || y === y1) push(x, y);
    }
  }
  return out;
}

/**
 * 椭圆：包围盒内接。先攒实心集合再减内芯出边，和「每行算两个 x 端点」
 * 的老办法相比，圆角处不会出现锯齿台阶，格子密度也均匀。
 */
export function ellipseShape(a: StrokeCell, b: StrokeCell, filled = false): StrokeCell[] {
  const { x0, x1, y0, y1 } = bounds(a, b);
  const cx = (x0 + x1) / 2;
  const cy = (y0 + y1) / 2;
  const rx = Math.max((x1 - x0) / 2, 0.5);
  const ry = Math.max((y1 - y0) / 2, 0.5);
  const filled_cells = new Map<string, StrokeCell>();
  for (let y = y0; y <= y1; y += 1) {
    for (let x = x0; x <= x1; x += 1) {
      const dx = (x - cx) / rx;
      const dy = (y - cy) / ry;
      if (dx * dx + dy * dy <= 1.0001) filled_cells.set(key(x, y), { x, y });
    }
  }
  return filled ? [...filled_cells.values()] : outlineOf(filled_cells);
}

/**
 * 三角形：包围盒内接，顶角在上边中点，底边两角贴下边两端。
 * 等腰而不是直角三角形：像素画里斜边和格子对不齐，歪斜感来自菱形对角线。
 */
export function triangleShape(a: StrokeCell, b: StrokeCell, filled = false): StrokeCell[] {
  const { x0, x1, y0, y1 } = bounds(a, b);
  const apex = { x: Math.round((x0 + x1) / 2), y: y0 };
  const left = { x: x0, y: y1 };
  const right = { x: x1, y: y1 };
  if (filled) return polygonFillCells([apex, left, right]);
  return polygonOutlineCells([apex, right, left, apex]);
}

/** 按顺序把折线连成格子链：首尾相接，不自己闭合（闭合由调用点多传一个点）。 */
export function polygonOutlineCells(points: StrokeCell[]): StrokeCell[] {
  if (points.length < 2) return points.slice();
  // 浮点必须先取整再连线：线段的两个端点是 0.5 这种坐标时，整数步进的连线
  // 永远差一点才到终点。取整放在这一层，调用点（圆角、平滑）就能一直用浮点算几何。
  const grid = points.map((p) => ({ x: Math.round(p.x), y: Math.round(p.y) }));
  const out: StrokeCell[] = [];
  for (let i = 1; i < grid.length; i += 1) {
    out.push(...lineCells(grid[i - 1], grid[i]));
  }
  return out;
}

/**
 * 多边形内部：y 扫描线 + 奇偶规则。
 *
 * 边界取闭区间 [minY, maxY]：半开区间会在「底边是水平边」的三角形上漏掉
 * 整整一行底像素。共享顶点两侧都记一次，极值行那两个 x 凑成的正好是
 * 一个点（尖角）或一条完整的底边。
 */
export function polygonFillCells(points: StrokeCell[]): StrokeCell[] {
  if (points.length < 3) return [];
  let y0 = Infinity;
  let y1 = -Infinity;
  for (const p of points) {
    y0 = Math.min(y0, p.y);
    y1 = Math.max(y1, p.y);
  }
  // 扫描线必须走在整数行上：顶点是浮点（圆角弧、平滑曲线都先算浮点）时，
  // 直接拿 3.09 起步会让每一行都偏移小数位，填充整体错行且边上缺一格。
  y0 = Math.floor(y0);
  y1 = Math.ceil(y1);
  const set = new Map<string, StrokeCell>();
  for (let y = y0; y <= y1; y += 1) {
    const xs: number[] = [];
    for (let i = 0; i < points.length; i += 1) {
      const p = points[i];
      const q = points[(i + 1) % points.length];
      if (p.y === q.y) continue;
      const lo = Math.min(p.y, q.y);
      const hi = Math.max(p.y, q.y);
      if (y < lo || y > hi) continue;
      const x = p.x + ((y - p.y) * (q.x - p.x)) / (q.y - p.y);
      xs.push(Math.round(x));
    }
    xs.sort((m, n) => m - n);
    for (let i = 0; i + 1 < xs.length; i += 2) {
      for (let x = xs[i]; x <= xs[i + 1]; x += 1) {
        set.set(key(x, y), { x, y });
      }
    }
  }
  return [...set.values()];
}

/** 去掉采样里的抖动点：距离上一个保留点太近就丢，端点不动。 */
export function decimatePoints(points: StrokeCell[], minDist = 1.6): StrokeCell[] {
  if (points.length < 3) return points.slice();
  const out: StrokeCell[] = [points[0]];
  for (let i = 1; i < points.length - 1; i += 1) {
    const p = points[i];
    const last = out[out.length - 1];
    if (Math.hypot(p.x - last.x, p.y - last.y) < minDist) continue;
    out.push(p);
  }
  out.push(points[points.length - 1]);
  return out;
}

/**
 * Chaikin 切角：折线每轮把每个角削掉四分之一，rounds 轮之后接近平滑。
 *
 * 端点单独保留（末点只往里推半步）：闭合形状切着切着会缩成一点，
 * 自由曲线切掉端点就成了缺口的弧，肉眼立刻看出来。
 */
export function chaikinOpen(points: StrokeCell[], rounds = 2): StrokeCell[] {
  if (points.length < 3 || rounds <= 0) return points.slice();
  let pts = points.slice();
  for (let r = 0; r < rounds; r += 1) {
    const next: StrokeCell[] = [pts[0]];
    for (let i = 1; i < pts.length; i += 1) {
      const p = pts[i - 1];
      const q = pts[i];
      next.push({ x: p.x + 0.25 * (q.x - p.x), y: p.y + 0.25 * (q.y - p.y) });
      next.push({ x: p.x + 0.75 * (q.x - p.x), y: p.y + 0.75 * (q.y - p.y) });
    }
    next.push(pts[pts.length - 1]);
    pts = next;
  }
  return pts;
}

/**
 * Catmull-Rom 采样加密：穿过每个采样点的插值曲线。
 *
 * 首尾各复制一个点做虚拟邻居，否则首末段的切线和中间不一样，起笔收笔
 * 会莫名翘一下；最后一段多采一个点，避免末点被丢。
 */
export function catmullRomPoints(points: StrokeCell[], samples = 6): StrokeCell[] {
  if (points.length < 3) return points.slice();
  const pts = [points[0], ...points, points[points.length - 1]];
  const out: StrokeCell[] = [];
  for (let i = 1; i + 2 < pts.length; i += 1) {
    const p0 = pts[i - 1];
    const p1 = pts[i];
    const p2 = pts[i + 1];
    const p3 = pts[i + 2];
    const isLast = i + 3 >= pts.length;
    const end = isLast ? samples : samples - 1;
    for (let s = 0; s <= end; s += 1) {
      const t = s / samples;
      const t2 = t * t;
      const t3 = t2 * t;
      out.push({
        x:
          0.5 *
          (2 * p1.x +
            (-p0.x + p2.x) * t +
            (2 * p0.x - 5 * p1.x + 4 * p2.x - p3.x) * t2 +
            (-p0.x + 3 * p1.x - 3 * p2.x + p3.x) * t3),
        y:
          0.5 *
          (2 * p1.y +
            (-p0.y + p2.y) * t +
            (2 * p0.y - 5 * p1.y + 4 * p2.y - p3.y) * t2 +
            (-p0.y + 3 * p1.y - 3 * p2.y + p3.y) * t3),
      });
    }
  }
  return out;
}

/**
 * 平滑曲线的完整链路：采样点 -> 去抖 -> Chaikin 削角 -> 加密采样 -> 格子。
 *
 * Chaikin 直接作用在格子上会把圆角啃出毛刺，所以先走浮点再做一次
 * Catmull-Rom 加密，最后才四舍五入回格子：曲线既顺，格子又连续不断。
 */
export function smoothPathCells(points: StrokeCell[], rounds = 2): StrokeCell[] {
  const usable = points.length >= 3 ? decimatePoints(points) : points;
  if (usable.length < 3) return polygonOutlineCells(usable);
  const smoothed = chaikinOpen(usable, rounds);
  const dense = catmullRomPoints(smoothed, 6);
  const rounded = dense.map((p) => ({ x: Math.round(p.x), y: Math.round(p.y) }));
  return polygonOutlineCells(rounded);
}


/**
 * 把闭合多边形的每个角让成圆弧。
 *
 * 为什么不用 Chaikin：Chaikin 是「不断切角」的极限思路，作用在闭合环上会
 * 整体往重心缩，矩形切两轮就小一圈，而且尖角永远留一点点。这里改成切线的
 * 解析做法——角两边各退 t = radius / tan(θ/2)，中间按真圆弧采样——形状
 * 不缩、圆的大小可控，三角和矩形走同一条路。
 *
 * 半径按「最短邻边的一半」夹住：拖到只有两三格大的形状还想要大圆角，
 * 两边会交叉成蝶形，夹住之后至多退成圆头三角，不会出现自相交。
 */
export function roundCorners(points: StrokeCell[], radius: number, arc = 4): StrokeCell[] {
  if (points.length < 3 || !(radius > 0)) return points.slice();
  const out: StrokeCell[] = [];
  // sin/cos 算出来的「本该是整数」的坐标常带 3.0000000000000004 这种毛刺。
  // 吸附一下：落在格子线上就是格子线，调用点按整数判边、按格子取整都省心。
  const snap = (v: number) => {
    const r = Math.round(v);
    return Math.abs(v - r) < 1e-9 ? r : v;
  };
  const clamp = (a: number) => (a <= -Math.PI ? a + 2 * Math.PI : a > Math.PI ? a - 2 * Math.PI : a);
  for (let i = 0; i < points.length; i += 1) {
    const prev = points[(i - 1 + points.length) % points.length];
    const v = points[i];
    const next = points[(i + 1) % points.length];
    const ax = prev.x - v.x;
    const ay = prev.y - v.y;
    const bx = next.x - v.x;
    const by = next.y - v.y;
    const la = Math.hypot(ax, ay);
    const lb = Math.hypot(bx, by);
    if (la < 1e-6 || lb < 1e-6) {
      out.push(v);
      continue;
    }
    const dax = ax / la;
    const day = ay / la;
    const dbx = bx / lb;
    const dby = by / lb;
    // θ 是角本身的大小：180° 的直线段没有角可让，0° 的刺尖也不让。
    const cos = Math.max(-1, Math.min(1, dax * dbx + day * dby));
    const theta = Math.acos(cos);
    if (theta < 0.05 || theta > Math.PI - 0.05) {
      out.push(v);
      continue;
    }
    const half = theta / 2;
    const tanHalf = Math.tan(half);
    if (tanHalf < 1e-6) {
      out.push(v);
      continue;
    }
    const r = Math.min(radius, (Math.min(la, lb) / 2) * tanHalf);
    const t = r / tanHalf;
    const nx = dax + dbx;
    const ny = day + dby;
    const nl = Math.hypot(nx, ny);
    if (nl < 1e-6) {
      out.push(v);
      continue;
    }
    // 圆心在角平分线上，离顶点 r / sin(θ/2)；切点两边各退 t。
    const cx = v.x + (nx / nl) * (r / Math.sin(half));
    const cy = v.y + (ny / nl) * (r / Math.sin(half));
    const px = v.x + dax * t;
    const py = v.y + day * t;
    const qx = v.x + dbx * t;
    const qy = v.y + dby * t;
    const a0 = Math.atan2(py - cy, px - cx);
    const a1 = Math.atan2(qy - cy, qx - cx);
    const toV = clamp(Math.atan2(v.y - cy, v.x - cx) - a0);
    let delta = clamp(a1 - a0);
    // 弧要从 P 绕到 Q，且必须拐向角这一侧；方向选反了就会在另一侧重画一段。
    const inside = delta > 0 ? toV > 0 && toV < delta : toV < 0 && toV > delta;
    if (!inside) delta = clamp(delta + (delta > 0 ? -2 * Math.PI : 2 * Math.PI));
    const steps = Math.max(2, Math.round(arc));
    for (let s = 0; s <= steps; s += 1) {
      const ang = a0 + (delta * s) / steps;
      out.push({ x: snap(cx + r * Math.cos(ang)), y: snap(cy + r * Math.sin(ang)) });
    }
  }
  return out;
}

/**
 * 圆角矩形：radius 取「短边比例」而不是绝对格数，拖大拖小圆度观感一致。
 * radius <= 0 时直接退回原来的硬角矩形，UI 上那个「尖角」档就是走这条。
 */
export function roundedRectShape(
  a: StrokeCell,
  b: StrokeCell,
  filled = false,
  radius = 0,
): StrokeCell[] {
  if (!(radius > 0)) return rectShape(a, b, filled);
  const { x0, x1, y0, y1 } = bounds(a, b);
  if (x1 - x0 < 2 || y1 - y0 < 2) return rectShape(a, b, filled);
  const r = Math.min(radius, 0.5) * Math.min(x1 - x0, y1 - y0);
  const ring = [
    { x: x0, y: y0 },
    { x: x1, y: y0 },
    { x: x1, y: y1 },
    { x: x0, y: y1 },
  ];
  const rounded = roundCorners(ring, r);
  return filled ? polygonFillCells(rounded) : polygonOutlineCells(rounded);
}

/**
 * 圆角三角形：三个角一起让，顶角也不例外。
 * 像素画里直角三角的斜边和格子对不齐，圆角之后反而更贴轮廓。
 */
export function roundedTriangleShape(
  a: StrokeCell,
  b: StrokeCell,
  filled = false,
  radius = 0,
): StrokeCell[] {
  if (!(radius > 0)) return triangleShape(a, b, filled);
  const { x0, x1, y0, y1 } = bounds(a, b);
  const ring = [
    { x: Math.round((x0 + x1) / 2), y: y0 },
    { x: x1, y: y1 },
    { x: x0, y: y1 },
  ];
  const edges = [
    Math.hypot(ring[1].x - ring[0].x, ring[1].y - ring[0].y),
    Math.hypot(ring[2].x - ring[1].x, ring[2].y - ring[1].y),
    Math.hypot(ring[0].x - ring[2].x, ring[0].y - ring[2].y),
  ];
  const shortest = Math.min(...edges);
  if (shortest < 2) return triangleShape(a, b, filled);
  const r = Math.min(radius, 0.5) * shortest;
  const rounded = roundCorners(ring, r);
  return filled ? polygonFillCells(rounded) : polygonOutlineCells(rounded);
}

/**
 * 形状档位的统一出口：锚点 + 当前格 -> 该落哪些格子。
 *
 * 调用点（DocumentPanel 的 shapePreview）原来是个 switch，每加一种形状或
 * 一个参数就要回去改一遍；现在形状和参数都在这里收口，UI 只管传语义。
 * 「平滑曲线」不吃锚点，走 smoothPathCells，这里返回空数组由调用分支开。
 */
export function shapeCells(
  kind: ShapeKind,
  a: StrokeCell,
  b: StrokeCell,
  options: { filled?: boolean; radius?: number } = {},
): StrokeCell[] {
  const filled = options.filled ?? false;
  const radius = options.radius ?? 0;
  switch (kind) {
    case "line":
      return lineShape(a, b);
    case "rect":
      return roundedRectShape(a, b, filled, radius);
    case "ellipse":
      return ellipseShape(a, b, filled);
    case "triangle":
      return roundedTriangleShape(a, b, filled, radius);
    default:
      return [];
  }
}
