// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
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

  it("模型写保险式转义时照样按强调排版，不把反斜杠摆在用户脸上", () => {
    // 模型怕星号被吃掉，就把 **粗** 写成 \*\*粗\*\*。CommonMark 说这是字面星号，
    // 可用户看到满屏反斜杠。成对出现的还原成粗体，语义才对得上模型的意图。
    const nodes = parseInline("\\*\\*光照分层\\*\\*");
    expect(nodes).toHaveLength(1);
    expect(nodes[0].kind).toBe("strong");
    expect(textOf(nodes)).toBe("光照分层");
  });

  it("转义的行内代码也能亮起来", () => {
    // 开头还有「用 」两个普通字符，代码节点不在下标 0：按顺序断言，别写死位置。
    const nodes = parseInline("用 \\`furShade()\\` 算法线");
    expect(nodes.map((node) => node.kind)).toEqual(["text", "code", "text"]);
    expect(textOf(nodes)).toBe("用 furShade() 算法线");
  });

  it("不成对的转义还是字面星号，想显示星号的人不受影响", () => {
    expect(textOf(parseInline("路由写成 C:\\path\\to 结尾"))).toBe("路由写成 C:\\path\\to 结尾");
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

  it("keeps overflow cells instead of dropping them", () => {
    // 模型多写了一根竖线。整段截掉的话那一列内容从界面上无声消失，
    // 用户只会以为模型没写。
    const blocks = parseMarkdown("| 帧 | 时长 |\n| --- | --- |\n| F0 | 120ms | 备注 |");
    expect(blocks[0].kind).toBe("table");
    if (blocks[0].kind !== "table") return;
    expect(blocks[0].rows[0]).toHaveLength(2);
    expect(markdownToText("| 帧 | 时长 |\n| --- | --- |\n| F0 | 120ms | 备注 |")).toContain("备注");
  });

  it("treats an escaped pipe as cell content, not a column break", () => {
    const blocks = parseMarkdown("| 写法 | 含义 |\n| --- | --- |\n| a\\|b | 或 |");
    expect(blocks[0].kind).toBe("table");
    if (blocks[0].kind !== "table") return;
    expect(blocks[0].rows[0]).toHaveLength(2);
    // 整根竖线还在第一格里：没被当成格缝砍成两截。
    expect(JSON.stringify(blocks[0].rows[0][0])).toContain("|");
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

  // 中转平台交回来的正文 bullet 常常被压进一行，用两个空格连排。这种文本走到
  // 行内解析就是个整段落，界面上糊成一面墙。拆回真列表之后 markdownToText
  // 应该给出逐行的样子，块级断言也该看到 paragraph + list 两块。
  it("un-cramps bullets that a relay model strung onto one line", () => {
    const cramp = "5 帧橘猫奔跑已完成（64x64）。  - 侧视橘猫，四足奔跑循环。  - 配色沿用调色板橘色系。";
    const blocks = parseMarkdown(cramp);
    expect(blocks.map((b) => b.kind)).toEqual(["paragraph", "list"]);
    const text = markdownToText(cramp);
    expect(text.split("\n")).toHaveLength(3);
    expect(text.split("\n")[1]).toBe("侧视橘猫，四足奔跑循环。");
  });

  it("leaves a dash that is only joined by one space alone", () => {
    // 「a - b」是个连字符，不是条目；拆了就等于改用户写的话。
    expect(parseMarkdown("变量 a - b 相减")).toHaveLength(1);
    expect(parseMarkdown("变量 a - b 相减")[0].kind).toBe("paragraph");
  });

  it("never splits inside a code fence", () => {
    const code = "```lua\nlocal a = 1  - 2\nlocal b = 3  - 4\n```";
    const blocks = parseMarkdown(code);
    expect(blocks).toHaveLength(1);
    expect(blocks[0].kind).toBe("code");
    if (blocks[0].kind !== "code") throw new Error("expected code");
    expect(blocks[0].value).toBe("local a = 1  - 2\nlocal b = 3  - 4");
  });

  it("never splits inside a table row", () => {
    const table = "| 项 | 说明 |\n| --- | --- |\n| 腿 | 前腿  - 深色 |";
    const blocks = parseMarkdown(table);
    expect(blocks[0].kind).toBe("table");
    if (blocks[0].kind !== "table") throw new Error("expected table");
    expect(blocks[0].rows[0]?.[1]).toBeDefined();
    expect(markdownToText(table)).toContain("前腿  - 深色");
  });

  it("never throws on a pile of half-formed syntax", () => {
    const messy = "**未闭合\n\n- \n| a\n>\n`` \n# \n1. ";
    expect(() => parseMarkdown(messy)).not.toThrow();
    expect(parseMarkdown(messy).length).toBeGreaterThan(0);
  });
});
