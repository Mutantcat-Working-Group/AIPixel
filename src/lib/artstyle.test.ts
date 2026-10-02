import { describe, expect, it } from "vitest";

import { ART_STYLE_IDS } from "./artstyle";
import { en, zh } from "./i18n";

/** 整个 agent-core 的源码原文：glob 在构建期展开，跑测试时就是一堆字符串。 */
const RUST_SOURCES = import.meta.glob("../../crates/*/src/*.rs", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

/**
 * Rust 是画风 id 的产地，前端下拉要按同一份清单摆选项。两边对着读源比对：
 * Rust 加了预设而 i18n 没补键，界面上那个选项就会露出 `style.xxx` 原文；
 * 反过来 i18n 里多了键而 Rust 不认识，用户选了下拉也只会拿到「unknown art style」。
 * 走 vite 的 raw glob 而不是 node:fs：这个工程故意不装 @types/node。
 */
function styleIdsFromRust(): string[] {
  const src = RUST_SOURCES["../../crates/agent-core/src/artstyle.rs"];
  if (src === undefined) throw new Error("artstyle.rs did not come through the raw glob");
  const ids = [...src.matchAll(/ArtStyle::\w+ => "([a-z0-9]+)",/g)].map((m) => m[1]);
  return [...new Set(ids)].sort();
}

describe("art style id parity", () => {
  it("lists exactly the ids rust knows", () => {
    expect([...ART_STYLE_IDS].sort()).toEqual(styleIdsFromRust());
  });

  it("names every preset in both dictionaries", () => {
    const missing = (dict: Record<string, string>) =>
      ART_STYLE_IDS.filter((id) => !(`style.${id}` in dict));
    expect(missing(zh)).toEqual([]);
    expect(missing(en)).toEqual([]);
  });
});
