// 三条硬规矩：表里点名的模型必须命中；厂商前缀 / 日期后缀不能挡住匹配；
// 认不出来的模型回落 MAX_TOKENS_FALLBACK，而不是留空或者瞎猜一个数。
// `gpt-4.1` 不能被 `gpt-4` 抢走这条是重点——分隔符规则错了就会静默填成小了一截的数。

import { describe, expect, it } from "vitest";

import {
  MAX_TOKENS_FALLBACK,
  maxTokensForModel,
  maxTokensHint,
  resolveAlias,
} from "./model-limits";

describe("maxTokensHint", () => {
  it("点名过的模型直接给出常用上限", () => {
    expect(maxTokensHint("claude-sonnet-4-5")).toBe(64000);
    expect(maxTokensHint("gpt-4o")).toBe(16384);
    expect(maxTokensHint("deepseek-reasoner")).toBe(65536);
    // 用户手里那台就是 deepseek-v4 系的 flash：小开点也要认作一家人。
    expect(maxTokensHint("deepseek-v4.1-flash")).toBe(65536);
    expect(maxTokensHint("deepseek-v4.2-flash")).toBe(65536);
  });

  it("认得出带日期后缀的模型名", () => {
    expect(maxTokensHint("claude-sonnet-4-5-20250929")).toBe(64000);
    expect(maxTokensHint("gpt-4o-mini-2024-07-18")).toBe(16384);
  });

  it("认得出带厂商前缀的模型名", () => {
    expect(maxTokensHint("openai/gpt-4o")).toBe(16384);
    expect(maxTokensHint("zhipu/glm-4.6")).toBe(32768);
    expect(
      maxTokensHint("accounts/fireworks/models/llama-3.3-70b-instruct"),
    ).toBe(32768);
  });

  it("大小写不影响匹配", () => {
    expect(maxTokensHint("Claude-Sonnet-4-5")).toBe(64000);
    expect(maxTokensHint(" DeepSeek-Chat ")).toBe(32768);
  });

  it("不会把一个模型误判成另一个的前缀", () => {
    // gpt-4.1 不是 gpt-4；分隔符不是 - 就不算命中。
    expect(maxTokensHint("gpt-4.1")).toBe(32768);
    expect(maxTokensHint("gpt-4.1-mini")).toBe(32768);
    expect(maxTokensHint("gpt-4o-mini")).toBe(16384);
  });

  it("认不出来就返回 null，让调用方自己决定", () => {
    expect(maxTokensHint("some-mystery-model")).toBeNull();
    expect(maxTokensHint("")).toBeNull();
    expect(maxTokensHint("   ")).toBeNull();
  });

  it("同一个模型换几种写法也查到同一份上限", () => {
    // 用户手里真实存在的情况：一个模型，三个平台三个叫法。
    for (const name of [
      "deepseek-v4.1-flash",
      "deepseek-v4-1-flash",
      "DeepSeek-V4-1-Flash",
      "vendor/models/deepseek_v4.1_flash",
      "deepseek-flash",
    ]) {
      expect(maxTokensHint(name)).toBe(65536);
    }
  });
});

describe("resolveAlias", () => {
  it("省掉版本号的简名认回自家那一支", () => {
    expect(resolveAlias("deepseek-flash")).toBe("deepseek-v4-1-flash");
    // 后缀得跟着搬：补回的版本号接着原来的日期。
    expect(resolveAlias("deepseek-flash-20260101")).toBe(
      "deepseek-v4-1-flash-20260101",
    );
  });

  it("没登记的简名不硬猜", () => {
    // 猜错的代价是上限填错：`deepseek-pro` 该跟 deepseek-v4.1-pro 是一个数吗？
    // 没人知道，所以原样交表，查不到就回落。
    expect(resolveAlias("deepseek-pro")).toBe("deepseek-pro");
    expect(maxTokensHint("deepseek-pro")).toBeNull();
  });
});

describe("maxTokensForModel", () => {
  it("命中表就用表里的数", () => {
    expect(maxTokensForModel("claude-opus-4-1")).toBe(32000);
  });

  it("没命中就回落默认值，字段永远有值", () => {
    expect(maxTokensForModel("some-mystery-model")).toBe(MAX_TOKENS_FALLBACK);
    // 兜底至少要装得下一段像样的分镜脚本，不能还是老掉牙的 8192。
    expect(MAX_TOKENS_FALLBACK).toBe(32768);
  });
});
