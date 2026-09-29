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

/** 行内语法：**、*、~~、`code`，以及反斜杠转义。 */
export function parseInline(src: string): InlineNode[] {
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
  return body.split("|").map((cell) => cell.trim());
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

/** 解析整篇文本。未闭合的代码围栏按「一直写到尾」处理，流式中途也要能看。 */
export function parseMarkdown(src: string): BlockNode[] {
  const lines = src.replace(/\r\n?/g, "\n").split("\n");
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
        const padded = cells.slice(0, head.length);
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
