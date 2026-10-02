// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
/**
 * 试着把指针捕获到某个元素上，拿不到就算了。
 *
 * `setPointerCapture` 只做一件事：指针划出元素边界之后，`pointermove` 还继续
 * 往这个元素送。它是跟手性的增强，不是功能前提——拿不到捕获的代价仅仅是
 * 「划出边界外不跟手」，绝不应该是「这一笔/这次拖动整段报废」。
 *
 * 可它对「这个 pointerId 不在活动指针表里」是会抛 `NotFoundError` 的：触屏
 * 拖动、合成事件、双指快速交替都可能撞上。画布落笔链就栽在这里——裸调放在
 * 状态准备之前，异常把 `paintingRef`、预览绘制和 `pointerup` 的提交一起带走，
 * 用户看到的现象正是「按下去没反应」。
 *
 * 所以统一收口到这一个函数：调用方只管把该做的事做完，捕获放在最后顺手一试。
 */
export function tryCapturePointer(el: Element, pointerId: number): boolean {
  try {
    el.setPointerCapture(pointerId);
    return true;
  } catch {
    // 没有活动指针可捕获（或浏览器不支持）：调用方的那件事本身已经成立。
    return false;
  }
}

/** `hasPointerCapture` 换个问法也怕抛：没捕获过 / 老浏览器缺方法，都当没有。 */
export function hasPointerCapture(el: Element, pointerId: number): boolean {
  try {
    return typeof el.hasPointerCapture === "function"
      ? el.hasPointerCapture(pointerId)
      : false;
  } catch {
    return false;
  }
}
