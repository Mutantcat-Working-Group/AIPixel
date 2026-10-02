// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only

/**
 * 横向条子（帧条、坞里的帧/图层挑选条）的滚动几何。
 *
 * 全部是纯函数：只吃三个数、吐结论，不碰 DOM。调用方（useHScroll 的边缘
 * 状态、HStrip 的滑块与翻页）都从这里取，判定口径只有一份，不会出现
 * 「翻页按钮亮了但滑块爬到顶上」这种各算一套的分裂。
 */

export type StripMetrics = {
  scrollWidth: number;
  clientWidth: number;
  scrollLeft: number;
};

export type StripEdges = { canLeft: boolean; canRight: boolean };

export const NEUTRAL_EDGES: StripEdges = { canLeft: false, canRight: false };

/** 内容比视窗宽出多少。浮点误差也要挡：宽出 0.4px 不算溢出。 */
export function stripReach(metrics: StripMetrics): number {
  return Math.max(0, metrics.scrollWidth - metrics.clientWidth);
}

/** 内容溢出来了才谈滚动，否则滑块和翻页按钮都不该出现。 */
export function stripScrollable(metrics: StripMetrics): boolean {
  return stripReach(metrics) > 1;
}

/** 往左、往右还有没有内容。翻页按钮的亮灭和两端渐隐都看这个。 */
export function stripEdgesOf(metrics: StripMetrics): StripEdges {
  const reach = stripReach(metrics);
  if (reach <= 1) return NEUTRAL_EDGES;
  return { canLeft: metrics.scrollLeft > 1, canRight: metrics.scrollLeft < reach - 1 };
}

/**
 * 把「条款第几格」的意图翻成像素：这格要落进条子的可见区。
 *
 * cellLeft 用内容坐标（含滚动偏移，即格子相对条子内容左缘的位置），
 * 不是视口坐标——调用方拿 getBoundingClientRect 相减再减一次 scrollLeft 就是。
 *
 * 整格已经在视野里就原样返回：点中间那一格时把条子抖一下，比不抖更烦。
 * 只有真看不见才挪，且尽量居中；夹不到尽头（最后一格天生居不了中）就贴边。
 */
export function stripScrollFor(
  metrics: StripMetrics,
  cellLeft: number,
  cellWidth: number,
): number {
  const reach = stripReach(metrics);
  const viewFrom = metrics.scrollLeft;
  const viewTo = viewFrom + metrics.clientWidth;
  if (cellLeft >= viewFrom && cellLeft + cellWidth <= viewTo) return metrics.scrollLeft;
  const room = Math.max(0, metrics.clientWidth - cellWidth);
  return Math.max(0, Math.min(reach, cellLeft - room / 2));
}

/**
 * 滑块在轨道里的位置和宽度。宽度跟着「可见部分占全部的几分之几」走，
 * 所以帧越多滑块越短，用户一眼看得出要滚多远。
 *
 * minThumb 是下限：再短也得留一寸能抓住，否则帧一多滑块成一根头发，
 * 抓着反而比拨轮难用。
 */
export function thumbBox(
  metrics: StripMetrics,
  trackWidth: number,
  minThumb: number,
): { left: number; width: number } {
  const track = Math.max(0, trackWidth);
  const reach = stripReach(metrics);
  if (track <= 0) return { left: 0, width: 0 };
  if (reach <= 1) return { left: 0, width: track };
  const width = Math.min(track, Math.max(minThumb, (track * metrics.clientWidth) / metrics.scrollWidth));
  // 滑块够短才有行程；一旦短不下去（轨道比 minThumb 还窄），行程归零，
  // 位置也锁在起点，免得除出个 NaN 把浮标甩出轨道。
  const travel = Math.max(0, track - width);
  const left = (travel * metrics.scrollLeft) / reach;
  return { left, width };
}

/** 拖着滑块走时，指针横移多少像素换成多少 scrollLeft。 */
export function thumbDragScroll(
  metrics: StripMetrics,
  travel: number,
  deltaPointer: number,
): number {
  const reach = stripReach(metrics);
  if (travel <= 0 || reach <= 0) return metrics.scrollLeft;
  return Math.max(0, Math.min(reach, metrics.scrollLeft + (deltaPointer / travel) * reach));
}
