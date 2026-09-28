// 字典的三条硬规矩：zh/en 逐键对齐、源码里 t("...") 出现过的键都有家、插值与后备不翻车。
// 界面中文化是一次全仓替换，漏一个键就是界面上裸一句英文，所以这里按源码扫，不靠人眼。

import { describe, expect, it } from "vitest";

import { en, LANG_OPTIONS, renderUiText, translate, translateText, zh } from "./i18n";
import type { UiText } from "./types";

const DOCK_KINDS = [
  "agent",
  "image_gen",
  "vision_brief",
  "video_frames",
  "video_brief",
  "frame_tween",
  "prompt_refine",
  "quantize",
];

/** 整个 src 的源码原文：glob 在构建期展开，跑测试时就是一堆字符串。 */
const SOURCES = import.meta.glob("../**/*.{ts,tsx}", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

/** 源码里所有字面量取键的调用：t("k")、translate(lang, "k")、translateText(lang, "k", ...)。 */
function referencedKeys(): string[] {
  const keys = new Set<string>();
  const patterns = [
    /\bt\(\s*"([\w.]+)"/g,
    /translate\(\s*lang\s*,\s*"([\w.]+)"/g,
    /translateText\(\s*lang\s*,\s*"([\w.]+)"/g,
  ];
  for (const [path, text] of Object.entries(SOURCES)) {
    if (path.endsWith(".test.ts")) continue;
    for (const pattern of patterns) {
      for (const match of text.matchAll(pattern)) keys.add(match[1]);
    }
  }
  return [...keys].sort();
}

/** Rust 源码原文：glob 到仓库外一层，跑测试时就是一堆字符串。 */
const RUST_SOURCES = import.meta.glob("../../{src-tauri,crates/*}/src/*.rs", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

/** Rust 侧 `UiText::new("k", "fallback")` 的键与英文后备。跨行写法和尾随逗号都要认。 */
function rustFallbacks(): { key: string; fallback: string; file: string }[] {
  const found: { key: string; fallback: string; file: string }[] = [];
  const pattern = /UiText::new\(\s*"([\w.]+)"\s*,\s*"((?:[^"\\]|\\.)*)"\s*,?\s*\)/gs;
  for (const [path, text] of Object.entries(RUST_SOURCES)) {
    for (const match of text.matchAll(pattern)) {
      // 报错里写清是哪个文件，不然改文案的人得自己翻。
      found.push({ key: match[1], fallback: match[2], file: path.replace("../../", "") });
    }
  }
  return found;
}

const placeholders = (template: string): string[] =>
  [...new Set([...template.matchAll(/\{(\w+)\}/g)].map((m) => m[1]))].sort();

describe("dictionaries", () => {
  it("aligns zh and en key for key", () => {
    expect(Object.keys(en).sort()).toEqual(Object.keys(zh).sort());
  });

  it("leaves no value empty", () => {
    for (const [key, value] of Object.entries(zh)) {
      expect(value.trim(), `zh ${key}`).not.toBe("");
      expect(en[key as keyof typeof en].trim(), `en ${key}`).not.toBe("");
    }
  });

  it("carries one option per language", () => {
    expect(LANG_OPTIONS.map((option) => option.value)).toEqual(["zh", "en"]);
  });

  it("gives every dynamic dock row a full set of machine-side keys", () => {
    for (const kind of DOCK_KINDS) {
      for (const field of ["title", "summary", "output"]) {
        const key = `wf.${kind}.${field}`;
        expect(key in zh, `zh ${key}`).toBe(true);
        expect(key in en, `en ${key}`).toBe(true);
      }
    }
  });
});

describe("referenced keys", () => {
  it("resolves every literal t() call in the source", () => {
    const missing = referencedKeys().filter((key) => !(key in zh) || !(key in en));
    expect(missing).toEqual([]);
  });
});

// Rust 的后备文案是「字典缺键时用户看到的英文」，所以它必须和 en 条目逐字同构。
// 占位符一旦漂移，缺键那一侧就会把 {count} 原样显示出来，或者把已传的变量悄悄吃掉。
describe("rust fallbacks", () => {
  const fallbacks = rustFallbacks();

  it("finds the UiText calls it is meant to police", () => {
    expect(fallbacks.length).toBeGreaterThan(5);
  });

  it("names a key the dictionaries actually have", () => {
    expect(fallbacks.filter((f) => !(f.key in en))).toEqual([]);
  });

  it("carries the same placeholders as the en entry", () => {
    const drifted = fallbacks
      .filter((f) => f.key in en)
      .filter((f) => placeholders(f.fallback).join() !== placeholders(en[f.key as keyof typeof en]).join())
      .map((f) => `${f.file} ${f.key}: fallback [${placeholders(f.fallback)}] vs en [${placeholders(en[f.key as keyof typeof en])}]`);
    expect(drifted).toEqual([]);
  });

  it("keeps zh and en placeholders identical", () => {
    const drifted = Object.keys(zh)
      .filter((key) => placeholders(zh[key as keyof typeof zh]).join() !== placeholders(en[key as keyof typeof en]).join())
      .map((key) => `${key}: zh [${placeholders(zh[key as keyof typeof zh])}] vs en [${placeholders(en[key as keyof typeof en])}]`);
    expect(drifted).toEqual([]);
  });
});

describe("translate", () => {
  it("returns the language asked for", () => {
    expect(translate("zh", "dock.title")).toBe("工作流");
    expect(translate("en", "dock.title")).toBe("Workflows");
  });

  it("interpolates named vars and keeps unknown placeholders intact", () => {
    expect(translate("zh", "dock.frames_in_between", { count: 4 })).toBe("中间插 4 帧");
    expect(translate("zh", "dock.blocked_title", { title: "量化" })).toBe("当前模型不能量化");
    expect(translate("en", "dock.frames_in_between", { count: 4 })).toBe("Frames in between: 4");
  });

  it("falls back to English when the dictionary has no such key", () => {
    expect(translateText("zh", "wf.unknown.title", undefined, "from Rust")).toBe("from Rust");
  });

  it("renders a Rust UiText in the current language", () => {
    const text: UiText = { key: "dock.insert", vars: { n: 1 }, fallback: "insert" };
    expect(renderUiText("zh", text)).toBe(translate("zh", "dock.insert", { n: 1 }));
    expect(renderUiText("en", text)).toBe(translate("en", "dock.insert", { n: 1 }));
  });

  it("keeps the fallback when the dict key is absent", () => {
    const text: UiText = { key: "outcome.v99", vars: undefined, fallback: "older build" };
    expect(renderUiText("en", text)).toBe("older build");
  });
});
