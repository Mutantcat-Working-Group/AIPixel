// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
import { describe, expect, it } from "vitest";

import {
  historyToTranscript,
  pushPendingAssistant,
  pushSideNotice,
  pushUserMessage,
  reduceEvent,
  sealTranscript,
  stripImageCaption,
} from "./transcript";
import type { AgentEvent, Message, PendingAttachment, TranscriptEntry } from "./types";

const CAPTION = [
  "make a knight",
  "",
  "Images attached to this message, in order:",
  "1. ref.png (reference)",
  "2. snapshot.png (snapshot)",
].join("\n");

function imagePending(): PendingAttachment[] {
  return [
    {
      key: "a",
      role: "reference",
      name: "ref.png",
      mediaType: "image/png",
      dataBase64: "AAAA",
      previewUrl: "data:image/png;base64,AAAA",
    },
  ];
}

function lastEntry(entries: TranscriptEntry[]): TranscriptEntry {
  return entries[entries.length - 1];
}

/** 抽帧坞每落一帧广播一条进度，拼成真机形状免得测试和实现各说各话。 */
function readingFrame(index: number, total: number): AgentEvent {
  return {
    kind: "status",
    message: {
      key: "status.reading_frame",
      vars: { index, total },
      fallback: "reading frame {index} of {total}",
    },
  } as AgentEvent;
}

describe("stripImageCaption", () => {
  it("drops the injected manifest but keeps the user text", () => {
    expect(stripImageCaption(CAPTION)).toBe("make a knight");
  });

  it("leaves plain text untouched", () => {
    expect(stripImageCaption("no images here")).toBe("no images here");
  });
});

describe("historyToTranscript", () => {
  it("pairs a tool_use with its tool_result", () => {
    const messages: Message[] = [
      { role: "user", content: [{ type: "text", text: "go" }] },
      {
        role: "assistant",
        content: [
          { type: "text", text: "on it" },
          { type: "tool_use", id: "t1", name: "pixel_apply_operations", input: { ops: [] } },
        ],
      },
      {
        role: "tool",
        content: [{ type: "tool_result", tool_use_id: "t1", content: "ok", is_error: false }],
      },
    ];
    const entries = historyToTranscript(messages);
    const tool = entries.find((entry) => entry.kind === "tool");
    expect(tool).toBeDefined();
    expect(tool?.kind === "tool" && tool.summary).toBe("ok");
    expect(entries.filter((entry) => entry.kind === "assistant")).toHaveLength(1);
  });

  it("rebuilds history image attachments from the stored blocks", () => {
    const messages: Message[] = [
      {
        role: "user",
        content: [
          { type: "text", text: CAPTION },
          { type: "image", media_type: "image/png", data_base64: "QUJD" },
        ],
      },
    ];
    const entries = historyToTranscript(messages);
    const user = entries[0];
    expect(user.kind).toBe("user");
    if (user.kind !== "user") return;
    expect(user.text).toBe("make a knight");
    expect(user.attachments).toHaveLength(1);
    expect(user.attachments[0].previewUrl).toBe("data:image/png;base64,QUJD");
  });
});

