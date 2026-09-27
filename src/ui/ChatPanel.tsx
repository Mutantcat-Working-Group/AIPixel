import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { Button, Input, Tooltip } from "antd";
import {
  Camera,
  ChevronDown,
  ChevronRight,
  CircleStop,
  ImagePlus,
  MessageSquare,
  Send,
  Sparkles,
  X,
} from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";

import { useStore } from "../lib/store";
import type { PendingAttachment, TranscriptEntry } from "../lib/types";

const IMAGE_FILTER = [{ name: "image", extensions: ["png", "jpg", "jpeg", "webp", "gif"] }];

function AttachChip({ item }: { item: PendingAttachment }) {
  return (
    <span className={`attach-chip ${item.role}`}>
      <img src={item.previewUrl} alt="" />
      <span className="role">{item.role === "snapshot" ? "snap" : "ref"}</span>
      <span>{item.name}</span>
    </span>
  );
}

function ToolEntry({
  entry,
  expanded,
  onToggle,
}: {
  entry: Extract<TranscriptEntry, { kind: "tool" }>;
  expanded: boolean;
  onToggle: () => void;
}) {
  const pending = entry.summary === null;
  const body = expanded ? (
    <div className="tool-body">
      <pre>{JSON.stringify(entry.input, null, 2)}</pre>
    </div>
  ) : null;
  return (
    <div className={`entry-tool ${entry.isError ? "error" : ""}`}>
      <button type="button" className="tool-head" onClick={onToggle}>
        {expanded ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
        <span className="tool-name">{entry.name}</span>
        <span className="tool-summary">
          {pending ? "running..." : entry.summary}
        </span>
      </button>
      {body}
    </div>
  );
}

function EntryRow({ entry }: { entry: TranscriptEntry }) {
  const [expanded, setExpanded] = useState(false);
  const toggle = () => setExpanded((v) => !v);

  if (entry.kind === "user") {
    return (
      <div className="entry-user">
        {entry.attachments.length > 0 ? (
          <div className="attachments-row">
            {entry.attachments.map((item) => (
              <AttachChip key={item.key} item={item} />
            ))}
          </div>
        ) : null}
        {entry.text !== "" ? <div className="bubble">{entry.text}</div> : null}
      </div>
    );
  }
  if (entry.kind === "assistant") {
    return (
      <div className="entry-assistant">
        <div className="assistant-label">
          <Sparkles size={12} />
          AIPixel
        </div>
        <div className="assistant-body">
          {entry.text}
          {entry.live ? <span className="caret" /> : null}
        </div>
      </div>
    );
  }
  if (entry.kind === "tool") {
    return <ToolEntry entry={entry} expanded={expanded} onToggle={toggle} />;
  }
  if (entry.kind === "reasoning") {
    return (
      <details className="entry-reasoning">
        <summary>Reasoning</summary>
        <div className="reasoning-body">{entry.text}</div>
      </details>
    );
  }
  return <div className={`entry-notice ${entry.isError ? "error" : ""}`}>{entry.text}</div>;
}

export default function ChatPanel() {
  const entries = useStore((s) => s.entries);
  const running = useStore((s) => s.running);
  const attachments = useStore((s) => s.attachments);
  const [draft, setDraft] = useState("");
  const scrollRef = useRef<HTMLDivElement>(null);
  const compose = useStore((s) => s.composeRequest);

  useEffect(() => {
    const node = scrollRef.current;
    if (!node) return;
    node.scrollTop = node.scrollHeight;
  }, [entries]);

  // 工作流坞「Send to chat」：已经有内容就追加，别把用户正写的半句吃掉。
  useEffect(() => {
    if (!compose) return;
    const text = compose.text;
    setDraft((prev) => (prev.trim() === "" ? text : `${prev}\n\n${text}`));
  }, [compose]);

  async function pickReferenceImages() {
    const picked = await open({ multiple: true, filters: IMAGE_FILTER });
    if (!picked) return;
    const paths = Array.isArray(picked) ? picked : [picked];
    await useStore.getState().attachReferenceImages(paths);
  }

  function submit() {
    const text = draft;
    if (text.trim() === "" && useStore.getState().attachments.length === 0) return;
    setDraft("");
    void useStore.getState().send(text);
  }

  function onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key !== "Enter" || event.shiftKey || event.nativeEvent.isComposing) return;
    event.preventDefault();
    submit();
  }

  return (
    <section className="chat">
      <div className="chat-head">
        <MessageSquare size={13} />
        <strong>Agent</strong>
        <span className="grow" />
        {running ? <span className="chat-running">running</span> : null}
      </div>

      <div className="panel-body transcript" ref={scrollRef}>
        {entries.length === 0 ? (
          <div className="chat-empty">
            <p>Ask for a sprite, a palette, or a shader pass. The canvas is the only source of truth.</p>
          </div>
        ) : (
          entries.map((entry) => <EntryRow key={entry.key} entry={entry} />)
        )}
      </div>

      <div className="composer">
        {attachments.length > 0 ? (
          <div className="composer-pending">
            {attachments.map((item) => (
              <span className="pending-chip" key={item.key}>
                <img src={item.previewUrl} alt="" />
                <span>{item.name}</span>
                <button
                  type="button"
                  className="remove"
                  aria-label="remove"
                  onClick={() => useStore.getState().removeAttachment(item.key)}
                >
                  <X size={12} />
                </button>
              </span>
            ))}
          </div>
        ) : null}

        <div className="composer-row">
          <Input.TextArea
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={onKeyDown}
            placeholder="Describe the pixel art you want..."
            autoSize={{ minRows: 2, maxRows: 8 }}
            disabled={running}
          />
        </div>

        <div className="composer-hint">
          <span className="composer-actions">
            <Tooltip title="Attach reference images (visual ground truth)">
              <Button size="small" type="text" icon={<ImagePlus size={14} />} onClick={pickReferenceImages} />
            </Tooltip>
            <Tooltip title="Attach the current canvas as a snapshot (context only)">
              <Button
                size="small"
                type="text"
                icon={<Camera size={14} />}
                onClick={() => void useStore.getState().attachSnapshot()}
              />
            </Tooltip>
          </span>
          <span className="composer-actions">
            {running ? (
              <Button
                size="small"
                danger
                icon={<CircleStop size={14} />}
                onClick={() => void useStore.getState().interrupt()}
              >
                Stop
              </Button>
            ) : (
              <Button size="small" type="primary" icon={<Send size={14} />} onClick={submit}>
                Send
              </Button>
            )}
          </span>
        </div>
      </div>
    </section>
  );
}
