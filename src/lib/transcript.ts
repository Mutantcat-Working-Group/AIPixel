// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
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

/** 发送后立刻弹一个占位节点，让用户马上有反馈；真实事件一到就被顶掉。 */
export function pushPendingAssistant(
  entries: TranscriptEntry[],
  thinking: boolean,
): TranscriptEntry[] {
  return [...entries, { key: key(), kind: "pending", thinking }];
}

/**
 * 追加一条旁支回执：工作流坞跑出来的结果、失败原因。
 *
 * 关键在只封 assistant 气泡、不封思考块：旁支是主回合之外的动作，用户让坞去
 * 抽帧、量化、生图，跟模型这一段思路没关系。旁支一落地就把思考块合上，用户
 * 盯着的那段推理就没了下文，后面还长的思考只能另起一块，读起来像模型自己
 * 断过。回执追加在列表末尾，也就是这一轮思考块之后，不插到它中间。
 */
export function pushSideNotice(
  entries: TranscriptEntry[],
  text: string,
  isError: boolean,
): TranscriptEntry[] {
  return [...sealLiveAssistant(entries), { key: key(), kind: "notice", text, isError, side: true }];
}

function sealLiveAssistant(entries: TranscriptEntry[]): TranscriptEntry[] {
  const next = [...entries];
  // 一轮里可能留下多个还没封口的 assistant：模型吐一段话 → 调工具 → 再吐一段，
  // 两段末尾都带着 live 光标。只封最后一个的话，前面那个会一直闪。
  for (let i = 0; i < next.length; i += 1) {
    const entry = next[i];
    if (entry.kind === "assistant" && entry.live) next[i] = { ...entry, live: false };
  }
  return next;
}

/** 最后一个还在往外流的节点（正文或思考）的下标，没有就是 -1。 */
function lastLiveEntry(entries: TranscriptEntry[]): number {
  for (let i = entries.length - 1; i >= 0; i -= 1) {
    const entry = entries[i];
    if ((entry.kind === "assistant" || entry.kind === "reasoning") && entry.live) return i;
  }
  return -1;
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
      // 还在占位：用第一个真实 token 顶掉「在处理」的假气泡。
      if (last && last.kind === "pending") {
        const next = [...entries];
        next[next.length - 1] = {
          key: last.key,
          kind: "assistant",
          text: event.text,
          live: true,
        };
        return next;
      }
      if (last && last.kind === "assistant" && last.live) {
        const next = [...entries];
        next[next.length - 1] = { ...last, text: last.text + event.text };
        return next;
      }
      return [...entries, { key: key(), kind: "assistant", text: event.text, live: true }];
    }
    case "reasoning": {
      const last = entries[entries.length - 1];
      // 占位节点变成真正的思考块：流式期间保持展开。
      if (last && last.kind === "pending") {
        const next = [...entries];
        next[next.length - 1] = {
          key: last.key,
          kind: "reasoning",
          text: event.text,
          live: true,
        };
        return next;
      }
      if (last && last.kind === "reasoning" && last.live) {
        const next = [...entries];
        next[next.length - 1] = { ...last, text: last.text + event.text };
        return next;
      }
      return [...entries, { key: key(), kind: "reasoning", text: event.text, live: true }];
    }
    case "tool_call": {
      // 模型上一步没吐字直接调工具：占位先撤，别留着假气泡占地方。
      // 同一轮里，正文可以被工具调用切成好几段。这里先把前面那段封口：
      // 不封的话它一直算 live，两个气泡同时闪光标，turn 结束也只封得到最后一个。
      const sealed = sealLiveAssistant(entries).filter((entry) => entry.kind !== "pending");
      return [
        ...sealed,
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
      // 续写进度是后台机制：撞输出上限这种事情用户不需要看见两次
      // （计时器和重试说明已经足够），只把「正在重发」这种要等的留在界面上。
      if (event.message.key === "agent.continuing") return entries;
      const note = renderUiText(lang, event.message);
      // 正文正在往外流的时候，状态说明贴在同一个气泡的底部：另起一条 notice
      // 会把 live 气泡顶到最后一位之外，下一个 token 就换个新气泡接着写，
      // 用户看到的就是一轮回复被撕成两半。
      const live = lastLiveEntry(entries);
      if (live >= 0) {
        const target = entries[live];
        if (target.kind === "assistant" || target.kind === "reasoning") {
          const next = [...entries];
          next[live] = { ...target, caption: note };
          return next;
        }
      }
      return [...entries, { key: key(), kind: "notice", text: note, isError: false }];
    }
    case "error": {
      return [
        ...entries,
        {
          key: key(),
          kind: "notice",
          text: renderUiText(lang, event.message),
          isError: true,
          retry: true,
        },
      ];
    }
    case "interrupted": {
      const text = translate(lang, "agent.interrupted");
      return [
        ...sealLiveAssistant(entries),
        { key: key(), kind: "notice", text, isError: false, retry: true },
      ];
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
  // 占位节点一轮收尾都没变成真内容（空回复 / 直接断掉）：直接抹掉，不残留假气泡。
  // 占位只会由紧随其后的事件（token / reasoning / tool_call）顶掉，收尾时还留着就是死的。
  const withoutPending = entries.filter((entry) => entry.kind !== "pending");
  return sealLiveAssistant(withoutPending).map((entry) =>
    entry.kind === "reasoning" && entry.live ? { ...entry, live: false } : entry,
  );
}
