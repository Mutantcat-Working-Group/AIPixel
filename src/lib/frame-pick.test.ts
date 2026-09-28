// 帧选择兜底的规矩：选中的帧还合法必须原样留着，用户的选择不能被动；
// 失效了退回兜底，兜底也失效退回第一帧；空文档返回空串不崩。
// 回归重点是「picked 合法时绝不被 fallback / 第一帧顶掉」——最容易被悄悄改错。

import { describe, expect, it } from "vitest";

import { resolveFrameId } from "./frame-pick";

describe("resolveFrameId", () => {
  const ids = ["f0", "f1", "f2", "f3"];

  it("选中的帧还合法，原样返回，不被兜底顶掉", () => {
    expect(resolveFrameId(ids, "f2", "f0")).toBe("f2");
    expect(resolveFrameId(ids, "f0", "f3")).toBe("f0");
  });

  it("选中的帧失效，退回兜底", () => {
    expect(resolveFrameId(ids, "gone", "f1")).toBe("f1");
  });

  it("选中的失效、兜底也失效，退回第一帧", () => {
    expect(resolveFrameId(ids, "gone", "also-gone")).toBe("f0");
    expect(resolveFrameId(ids, "gone", null)).toBe("f0");
  });

  it("空文档返回空串", () => {
    expect(resolveFrameId([], "f0", "f1")).toBe("");
    expect(resolveFrameId([], "f0", null)).toBe("");
  });
});
