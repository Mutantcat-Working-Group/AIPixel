// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
import { describe, expect, it } from "vitest";

import { isSheetGrid } from "./sheet";

describe("isSheetGrid", () => {
  it("认得出四代引擎的角色行走图网格", () => {
    // 行恒 4：144x192 的格子是 48px，96x128 与 128x128 的格子是 32px，
    // 72x128 是 3 列 24px 的小格档。这几个数是用户的原话，错一个都白铺。
    for (const [w, h] of [
      [144, 192],
      [96, 128],
      [128, 128],
      [72, 128],
    ]) {
      expect(isSheetGrid(w, h), `${w}x${h}`).toBe(true);
    }
  });

  it("格子放不下人形的一概不认", () => {
    // 高度不足 32px（四行每格不到 8px）或者窄于 24px（3 列每格不到 8px）
    // 时，人形连头都搁不下，后端会拒，界面得先把按钮摁住。
    for (const [w, h] of [
      [12, 100],
      [20, 128],
      [16, 16],
      [64, 20],
    ]) {
      expect(isSheetGrid(w, h), `${w}x${h}`).toBe(false);
    }
  });

  it("64x64 也算网格：后端按 4 列 16px 格照样铺", () => {
    // 这一条是和 Rust detect 的对表：same input, same answer。
    // 界面上置灰与否必须和后端的裁决一致，不然用户以为按钮坏了。
    expect(isSheetGrid(64, 64)).toBe(true);
  });
});
