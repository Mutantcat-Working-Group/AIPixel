// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
// 角色行走图网格的识别。和 Rust 的 pixel_core::paperdoll::detect 是同一把尺：
// 行恒 4，格子至少 8px，3 列或 4 列至少有一边放得下。
//
// 前端要它，是为了在按钮还来得及置灰的时候就说清楚「白膜只铺在这种画布上」，
// 而不是让用户点完才接一条红字报错。真正说了算的仍是后端——两边判据一致，
// 用户看到的预判才不会和后端的裁决打架。

/**
 * 这个宽高是不是一个 4 行角色行走图网格。
 * 144x192（MV/MZ，3 列 48px 格）、96x128（VX/Ace，3 列 32px 格）、
 * 128x128（XP，4 列 32px 格）、72x128（3 列 24px 格）都认；
 * 高度不足 32px 或窄于 24px 的不认——人形连头都放不下。
 */
export function isSheetGrid(width: number, height: number): boolean {
  if (width < 8 || Math.floor(height / 4) < 8) return false;
  return [3, 4].some((cols) => Math.floor(width / cols) >= 8);
}