describe("reduceEvent", () => {
  it("appends tokens onto the live assistant entry", () => {
    let entries: TranscriptEntry[] = [];
    entries = reduceEvent(entries, { kind: "token", text: "Draw" } as AgentEvent);
    entries = reduceEvent(entries, { kind: "token", text: "ing" } as AgentEvent);
    const last = lastEntry(entries);
    expect(last.kind).toBe("assistant");
    expect(last.kind === "assistant" && last.text).toBe("Drawing");
    expect(last.kind === "assistant" && last.live).toBe(true);
  });

  it("closes a live turn on completed, error, and interrupted", () => {
    const base = pushUserMessage([], "hi", []);
    const live = reduceEvent(base, { kind: "token", text: "partial" } as AgentEvent);
    expect(lastEntry(sealTranscript(live)).kind === "assistant" &&
      (lastEntry(sealTranscript(live)) as { live: boolean }).live).toBe(false);

    const interrupted = reduceEvent(live, { kind: "interrupted" } as AgentEvent);
    expect(lastEntry(interrupted).kind).toBe("notice");
  });

  it("leaves no dangling caret when one turn is split by tool calls", () => {
    // 真实时序：正文 → 工具 → 正文 → 工具 → 正文，五段里四段是 assistant。
    // 只封最后一个的话，前面几段的蓝色光标会一直闪。
    let entries = pushUserMessage([], "画个猫", []);
    entries = reduceEvent(entries, { kind: "token", text: "先画身体" } as AgentEvent);
    entries = reduceEvent(entries, {
      kind: "tool_call",
      id: "t1",
      name: "pixel_run_shader",
      input: {},
    } as AgentEvent);
    entries = reduceEvent(entries, {
      kind: "tool_result",
      id: "t1",
      name: "pixel_run_shader",
      summary: "ok",
      is_error: false,
    } as AgentEvent);
    entries = reduceEvent(entries, { kind: "token", text: "再画腿" } as AgentEvent);
    entries = reduceEvent(entries, {
      kind: "tool_call",
      id: "t2",
      name: "pixel_run_shader",
      input: {},
    } as AgentEvent);
    entries = reduceEvent(entries, {
      kind: "token",
      text: "完成",
    } as AgentEvent);

    const blocks = entries.filter((entry) => entry.kind === "assistant");
    expect(blocks.map((entry) => entry.kind === "assistant" && entry.text)).toEqual([
      "先画身体",
      "再画腿",
      "完成",
    ]);

    const sealed = sealTranscript(entries);
    expect(sealed.filter((entry) => entry.kind === "assistant").every((entry) => !entry.live)).toBe(
      true,
    );
    expect(sealed.some((entry) => entry.kind === "pending")).toBe(false);
  });

  // 发出去没等到 tool_result 就被收尾（点停止 / 拔网线）：这一条不能永远
  // 挂着「运行中」。封口之后界面才问得出「到底跑完没有」。
  it("seals a tool call that never got its result", () => {
    const entries = reduceEvent([], {
      kind: "tool_call",
      id: "t9",
      name: "pixel_run_shader",
      input: {},
    } as AgentEvent);

    const live = lastEntry(entries);
    expect(live.kind === "tool" && live.live).toBe(true);

    const sealed = sealTranscript(entries);
    const tool = sealed.find((entry) => entry.kind === "tool");
    expect(tool?.kind === "tool" && tool.live).toBe(false);
    expect(tool?.kind === "tool" && tool.summary).toBe(null);
  });

  it("marks a failed tool result as an error and stops it being live", () => {
    let entries = reduceEvent([], {
      kind: "tool_call",
      id: "t1",
      name: "pixel_run_shader",
      input: {},
    } as AgentEvent);
    entries = reduceEvent(entries, {
      kind: "tool_result",
      id: "t1",
      name: "pixel_run_shader",
      summary: "lua error: boom",
      is_error: true,
    } as AgentEvent);
    const tool = entries.find((entry) => entry.kind === "tool");
    expect(tool?.kind === "tool" && tool.isError).toBe(true);
    expect(tool?.kind === "tool" && tool.live).toBe(false);
  });

  it("replaces the pending placeholder with the first real token", () => {
    let entries = pushUserMessage([], "hi", []);
    entries = pushPendingAssistant(entries, false);
    expect(lastEntry(entries).kind).toBe("pending");
    entries = reduceEvent(entries, { kind: "token", text: "On it" } as AgentEvent);
    const last = lastEntry(entries);
    expect(last.kind).toBe("assistant");
    expect(last.kind === "assistant" && last.text).toBe("On it");
    expect(last.kind === "assistant" && last.live).toBe(true);
  });

  // 中转每回合都从 call_0 / 0 重新编号，两回合的 id 一模一样。按 id 全局找
  // 第一条的话，第二回合的结果会封到第一回合的条目上，本回合那条永远
  // 转圈——用户看到的就是「刚发的话卡在那儿，上面的旧块还被改写了」。
  it("pairs a tool result with the call from the same turn, not an older twin", () => {
    let entries = pushUserMessage([], "画只猫", []);
    entries = reduceEvent(entries, {
      kind: "tool_call",
      id: "call_0",
      name: "pixel_plan",
      input: { intent: "cat" },
    } as AgentEvent);
    entries = reduceEvent(entries, {
      kind: "tool_result",
      id: "call_0",
      name: "pixel_plan",
      summary: "第一回合",
      is_error: false,
    } as AgentEvent);

    entries = pushUserMessage(entries, "再画条狗", []);
    entries = reduceEvent(entries, {
      kind: "tool_call",
      id: "call_0",
      name: "pixel_plan",
      input: { intent: "dog" },
    } as AgentEvent);
    entries = reduceEvent(entries, {
      kind: "tool_result",
      id: "call_0",
      name: "pixel_plan",
      summary: "第二回合",
      is_error: false,
    } as AgentEvent);

    const tools = entries.filter((entry) => entry.kind === "tool");
    expect(tools).toHaveLength(2);
    expect(tools[0]?.kind === "tool" && tools[0].summary).toBe("第一回合");
    expect(tools[1]?.kind === "tool" && tools[1].summary).toBe("第二回合");
    expect(tools[1]?.kind === "tool" && tools[1].live).toBe(false);
  });

  it("replaces the pending placeholder with the first token", () => {
    let entries = pushUserMessage([], "hi", []);
    entries = pushPendingAssistant(entries, false);
    expect(lastEntry(entries).kind).toBe("pending");
    entries = reduceEvent(entries, { kind: "token", text: "On it" } as AgentEvent);
    const last = lastEntry(entries);
    expect(last.kind).toBe("assistant");
    expect(last.kind === "assistant" && last.text).toBe("On it");
    expect(last.kind === "assistant" && last.live).toBe(true);
  });

  it("turns the pending placeholder into a live reasoning block", () => {
    let entries = pushUserMessage([], "hi", []);
    entries = pushPendingAssistant(entries, true);
    entries = reduceEvent(entries, { kind: "reasoning", text: "think" } as AgentEvent);
    const last = lastEntry(entries);
    expect(last.kind).toBe("reasoning");
    expect(last.kind === "reasoning" && last.text).toBe("think");
    expect(last.kind === "reasoning" && last.live).toBe(true);
  });

  it("drops the pending placeholder when the assistant goes straight to a tool", () => {
    let entries = pushUserMessage([], "hi", []);
    entries = pushPendingAssistant(entries, false);
    entries = reduceEvent(entries, {
      kind: "tool_call",
      id: "t1",
      name: "pixel_run_shader",
      input: {},
    } as AgentEvent);
    expect(entries.some((entry) => entry.kind === "pending")).toBe(false);
    expect(entries.some((entry) => entry.kind === "tool")).toBe(true);
  });

  it("seal drops a leftover pending and flags interrupted as retryable", () => {
    let entries = pushUserMessage([], "hi", []);
    entries = pushPendingAssistant(entries, false);
    expect(sealTranscript(entries).some((entry) => entry.kind === "pending")).toBe(false);

    const interrupted = reduceEvent(pushUserMessage([], "hi", []), {
      kind: "interrupted",
    } as AgentEvent);
    const last = lastEntry(interrupted);
    expect(last.kind).toBe("notice");
    expect(last.kind === "notice" && last.retry).toBe(true);
  });

  // 续写进度是后台机制，用户已经要求别再摆出来：那句话只会让人以为
  // 模型在重写。聊天面板有时针和重试说明顶着，这一条就彻底不出现。
  it("keeps continuation progress out of the transcript", () => {
    let entries = pushUserMessage([], "hi", []);
    entries = reduceEvent(entries, { kind: "token", text: "前半段" } as AgentEvent);
    entries = reduceEvent(entries, {
      kind: "status",
      message: {
        key: "agent.continuing",
        vars: { done: 1, max: 20 },
        fallback: "continuing ({done} of {max})",
      },
    } as AgentEvent);

    entries = reduceEvent(entries, { kind: "token", text: "后半段" } as AgentEvent);
    expect(entries).toHaveLength(2);
    const last = lastEntry(entries);
    expect(last.kind).toBe("assistant");
    expect(last.kind === "assistant" && last.text).toBe("前半段后半段");
    expect(last.kind === "assistant" && last.caption).toBeUndefined();
  });

  // 状态说明一旦变成独立条目，下一发 token 就会另起一个气泡，
  // 一轮回复被撕成两半，看着就像模型把话重写了一遍。
  it("keeps a status notice inside the bubble it is streaming into", () => {
    let entries = pushUserMessage([], "hi", []);
    entries = reduceEvent(entries, { kind: "token", text: "前半段" } as AgentEvent);
    entries = reduceEvent(entries, {
      kind: "status",
      message: {
        key: "agent.thinking_off_retry",
        vars: {},
        fallback: "retrying with thinking turned off",
      },
    } as AgentEvent);
    expect(entries.some((entry) => entry.kind === "notice")).toBe(false);

    // 后续 token 还往同一个气泡里追加，caption 也留着。
    entries = reduceEvent(entries, { kind: "token", text: "后半段" } as AgentEvent);
    expect(entries).toHaveLength(2);
    const last = lastEntry(entries);
    expect(last.kind).toBe("assistant");
    expect(last.kind === "assistant" && last.text).toBe("前半段后半段");
    expect(last.kind === "assistant" && last.caption).toBe(
      "这个模型把预算全花在思考上了，一个工具都没调；已关掉思考重问一次",
    );
  });

  it("puts the notice in its own entry when nothing is streaming", () => {
    const base = pushUserMessage([], "hi", []);
    const entries = reduceEvent(base, {
      kind: "status",
      message: {
        key: "agent.retrying",
        vars: { attempt: 1, max: 5, reason: "boom" },
        fallback: "retrying ({attempt} of {max})",
      },
    } as AgentEvent);
    const last = lastEntry(entries);
    expect(last.kind).toBe("notice");
    expect(last.kind === "notice" && last.text).toBe("这次请求没成（boom），正在重试 1/5");
  });

  // 抽帧一个动作播四条「正在读取第 N 帧」：不就地改写的话四条全留在历史里，
  // 用户往回翻看到的净是中间态。位置不动、只换字，跑完只剩最后一条。
  it("rewrites the same progress notice in place instead of stacking one per frame", () => {
    const base = pushUserMessage([], "抽四帧", []);
    let entries = reduceEvent(base, readingFrame(1, 4));
    entries = reduceEvent(entries, readingFrame(2, 4));
    entries = reduceEvent(entries, readingFrame(3, 4));
    const notices = entries.filter((entry) => entry.kind === "notice");
    expect(notices).toHaveLength(1);
    const last = lastEntry(entries);
    expect(last.kind).toBe("notice");
    expect(last.kind === "notice" && last.text).toBe("正在读取第 3 帧，共 4 帧");
  });

  it("keeps progress notices of different keys side by side", () => {
    const base = pushUserMessage([], "抽帧再量化", []);
    let entries = reduceEvent(base, readingFrame(4, 4));
    entries = reduceEvent(entries, {
      kind: "status",
      message: {
        key: "status.quantizing",
        vars: {},
        fallback: "quantizing the generated image onto the grid",
      },
    } as AgentEvent);
    const notices = entries.filter((entry) => entry.kind === "notice");
    expect(notices).toHaveLength(2);
  });
});

