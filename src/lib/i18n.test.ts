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
