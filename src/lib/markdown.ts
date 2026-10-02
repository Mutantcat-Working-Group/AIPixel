// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
// 轻量 Markdown 解析：只覆盖助手真正会写的那一小类语法，
// 换来「零依赖 + 流式中途也能渲染」。输出是纯 AST，渲染交给 React。

export type InlineNode =
  | { kind: "text"; value: string }
  | { kind: "code"; value: string }
  | { kind: "strong"; children: InlineNode[] }
  | { kind: "em"; children: InlineNode[] }
  | { kind: "del"; children: InlineNode[] };

export type BlockNode =
  | { kind: "paragraph"; children: InlineNode[] }
  | { kind: "heading"; level: number; children: InlineNode[] }
  | { kind: "code"; lang: string | null; value: string }
  | { kind: "list"; ordered: boolean; items: InlineNode[][] }
  | { kind: "quote"; children: InlineNode[] }
  | { kind: "table"; head: InlineNode[][]; rows: InlineNode[][][] }
  | { kind: "rule" };

type Delimiter =
  | { kind: "strong"; run: string }
  | { kind: "em"; run: string }
  | { kind: "del"; run: string };

const PUNCT = /[!"#$%&'()*+,\-./:;<=>?@[\\\]^_`{|}~]/;

/** `_` 夹在单词中间不算强调：pixel_run_shader 不能被切成两半。 */
function underscoresAreEmphasis(src: string, index: number): boolean {
  const before = index > 0 ? src[index - 1] : "";
  if (/[0-9a-zA-Z]/.test(before)) return false;
  const rest = src.slice(index).replace(/^_+/, "");
  return !/[0-9a-zA-Z]/.test(rest);
}

function matchDelimiter(src: string, index: number): Delimiter | null {
  const rest = src.slice(index);
  if (rest.startsWith("***")) return { kind: "strong", run: "***" };
  if (rest.startsWith("**")) return { kind: "strong", run: "**" };
  if (rest.startsWith("~~")) return { kind: "del", run: "~~" };
  if (rest.startsWith("__")) return { kind: "strong", run: "__" };
  // 单个的 * 与 _：__ 已经先匹配走了，这里剩下的 _ 只会是单词内或单独一个。
  if (src[index] === "*") return { kind: "em", run: "*" };
  if (src[index] === "_" && underscoresAreEmphasis(src, index)) {
    return { kind: "em", run: "_" };
  }
  return null;
}

/** 从 open 之后找同一个界定符。跨一个软换行可以，跨段不行。 */
function findCloser(src: string, from: number, run: string): number {
  for (let i = from; i < src.length; i += 1) {
    if (src.startsWith(run, i) && src[i - 1] !== "\\") return i;
  }
  return -1;
}

function pushText(out: InlineNode[], buffer: { value: string }) {
  if (buffer.value !== "") {
    out.push({ kind: "text", value: buffer.value });
    buffer.value = "";
  }
}

/**
 * 把「成对出现的转义强调界定符」还原成真正的界定符。
 *
 * 只认 `\*` `_` `~` `` ` `` 这几种能构成强调的字符，且前后必须同一串、中间
 * 还得有内容——`\*\*加粗\*\*` 这一类才有还原的资格。单个 `\*`（想显示一颗
 * 星号）和 `C:\path` 这种跟强调无关的反斜杠一律原样留下。
 *
 * 中间段落允许再出现转义符（`(?:[^\\]|\\.)*?`）：模型给行内代码里写正则时，
 * 两头转义、中间也转义是常事，非贪婪匹配会把最近的那一对先配走。
 */
const ESCAPED_EMPHASIS = /\\([*_~`]{1,3})((?:[^\\]|\\.)*?)\\\1/g;

function unwrapEscapedEmphasis(src: string): string {
  // 连续替换到不再变化：\*\*\*粗斜\*\*\* 还原成 ***粗斜*** 之后，
  // 里面的界定符还要再交给常规流程，多跑一轮才稳。
  let out = src;
  for (let guard = 0; guard < 4; guard += 1) {
    const next = out.replace(ESCAPED_EMPHASIS, "$1$2$1");
    if (next === out) return out;
    out = next;
  }
  return out;
}

/** 行内语法：**、*、~~、`code`，以及反斜杠转义。 */
export function parseInline(src: string): InlineNode[] {
  // 模型爱写「保险式转义」：怕星号被当成强调，就把 **加粗** 写成 \*\*加粗\*\*，
  // 把 `pset()` 写成 \`pset()\`。按 CommonMark 这些该原样显示成字面星号，于是
  // 用户看到满屏反斜杠，而模型明明想给的是加粗和高亮。先把「成对出现」的那种
  // 还原回去；单个 \* 还是字面星号，真要显示星号的人不受影响。
  src = unwrapEscapedEmphasis(src);
  const out: InlineNode[] = [];
  const buffer = { value: "" };
  let i = 0;
  while (i < src.length) {
    const ch = src[i];
    if (ch === "\\" && i + 1 < src.length && PUNCT.test(src[i + 1])) {
      buffer.value += src[i + 1];
      i += 2;
      continue;
    }
    if (ch === "`") {
      let run = 0;
      while (src[i + run] === "`") run += 1;
      const fence = "`".repeat(run);
      const close = src.indexOf(fence, i + run);
      if (close >= 0) {
        let value = src.slice(i + run, close);
        // CommonMark 的规矩：两端都带空格时各去一个，` a ` 显示成 a。
        if (value.length > 1 && value.startsWith(" ") && value.endsWith(" ")) {
          value = value.slice(1, -1);
        }
        pushText(out, buffer);
        out.push({ kind: "code", value });
        i = close + run;
        continue;
      }
      buffer.value += fence;
      i += run;
      continue;
    }
    const delim = matchDelimiter(src, i);
    if (delim) {
      const closer = findCloser(src, i + delim.run.length, delim.run);
      if (closer >= 0) {
        pushText(out, buffer);
        out.push({ kind: delim.kind, children: parseInline(src.slice(i + delim.run.length, closer)) });
        i = closer + delim.run.length;
        continue;
      }
    }
    buffer.value += ch;
    i += 1;
  }
  pushText(out, buffer);
  return out;
}