describe("pushUserMessage", () => {
  it("keeps attachments with the message so a failed send still shows intent", () => {
    const entries = pushUserMessage([], "tweak this", imagePending());
    const user = entries[0];
    expect(user.kind).toBe("user");
    if (user.kind !== "user") return;
    expect(user.attachments[0].role).toBe("reference");
  });
});

describe("pushSideNotice", () => {
  // 旁支是主回合之外的动作：用户在工作流坞点了抽帧，不能把模型那段还没写完的
  // 思考过程合上。合上了用户就只看到折叠的一块，以为模型自己断了。
  it("keeps a live reasoning block open", () => {
    let entries = pushPendingAssistant([], true);
    entries = reduceEvent(entries, { kind: "reasoning", text: "先想一下腿怎么摆" } as AgentEvent);
    entries = pushSideNotice(entries, "参考图简报已生成", false);

    const reasoning = entries.find((entry) => entry.kind === "reasoning");
    expect(reasoning?.kind === "reasoning" && reasoning.live).toBe(true);
    const notice = entries[entries.length - 1];
    expect(notice.kind).toBe("notice");
    if (notice.kind !== "notice") return;
    expect(notice.text).toBe("参考图简报已生成");
    expect(notice.side).toBe(true);
  });

  it("still seals a streaming assistant bubble", () => {
    let entries = pushPendingAssistant([], false);
    entries = reduceEvent(entries, { kind: "token", text: "我这就画" } as AgentEvent);
    entries = pushSideNotice(entries, "量化完成", false);

    const assistant = entries.find((entry) => entry.kind === "assistant");
    expect(assistant?.kind === "assistant" && assistant.live).toBe(false);
  });
});

