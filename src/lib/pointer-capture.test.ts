// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
import { describe, expect, it } from "vitest";

import { hasPointerCapture, tryCapturePointer } from "./pointer-capture";

/** 一段假的元素：捕获可以按脚本成功或抛 NotFoundError，用来喂给两个守卫。 */
function fakeElement(behavior: "ok" | "throw"): Element & {
  pointerCaptureCalls: number[];
  releaseCalls: number[];
} {
  const pointerCaptureCalls: number[] = [];
  const releaseCalls: number[] = [];
  return {
    pointerCaptureCalls,
    releaseCalls,
    setPointerCapture(id: number) {
      pointerCaptureCalls.push(id);
      if (behavior === "throw") {
        // 真实浏览器对「这个 pointerId 不是活动指针」抛的就是这个。
        throw new DOMException("No active pointer with the given id is found.", "NotFoundError");
      }
    },
    hasPointerCapture: () => behavior === "ok",
    releasePointerCapture(id: number) {
      releaseCalls.push(id);
    },
  } as unknown as Element & { pointerCaptureCalls: number[]; releaseCalls: number[] };
}

describe("tryCapturePointer", () => {
  it("hands back the success of the capture so callers can tell", () => {
    const el = fakeElement("ok");
    expect(tryCapturePointer(el, 7)).toBe(true);
    expect(el.pointerCaptureCalls).toEqual([7]);
  });

  it("swallows NotFoundError instead of letting it kill the caller's work", () => {
    // 画布落笔链栽在这儿：异常把 paintingRef、预览和 pointerup 的提交一起带走，
    // 用户看到「按下去没反应」。守卫必须只返回 false，绝不往外抛。
    const el = fakeElement("throw");
    expect(() => tryCapturePointer(el, 3)).not.toThrow();
    expect(tryCapturePointer(el, 3)).toBe(false);
  });
});

describe("hasPointerCapture", () => {
  it("reads the real answer when the element can say", () => {
    expect(hasPointerCapture(fakeElement("ok"), 1)).toBe(true);
    expect(hasPointerCapture(fakeElement("throw"), 1)).toBe(false);
  });

  it("treats a missing or throwing hasPointerCapture as 'not captured'", () => {
    const bare = {} as Element;
    expect(hasPointerCapture(bare, 1)).toBe(false);
    const nasty = {
      hasPointerCapture() {
        throw new Error("weird DOM");
      },
    } as unknown as Element;
    expect(hasPointerCapture(nasty, 1)).toBe(false);
  });
});