const BULLET = /^[-*+]\s+(.*)$/;
const ORDERED = /^(\d{1,9})[.)]\s+(.*)$/;
/** 条目正文落在哪个分组上：BULLET 只有一个括号，ORDERED 前面还吃着序号。 */
function itemText(match: RegExpExecArray, list: "bullet" | "ordered"): string {
  return (match[list === "bullet" ? 1 : 2] ?? "").trim();
}
const HEADING = /^(#{1,6})\s+(.*)$/;
const RULE = /^(?:-{3,}|\*{3,}|_{3,})$/;

/** 表格行：| a | b |，首尾的竖线可有可无。 */
function tableCells(line: string): string[] | null {
  if (!line.includes("|")) return null;
  let body = line.trim();
  if (body.startsWith("|")) body = body.slice(1);
  if (body.endsWith("|") && !body.endsWith("\\|")) body = body.slice(0, -1);
  // 只有没被反斜杠转义的竖线才是格缝：模型在格子里写正则、写字面量时，
  // 竖线本身是内容。按裸 split 拆会把 \| 后半截挤进下一格，一列格子从此错位、
  // 而且看不出是哪一行歪的。
  const cells: string[] = [];
  let cell = "";
  for (let i = 0; i < body.length; i += 1) {
    if (body[i] === "\\" && i + 1 < body.length) {
      // 转义序列整个交给行内解析那层，这里不提前吃掉反斜杠。
      cell += body[i] + body[i + 1];
      i += 1;
      continue;
    }
    if (body[i] === "|") {
      cells.push(cell.trim());
      cell = "";
      continue;
    }
    cell += body[i];
  }
  cells.push(cell.trim());
  return cells;
}

function isDividerRow(cells: string[]): boolean {
  return cells.length > 0 && cells.every((cell) => /^:?-+:?$/.test(cell));
}

function isBlockStart(line: string): boolean {
  return (
    HEADING.test(line) ||
    BULLET.test(line) ||
    ORDERED.test(line) ||
    RULE.test(line) ||
    line.startsWith("```") ||
    line.startsWith(">") ||
    tableCells(line) !== null
  );
}

/**
 * 中转模型最爱把一串 bullet 挤进一行，用两个空格连排：
 * 「5 帧完成。  - 侧视橘猫：……  - 配色：……」。行内解析把这种段整个吞成一个
 * 段落，本该是清单的东西在界面上糊成一面墙——用户看到的就是「markdown 没排」。
 * 这里在分块之前先把它拆回真列表，下游那堆块级解析一行都不用改。
 *
 * 三条护栏，少一条就把好文本拆坏：界定符前面必须贴着一个非空格，行首的 `- `
 * 本来就是合法条目，不碰；代码围栏里一律不碰，Lua 脚本里「两个空格加一个减号」
 * 是要原样交给 run_shader 的；表格行不拆，`|` 缝被挪位置整张表就塌。
 */
const INLINE_BULLET = /(\S) {2,}[-*+] (?=\S)/g;
function splitCrampedBullets(src: string): string {
  const lines = src.replace(/\r\n?/g, "\n").split("\n");
  let fenced = false;
  const out: string[] = [];
  for (const line of lines) {
    const trimmed = line.trimStart();
    if (trimmed.startsWith("```")) fenced = !fenced;
    // 加 g 的正则带 lastIndex：先问一句能不能拆，这一问就把游标推到串尾了，
    // 紧接着的 replace 会从那儿起跳、一次都匹配不上。所以问完必须归零。
    const breakable = !fenced && !trimmed.startsWith("|") && INLINE_BULLET.test(line);
    if (!breakable) {
      out.push(line);
      continue;
    }
    INLINE_BULLET.lastIndex = 0;
    out.push(...line.replace(INLINE_BULLET, "$1\n- ").split("\n"));
  }
  return out.join("\n");
}

/** 解析整篇文本。未闭合的代码围栏按「一直写到尾」处理，流式中途也要能看。 */
export function parseMarkdown(src: string): BlockNode[] {
  const lines = splitCrampedBullets(src).split("\n");
  const blocks: BlockNode[] = [];
  let i = 0;

  while (i < lines.length) {
    const line = lines[i];
    if (line.trim() === "") {
      i += 1;
      continue;
    }

    if (line.trimStart().startsWith("```")) {
      const fence = line.trimStart().slice(0, 3);
      const lang = line.trimStart().slice(3).trim().split(/\s+/)[0];
      const body: string[] = [];
      i += 1;
      while (i < lines.length && !lines[i].trimStart().startsWith(fence)) {
        body.push(lines[i]);
        i += 1;
      }
      if (i < lines.length) i += 1;
      blocks.push({ kind: "code", lang: lang === "" ? null : lang, value: body.join("\n") });
      continue;
    }

    const heading = HEADING.exec(line);
    if (heading) {
      blocks.push({
        kind: "heading",
        level: heading[1].length,
        children: parseInline(heading[2].trim()),
      });
      i += 1;
      continue;
    }

    if (RULE.test(line.trim())) {
      blocks.push({ kind: "rule" });
      i += 1;
      continue;
    }

    // 表格：当前行能拆出格子、下一行是全 -- -- 分隔行，才是表头。
    const maybeCells = tableCells(line);
    const nextCells = i + 1 < lines.length ? tableCells(lines[i + 1]) : null;
    if (
      maybeCells &&
      nextCells &&
      maybeCells.length === nextCells.length &&
      isDividerRow(nextCells)
    ) {
      const head = maybeCells.map(parseInline);
      const rows: InlineNode[][][] = [];
      i += 2;
      while (i < lines.length) {
        if (lines[i].trim() === "") break;
        const cells = tableCells(lines[i]);
        if (!cells) break;
        // 模型常常少写一根竖线，缺的格子补空，绝不让整张表塌掉。
        // 多写的格子也不许悄悄丢：整段截掉的话，那一列内容从界面上无声消失，
        // 用户只会以为模型没写。溢出部分并进最后一格，排版让步，内容留下。
        const padded =
          cells.length > head.length
            ? [...cells.slice(0, head.length - 1), cells.slice(head.length - 1).join(" ")]
            : [...cells];
        while (padded.length < head.length) padded.push("");
        rows.push(padded.map(parseInline));
        i += 1;
      }
      blocks.push({ kind: "table", head, rows });
      continue;
    }

    const bullet = BULLET.exec(line);
    const ordered = ORDERED.exec(line);
    if (bullet || ordered) {
      const isOrdered = Boolean(ordered);
      const items: InlineNode[][] = [];
      while (i < lines.length) {
        if (lines[i].trim() === "") break;
        const b = BULLET.exec(lines[i]);
        const o = ORDERED.exec(lines[i]);
        if (isOrdered ? !o : !b) break;
        items.push(parseInline(isOrdered ? itemText(o!, "ordered") : itemText(b!, "bullet")));
        i += 1;
      }
      blocks.push({ kind: "list", ordered: isOrdered, items });
      continue;
    }

    if (line.startsWith(">")) {
      const quoted: string[] = [];
      while (i < lines.length && lines[i].startsWith(">")) {
        quoted.push(lines[i].replace(/^>\s?/, ""));
        i += 1;
      }
      blocks.push({ kind: "quote", children: parseInline(quoted.join("\n")) });
      continue;
    }

    // 普通段落：吃到空行或者下一个块级语法开头为止。
    const paragraph: string[] = [];
    while (i < lines.length && lines[i].trim() !== "" && !isBlockStart(lines[i])) {
      paragraph.push(lines[i]);
      i += 1;
    }
    if (paragraph.length === 0) {
      paragraph.push(lines[i]);
      i += 1;
    }
    blocks.push({ kind: "paragraph", children: parseInline(paragraph.join("\n")) });
  }

  return blocks;
}

function inlineToText(nodes: InlineNode[]): string {
  return nodes
    .map((node) => {
      if (node.kind === "text" || node.kind === "code") return node.value;
      return inlineToText(node.children);
    })
    .join("");
}

/** 纯文本版：给复制、摘要、检索用。 */
export function markdownToText(src: string): string {
  return parseMarkdown(src)
    .map((block) => {
      switch (block.kind) {
        case "code":
          return block.value;
        case "rule":
          return "";
        case "list":
          return block.items.map((item) => inlineToText(item)).join("\n");
        case "quote":
          return inlineToText(block.children);
        case "table":
          return [block.head, ...block.rows]
            .map((row) => row.map((cell) => inlineToText(cell)).join("\t"))
            .join("\n");
        default:
          return inlineToText(block.children);
      }
    })
    .join("\n")
    .trim();
}
