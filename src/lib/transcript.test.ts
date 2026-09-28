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
