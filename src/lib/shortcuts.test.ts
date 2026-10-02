// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
import { describe, expect, it } from "vitest";

import { editShortcut } from "./shortcuts";

const PLAIN = { meta: false, ctrl: false, shift: false, alt: false };

describe("editShortcut", () => {
  it("maps the undo pair on both platform modifiers", () => {
    expect(editShortcut("z", { ...PLAIN, meta: true }, false)).toBe("undo");
    expect(editShortcut("z", { ...PLAIN, ctrl: true }, false)).toBe("undo");
  });

  it("separates redo from undo by shift or by the y key", () => {
    expect(editShortcut("z", { ...PLAIN, meta: true, shift: true }, false)).toBe("redo");
    expect(editShortcut("y", { ...PLAIN, meta: true }, false)).toBe("redo");
    expect(editShortcut("y", { ...PLAIN, ctrl: true }, false)).toBe("redo");
  });

  it("ignores keys that are not edit shortcuts", () => {
    for (const key of ["a", "Enter", "Escape", "z", "y"]) {
      expect(editShortcut(key, PLAIN, false), key).toBeNull();
    }
  });

  it("stays out of text editing so typed words keep their own undo", () => {
    expect(editShortcut("z", { ...PLAIN, meta: true }, true)).toBeNull();
    expect(editShortcut("y", { ...PLAIN, meta: true }, true)).toBeNull();
  });

  it("never touches Alt combos because input methods use them", () => {
    expect(editShortcut("z", { ...PLAIN, meta: true, alt: true }, false)).toBeNull();
    expect(editShortcut("y", { ...PLAIN, ctrl: true, alt: true }, false)).toBeNull();
  });

  it("treats an uppercase key as the lowercase letter", () => {
    // 调用方统一 toLowerCase，这里只确认传入什么就按什么判。
    expect(editShortcut("Z", { ...PLAIN, meta: true }, false)).toBeNull();
    expect(editShortcut("z", { ...PLAIN, meta: true }, false)).toBe("undo");
  });
});
