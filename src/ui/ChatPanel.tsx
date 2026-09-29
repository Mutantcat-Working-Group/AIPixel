import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { Button, Input, Tooltip } from "antd";
import {
  AlertTriangle,
  Camera,
  Check,
  CheckCheck,
  ChevronDown,
  ChevronRight,
  CircleStop,
  ImagePlus,
  MessageSquare,
  RefreshCcw,
  Send,
  ShieldQuestion,
  Sparkles,
  X,
} from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";

import { STALL_SECONDS, useStore } from "../lib/store";
import { useT } from "../lib/t";
import type { ApprovalDecision, PendingAttachment, TranscriptEntry } from "../lib/types";

const IMAGE_EXTENSIONS = ["png", "jpg", "jpeg", "webp", "gif"];

/** 本轮对话的计时器：跑起来之后每秒走一格，停下来归零。 */
function TurnTimer() {
  const t = useT();
  const startedAt = useStore((s) => s.runStartedAt);
  const running = useStore((s) => s.running);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    if (!running || startedAt === null) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [running, startedAt]);

  if (!running || startedAt === null) return null;
  const total = Math.max(0, Math.floor((now - startedAt) / 1000));
  const mm = String(Math.floor(total / 60)).padStart(2, "0");
  const ss = String(total % 60).padStart(2, "0");
  return <span className="chat-running">{t("chat.elapsed", { mm, ss })}</span>;
}

function AttachChip({ item }: { item: PendingAttachment }) {
  const t = useT();
  return (
    <span className={`attach-chip ${item.role}`}>
      <img src={item.previewUrl} alt="" />
      <span className="role">
        {item.role === "snapshot" ? t("chat.role_snapshot") : t("chat.role_reference")}
      </span>
      <span>{item.name}</span>
    </span>
  );
}

function ToolEntry({
  entry,
  expanded,
  onToggle,
  awaiting,
}: {
  entry: Extract<TranscriptEntry, { kind: "tool" }>;
  expanded: boolean;
  onToggle: () => void;
  /** 这一条正是主循环停下来等决定的那条，状态文案要换。 */
  awaiting: boolean;
}) {
  const t = useT();
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
          {awaiting ? t("chat.awaiting") : pending ? t("chat.tool_running") : entry.summary}
        </span>
      </button>
      {body}
    </div>
  );
}

/** 审批卡：主循环停在一条工具调用上，只有这里能让它继续走。 */
function ApprovalCard() {
  const t = useT();
  const pending = useStore((s) => s.pendingApproval);
  if (!pending) return null;
  const resolve = (decision: ApprovalDecision) => () => {
    void useStore.getState().resolveApproval(decision);
  };
  return (
    <div className="approval-card">
      <div className="approval-head">
        <ShieldQuestion size={13} />
        <span className="approval-name">{pending.name}</span>
        <span className="approval-tag">{t("chat.awaiting")}</span>
      </div>
      <details className="approval-input">
        <summary>{t("chat.tool_input")}</summary>
        <pre>{JSON.stringify(pending.input, null, 2)}</pre>
      </details>
      <div className="approval-actions">
        <Button size="small" type="primary" icon={<Check size={13} />} onClick={resolve("approve")}>
          {t("chat.approve")}
        </Button>
        <Button size="small" icon={<CheckCheck size={13} />} onClick={resolve("approve_all")}>
          {t("chat.approve_all")}
        </Button>
        <Button size="small" danger icon={<X size={13} />} onClick={resolve("reject")}>
          {t("chat.reject")}
        </Button>
      </div>
    </div>
  );
}

