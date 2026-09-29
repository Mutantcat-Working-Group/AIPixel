import { describe, expect, it } from "vitest";

import { markdownToText, parseInline, parseMarkdown } from "./markdown";

function textOf(nodes: ReturnType<typeof parseInline>): string {
  return nodes
    .map((node) => {
      if (node.kind === "text" || node.kind === "code") return node.value;
      return textOf(node.children);
    })
    .join("");
}

describe("parseInline", () => {
  it("splits bold, italic and code out of a run of text", () => {
    const nodes = parseInline("这是 **加粗** 与 *斜体* 加 `pixel_run_shader`");
    expect(nodes.map((node) => node.kind)).toEqual(["text", "strong", "text", "em", "text", "code"]);
    expect(textOf(nodes)).toBe("这是 加粗 与 斜体 加 pixel_run_shader");
  });

  it("keeps underscores inside words intact", () => {
    expect(parseInline("pixel_run_shader 别切")).toEqual([
      { kind: "text", value: "pixel_run_shader 别切" },
    ]);
  });

  it("still reads __bold__ and ~~strikethrough~~", () => {
    const nodes = parseInline("__粗__ 和 ~~删~~");
    expect(nodes[0].kind).toBe("strong");
    expect(nodes[2].kind).toBe("del");
  });

  it("drops the marker backslashes but keeps the literal character", () => {
    expect(textOf(parseInline("字面量 \\* 星号"))).toBe("字面量 * 星号");
  });

  it("treats a lone star as a plain character", () => {
    expect(parseInline("2 * 3 = 6")).toEqual([{ kind: "text", value: "2 * 3 = 6" }]);
  });

  it("keeps *** triple emphasis readable instead of losing content", () => {
    const nodes = parseInline("***重点***");
    expect(nodes[0].kind).toBe("strong");
    expect(textOf(nodes)).toBe("重点");
  });
});

describe("parseMarkdown", () => {
  it("turns a bullet list into one ordered list block", () => {
    const blocks = parseMarkdown("- 一\n- 二\n- 三");
    expect(blocks).toHaveLength(1);
    expect(blocks[0].kind).toBe("list");
    if (blocks[0].kind !== "list") return;
    expect(blocks[0].ordered).toBe(false);
    expect(blocks[0].items).toHaveLength(3);
  });

  it("reads numbered lists as ordered", () => {
    const blocks = parseMarkdown("1. 一\n2. 二");
    expect(blocks[0].kind).toBe("list");
    if (blocks[0].kind !== "list") return;
    expect(blocks[0].ordered).toBe(true);
    expect(blocks[0].items).toHaveLength(2);
  });

  it("keeps fenced code verbatim, lua syntax included", () => {
    const blocks = parseMarkdown("```lua\npset(1, 2, '#ff0000')\nif x then end\n```");
    expect(blocks[0]).toEqual({
      kind: "code",
      lang: "lua",
      value: "pset(1, 2, '#ff0000')\nif x then end",
    });
  });

  it("renders a half-written fence while the reply is still streaming", () => {
    const blocks = parseMarkdown("```lua\npset(0, 0, hsv(3");
    expect(blocks[0].kind).toBe("code");
    if (blocks[0].kind !== "code") return;
    expect(blocks[0].value).toBe("pset(0, 0, hsv(3");
  });

  it("reads heading levels", () => {
    const blocks = parseMarkdown("## 小标题\n\n正文");
    expect(blocks.map((block) => block.kind)).toEqual(["heading", "paragraph"]);
    if (blocks[0].kind !== "heading") return;
    expect(blocks[0].level).toBe(2);
  });

  it("reads a table and pads rows that are missing a pipe", () => {
    const blocks = parseMarkdown("| 组件 | 用法 |\n| --- | --- |\n| 画笔 | 画像素 |\n| 橡皮 | 擦 |");
    expect(blocks[0].kind).toBe("table");
    if (blocks[0].kind !== "table") return;
    expect(blocks[0].head).toHaveLength(2);
    expect(blocks[0].rows).toHaveLength(2);
    expect(blocks[0].rows[1]).toHaveLength(2);
  });

  it("does not turn a bare pipe pair into a table", () => {
    const blocks = parseMarkdown("a | b 这一行没有分隔行");
    expect(blocks[0].kind).toBe("paragraph");
  });

  it("splits paragraphs on blank lines and on the next block start", () => {
    const blocks = parseMarkdown("第一段\n\n第二段");
    expect(blocks).toHaveLength(2);
  });

  it("keeps inline emphasis inside a list item", () => {
    const blocks = parseMarkdown("- **粗** 一点");
    if (blocks[0].kind !== "list") throw new Error("expected a list");
    expect(blocks[0].items[0]).toHaveLength(2);
    expect(blocks[0].items[0][0].kind).toBe("strong");
    expect(blocks[0].items[0][1]).toEqual({ kind: "text", value: " 一点" });
  });

  it("strips markers back out for plain-text consumers", () => {
    expect(markdownToText("- **粗** 和 `code`")).toBe("粗 和 code");
  });

  it("never throws on a pile of half-formed syntax", () => {
    const messy = "**未闭合\n\n- \n| a\n>\n`` \n# \n1. ";
    expect(() => parseMarkdown(messy)).not.toThrow();
    expect(parseMarkdown(messy).length).toBeGreaterThan(0);
  });
});