describe("工具调用 id 能跨重建认出来", () => {
  // 工具块的展开态按调用 id 记，就是为了让它在条目列表重建之后还认得自己。
  // Rust 历史读回来一遍是一条全新列表（连 key 都是新的），只有 id 是同一条调用。
  const messages: Message[] = [
    {
      role: "user",
      content: [{ type: "text", text: "画一只猫" }],
    },
    {
      role: "assistant",
      content: [
        { type: "reasoning", text: "先想一下" },
        {
          type: "tool_use",
          id: "call_01",
          name: "pixel_apply_operations",
          input: { ops: [] },
        },
        { type: "tool_result", tool_use_id: "call_01", content: "已写入 1 个操作", is_error: false },
      ],
    },
  ];

  it("rebuild keeps the tool call id", () => {
    const first = historyToTranscript(messages);
    const second = historyToTranscript(messages);

    const ids = (entries: ReturnType<typeof historyToTranscript>) =>
      entries.filter((entry) => entry.kind === "tool").map((entry) => (entry.kind === "tool" ? entry.id : ""));
    expect(ids(first)).toEqual(["call_01"]);
    expect(ids(second)).toEqual(["call_01"]);
    // 展示用的 key 每次重建都是新的：不靠 key 记状态，靠的就是 id。
    expect(first.map((entry) => entry.key)).not.toEqual(second.map((entry) => entry.key));

    const tool = first.find((entry) => entry.kind === "tool");
    expect(tool?.kind === "tool" && tool.summary).toBe("已写入 1 个操作");
  });
});