function EntryRow({ entry }: { entry: TranscriptEntry }) {
  const [expanded, setExpanded] = useState(false);
  const t = useT();
  // 等待审批的调用要在对话流里标出来，否则用户不知道停在哪一条。
  const awaitingId = useStore((s) => s.pendingApproval?.callId ?? null);
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
        {entry.caption ? <div className="stream-caption">{entry.caption}</div> : null}
      </div>
    );
  }
  if (entry.kind === "pending") {
    if (entry.thinking) {
      return (
        <details className="entry-reasoning thinking" open>
          <summary>{t("chat.reasoning")}</summary>
          <div className="reasoning-body thinking-body">
            <span className="thinking-dot" />
            {t("chat.thinking")}
            <span className="caret" />
          </div>
        </details>
      );
    }
    return (
      <div className="entry-assistant">
        <div className="assistant-label">
          <Sparkles size={12} />
          AIPixel
        </div>
        <div className="assistant-body">
          {t("chat.processing")}
          <span className="caret" />
        </div>
      </div>
    );
  }
  if (entry.kind === "tool") {
    return (
      <ToolEntry
        entry={entry}
        expanded={expanded}
        onToggle={toggle}
        awaiting={awaitingId === entry.id}
      />
    );
  }
  if (entry.kind === "reasoning") {
    return (
      <>
        <details className="entry-reasoning" open={entry.live}>
          <summary>{t("chat.reasoning")}</summary>
          <div className="reasoning-body">{entry.text}</div>
        </details>
        {/* 思考块折叠起来之后，续写这一行还得看得见：用户得知道它没卡死。 */}
        {entry.caption ? <div className="stream-caption">{entry.caption}</div> : null}
      </>
    );
  }
  return (
    <div className={`entry-notice ${entry.isError ? "error" : ""}`}>
      <span>{entry.text}</span>
      {entry.retry ? (
        <Tooltip title={t("chat.retry_tip")}>
          <Button
            size="small"
            type="text"
            icon={<RefreshCcw size={13} />}
            onClick={() => void useStore.getState().retry()}
          >
            {t("chat.retry")}
          </Button>
        </Tooltip>
      ) : null}
    </div>
  );
}

export default function ChatPanel() {
  const t = useT();
  const entries = useStore((s) => s.entries);
  const running = useStore((s) => s.running);
  const stalled = useStore((s) => s.stalled);
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
    const picked = await open({ multiple: true, filters: [{ name: t("dialog.image"), extensions: IMAGE_EXTENSIONS }] });
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
        <strong>{t("chat.title")}</strong>
        <span className="grow" />
        <TurnTimer />
      </div>

      <div className="panel-body transcript" ref={scrollRef}>
        {entries.length === 0 ? (
          <div className="chat-empty">
            <p>{t("chat.empty")}</p>
          </div>
        ) : (
          entries.map((entry) => <EntryRow key={entry.key} entry={entry} />)
        )}
      </div>

      <div className="composer">
        {stalled ? (
          <div className="chat-stall">
            <AlertTriangle size={13} />
            <span>{t("chat.stall", { secs: STALL_SECONDS })}</span>
            <span className="grow" />
            <Button
              size="small"
              type="link"
              icon={<RefreshCcw size={13} />}
              onClick={() => void useStore.getState().retry()}
            >
              {t("chat.retry")}
            </Button>
            <Button
              size="small"
              type="link"
              danger
              icon={<CircleStop size={13} />}
              onClick={() => void useStore.getState().interrupt()}
            >
              {t("chat.stop")}
            </Button>
          </div>
        ) : null}
        <ApprovalCard />
        {attachments.length > 0 ? (
          <div className="composer-pending">
            {attachments.map((item) => (
              <span className="pending-chip" key={item.key}>
                <img src={item.previewUrl} alt="" />
                <span>{item.name}</span>
                <button
                  type="button"
                  className="remove"
                  aria-label={t("chat.remove_attachment")}
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
            placeholder={t("chat.placeholder")}
            autoSize={{ minRows: 2, maxRows: 8 }}
            disabled={running}
          />
        </div>

        <div className="composer-hint">
          <span className="composer-actions">
            <Tooltip title={t("chat.attach_reference")}>
              <Button size="small" type="text" icon={<ImagePlus size={14} />} onClick={pickReferenceImages} />
            </Tooltip>
            <Tooltip title={t("chat.attach_snapshot")}>
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
                {t("chat.stop")}
              </Button>
            ) : (
              <Button size="small" type="primary" icon={<Send size={14} />} onClick={submit}>
                {t("chat.send")}
              </Button>
            )}
          </span>
        </div>
      </div>
    </section>
  );
}
