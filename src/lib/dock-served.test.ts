// 坞里「由谁跑」那句的两条硬规矩：
// 单绑了模型说「由 X 跑」，没绑、主模型兜底必须说「用主模型 X 跑」；
// 拿主模型冒充专属模型，是用户最反感的误导，这里钉死。

import { describe, expect, it } from "vitest";

import { translate } from "./i18n";
import { isModelBacked, servedLineText, type ServedBy, type T } from "./dock-served";
import type { DockKind } from "./types";

function tFor(lang: "zh" | "en"): T {
  return (key, vars) => translate(lang, key, vars);
}

describe("servedLineText", () => {
  it("单独绑了模型的中文要说「由 X 跑」", () => {
    const served: ServedBy = { label: "本地测试模型", detached: true };
    expect(servedLineText(served, tFor("zh"))).toBe("由 本地测试模型 跑");
  });

  it("没绑模型、主模型兜底的中文要说「用主模型 X 跑」", () => {
    const served: ServedBy = { label: "本地测试模型", detached: false };
    expect(servedLineText(served, tFor("zh"))).toBe("用主模型 本地测试模型 跑");
  });

  it("英文同样分得清专属与兜底", () => {
    const t = tFor("en");
    expect(servedLineText({ label: "Mock", detached: true }, t)).toBe("Run by Mock");
    expect(servedLineText({ label: "Mock", detached: false }, t)).toBe("Run by session model Mock");
  });
});

describe("isModelBacked", () => {
  it("agent 主循环和四条模型流程在列", () => {
    for (const kind of ["agent", "prompt_refine", "image_gen", "vision_brief", "video_brief"] as DockKind[]) {
      expect(isModelBacked(kind)).toBe(true);
    }
  });

  it("抽帧、补间、量化全程本机，不说谁跑", () => {
    for (const kind of ["video_frames", "frame_tween", "quantize"] as DockKind[]) {
      expect(isModelBacked(kind)).toBe(false);
    }
  });
});
