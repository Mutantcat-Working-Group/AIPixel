import { describe, expect, it } from "vitest";

import { en, zh } from "./i18n";

/** 整个 agent-core 的源码原文：glob 在构建期展开，跑测试时就是一堆字符串。 */
const RUST_SOURCES = import.meta.glob("../../crates/*/src/*.rs", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

/**
 * 计划节点把知识 id 念成 knowledge.<id>，字典缺键就会在界面上露出原始英文 id。
 * Rust 是这些 id 的产地，这里直接读源按块抽，两边字典各有一半覆盖就退化成
 * 单语界面，两边都得有。读源走 vite 的 raw glob：装 @types/node 只为读一个文件，
 * 会把一整包 Node 全局类型拖进 DOM 工程的 tsc，那笔账比省事贵得多。
 */
function knowledgeIdsFromRust(): string[] {
  const src = RUST_SOURCES["../../crates/agent-core/src/knowledge.rs"];
  if (src === undefined) throw new Error("knowledge.rs did not come through the raw glob");
  const ids = [...src.matchAll(/KnowledgeEntry \{([\s\S]*?)\n {4}\}/g)].map(
    (block) => block[1].match(/id: "([a-z0-9-]+)"/)?.[1],
  );
  const found = ids.filter((id): id is string => typeof id === "string");
  expect(found.length).toBeGreaterThan(50);
  return [...new Set(found)].sort();
}

describe("knowledge i18n keys", () => {
  it("covers every entry rust can emit", () => {
    const missing = (dict: Record<string, string>) =>
      knowledgeIdsFromRust().filter((id) => !(`knowledge.${id}` in dict));
    expect(missing(zh)).toEqual([]);
    expect(missing(en)).toEqual([]);
  });
});
