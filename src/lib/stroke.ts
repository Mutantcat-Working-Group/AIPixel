// 一笔笔画的几何：把指针采样的格子连成连续的格子链。
// 纯函数、零依赖，方便单测；采样在组件里做（pointermove），这里只管连线。
//
// 为什么不用 draw_line：Rust 的 draw_line 把格子写进 cel，是「结果」；
// 这里要的是「路径」，好让画笔在 pointermove 里增量累加，抬笔再整笔落文档。

import type { StrokeCell } from "./types";

/** Bresenham：from -> to 之间经过的所有格子（含两端）。 */
export function lineCells(from: StrokeCell, to: StrokeCell): StrokeCell[] {
  const cells: StrokeCell[] = [];
  let x = from.x;
  let y = from.y;
  const dx = Math.abs(to.x - from.x);
  const dy = Math.abs(to.y - from.y);
  const sx = from.x < to.x ? 1 : -1;
  const sy = from.y < to.y ? 1 : -1;
  let err = dx - dy;
  // 对角线步进时两个方向各自偏一步，画出来才是「连续」而不是 L 形折线。
  for (;;) {
    cells.push({ x, y });
    if (x === to.x && y === to.y) break;
    const doubled = err * 2;
    if (doubled > -dy && doubled < dx) {
      x += sx;
      y += sy;
      err += dx - dy;
    } else if (doubled > -dy) {
      x += sx;
      err -= dy;
    } else {
      y += sy;
      err += dx;
    }
  }
  return cells;
}

/** 把新采样接到已有格子链后面：跳过与前一段重叠的格子，顺序保持不变。 */
export function appendStroke(cells: StrokeCell[], added: StrokeCell[]): StrokeCell[] {
  if (added.length === 0) return cells;
  const last = cells[cells.length - 1];
  // 上一段线性段的起点就是上一笔的终点，直接接上；stop-1 避免重复点。
  const start = last && added[0].x === last.x && added[0].y === last.y ? 1 : 0;
  const seen = new Set(cells.map((cell) => `${cell.x},${cell.y}`));
  const out = [...cells];
  for (let i = start; i < added.length; i += 1) {
    const cell = added[i];
    const k = `${cell.x},${cell.y}`;
    if (seen.has(k)) continue;
    seen.add(k);
    out.push(cell);
  }
  return out;
}
