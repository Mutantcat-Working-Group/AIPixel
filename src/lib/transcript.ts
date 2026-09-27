// 把 Rust 主循环广播的 AgentEvent 流折叠成可渲染的对话条目。
// 纯函数、零依赖，方便单测；副作用（刷新画布、读 PNG）由 store 处理。

import { translate, renderUiText, type Lang } from "./i18n";
import type {
  AgentEvent,
  ContentBlock,
  Message,
  PendingAttachment,
  TranscriptEntry,
} from "./types";

let counter = 0;

function key(): string {
  counter += 1;
  return `t${counter}`;
}

export function emptyTranscript(): TranscriptEntry[] {
  return [];
}

function summaryOf(content: string, lang: Lang): string {
  const first = content.split("\n")[0].trim();
  if (first === "") return translate(lang, "chat.tool_no_output");
  return first.length <= 180 ? first : `${first.slice(0, 180)}...`;
}

/** Rust 会把图片清单说明塞进用户消息文本里；展示时要还用户一个干净的输入框。 */
export function stripImageCaption(text: string): string {
  const lines = text.split("\n");
  const start = lines.findIndex((line) =>
    line.startsWith("Images attached to this message, in order:"),
  );
  if (start < 0) return text;
  let end = start + 1;
  while (end < lines.length && /^\d+\.\s/.test(lines[end])) end += 1;
  return [...lines.slice(0, start), ...lines.slice(end)].join("\n").trim();
}

/**
 * 从 Rust 会话历史重建对话视图。
 * tool_use 与 tool_result 分属两条消息，先按 tool_use_id 收拢结果，再顺序展开。
 */
export function historyToTranscript(messages: Message[], lang: Lang = "zh"): TranscriptEntry[] {
  const results = new Map<string, { summary: string; isError: boolean }>();
  for (const message of messages) {
    for (const block of message.content) {
      if (block.type === "tool_result") {
        results.set(block.tool_use_id, {
          summary: summaryOf(block.content, lang),
          isError: block.is_error,
        });
      }
    }
  }

  const entries: TranscriptEntry[] = [];
  for (const message of messages) {
    const text = message.content
      .filter((block): block is Extract<ContentBlock, { type: "text" }> => block.type === "text")
      .map((block) => block.text)
      .join("\n");
    const images = message.content.filter(
      (block): block is Extract<ContentBlock, { type: "image" }> => block.type === "image",
    );
    if (message.role === "user" && (text !== "" || images.length > 0)) {
      const display = stripImageCaption(text);
      entries.push({
        key: key(),
        kind: "user",
        text: display,
        attachments: images.map((block, index) => ({
          key: `history:${index}`,
          role: "reference" as const,
          name: block.media_type,
          mediaType: block.media_type,
          dataBase64: block.data_base64,
          previewUrl: `data:${block.media_type};base64,${block.data_base64}`,
        })),
      });
    }
    for (const block of message.content) {
      if (block.type === "reasoning") {
        entries.push({ key: key(), kind: "reasoning", text: block.text, live: false });
      } else if (block.type === "tool_use") {
        const result = results.get(block.id);
        entries.push({
          key: key(),
          kind: "tool",
          id: block.id,
          name: block.name,
          input: block.input as Record<string, unknown>,
          summary: result?.summary ?? null,
          isError: result?.isError ?? false,
          live: false,
        });
      }
    }
    if (message.role === "assistant" && text !== "") {
      entries.push({ key: key(), kind: "assistant", text, live: false });
    }
  }
  return entries;
}

/** 用户消息先进条目列表，附件 chips 一起带上，发送失败也能保留现场。 */
export function pushUserMessage(
  entries: TranscriptEntry[],
  text: string,
  attachments: PendingAttachment[],
): TranscriptEntry[] {
  return [...entries, { key: key(), kind: "user", text, attachments }];
}

/**
 * 追加一条系统提示（工作流跑完的结果、失败原因）。
 * 先封口 live 条目：直接 append 会把 notice 塞进正在流式输出的气泡后面，
 * 下一个 token 就会另起一个气泡，把一轮对话撕成两半。
 */
export function pushNotice(
  entries: TranscriptEntry[],
  text: string,
  isError: boolean,
): TranscriptEntry[] {
  return [...sealTranscript(entries), { key: key(), kind: "notice", text, isError }];
}

function sealLiveAssistant(entries: TranscriptEntry[]): TranscriptEntry[] {
  const next = [...entries];
  for (let i = next.length - 1; i >= 0; i -= 1) {
    const entry = next[i];
    if (entry.kind === "assistant" && entry.live) {
      next[i] = { ...entry, live: false };
      break;
    }
  }
  return next;
}

/** 折叠一条事件。document_updated 不在这里处理（驱动画布，不是对话内容）。 */
export function reduceEvent(
  entries: TranscriptEntry[],
  event: AgentEvent,
  lang: Lang = "zh",
): TranscriptEntry[] {
  switch (event.kind) {
    case "token": {
      const last = entries[entries.length - 1];
      if (last && last.kind === "assistant" && last.live) {
        const next = [...entries];
        next[next.length - 1] = { ...last, text: last.text + event.text };
        return next;
      }
      return [...entries, { key: key(), kind: "assistant", text: event.text, live: true }];
    }
    case "reasoning": {
      const last = entries[entries.length - 1];
      if (last && last.kind === "reasoning" && last.live) {
        const next = [...entries];
        next[next.length - 1] = { ...last, text: last.text + event.text };
        return next;
      }
      return [...entries, { key: key(), kind: "reasoning", text: event.text, live: true }];
    }
    case "tool_call": {
      return [
        ...entries,
        {
          key: key(),
          kind: "tool",
          id: event.id,
          name: event.name,
          input: event.input,
          summary: null,
          isError: false,
          live: true,
        },
      ];
    }
    case "tool_result": {
      const index = entries.findIndex((e) => e.kind === "tool" && e.id === event.id);
      if (index < 0) return entries;
      const next = [...entries];
      const entry = next[index];
      if (entry.kind !== "tool") return entries;
      next[index] = {
        ...entry,
        summary: event.summary,
        isError: event.is_error,
        live: false,
      };
      return next;
    }
    case "status": {
      return [
        ...entries,
        { key: key(), kind: "notice", text: renderUiText(lang, event.message), isError: false },
      ];
    }
    case "error": {
      return [...entries, { key: key(), kind: "notice", text: event.message, isError: true }];
    }
    case "interrupted": {
      const text = translate(lang, "agent.interrupted");
      return [...sealLiveAssistant(entries), { key: key(), kind: "notice", text, isError: false }];
    }
    case "completed": {
      return sealLiveAssistant(entries);
    }
    case "usage":
    case "document_updated":
      return entries;
    default:
      return entries;
  }
}

/** turn 结束（完成 / 报错 / 中断）后的收尾：封口所有 live 条目。 */
export function sealTranscript(entries: TranscriptEntry[]): TranscriptEntry[] {
  return sealLiveAssistant(entries).map((entry) =>
    entry.kind === "reasoning" && entry.live ? { ...entry, live: false } : entry,
  );
}
