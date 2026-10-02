// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
import { describe, expect, it } from "vitest";

import {
  modelChangeTarget,
  needsModelBanner,
  resolveBoundModelId,
} from "./model-binding";

const IDS = ["m1", "m2"];

describe("resolveBoundModelId", () => {
  it("会话绑的模型有效时用它", () => {
    expect(resolveBoundModelId("m2", "m1", IDS)).toBe("m2");
  });

  it("没有会话时落到全局激活模型，而不是判成未设置", () => {
    // 空会话是正常态：用户配好了模型，只是还没建会话。这里判错就会出现
    // 「设置里明明有模型，顶栏却写未设置」。
    expect(resolveBoundModelId(null, "m1", IDS)).toBe("m1");
    expect(resolveBoundModelId(undefined, "m1", IDS)).toBe("m1");
  });

  it("会话绑的模型被删掉后也落到全局激活模型", () => {
    expect(resolveBoundModelId("gone", "m1", IDS)).toBe("m1");
  });

  it("全局激活模型也被删掉时再往下落", () => {
    expect(resolveBoundModelId(null, "gone", IDS)).toBeNull();
  });

  it("一个模型都没有才算真的没得用", () => {
    expect(resolveBoundModelId(null, null, [])).toBeNull();
  });
});

describe("needsModelBanner", () => {
  it("没配模型才要提示", () => {
    expect(needsModelBanner(0)).toBe(true);
  });

  it("配了模型就不提示，哪怕还没有会话", () => {
    expect(needsModelBanner(2)).toBe(false);
    expect(needsModelBanner(1)).toBe(false);
  });
});

describe("modelChangeTarget", () => {
  it("有会话改会话绑定", () => {
    expect(modelChangeTarget(true)).toBe("session");
  });

  it("空会话改全局激活模型，好让下一个新会话接着用", () => {
    expect(modelChangeTarget(false)).toBe("active");
  });
});
