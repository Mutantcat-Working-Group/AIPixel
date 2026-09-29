import { describe, expect, it } from "vitest";

import {
  historyToTranscript,
  pushPendingAssistant,
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

  // 续写是用户抱怨最狠的一处：状态说明只要变成独立条目，下一发 token
  // 就会另起一个气泡，一轮回复被撕成两半，看着就像模型把话重写了一遍。
  it("keeps a status notice inside the bubble it is streaming into", () => {
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
    expect(entries.some((entry) => entry.kind === "notice")).toBe(false);

    // 后续 token 还往同一个气泡里追加，caption 也留着。
    entries = reduceEvent(entries, { kind: "token", text: "后半段" } as AgentEvent);
    expect(entries).toHaveLength(2);
    const last = lastEntry(entries);
    expect(last.kind).toBe("assistant");
    expect(last.kind === "assistant" && last.text).toBe("前半段后半段");
    expect(last.kind === "assistant" && last.caption).toBe(
      "回答到了输出上限，正在接着写（第 1/20 次）",
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
