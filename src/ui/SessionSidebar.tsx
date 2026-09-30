import { useRef, useState } from "react";
import { Button, Input, InputNumber, Modal, Tooltip } from "antd";
import { Github, Pencil, Plus, Trash2 } from "lucide-react";

import { useStore } from "../lib/store";
import { useT } from "../lib/t";
import { openContextMenu } from "./ContextMenu";

const REPO_URL = "https://github.com/Mutantcat-Working-Group/AIPixel";

/** 像素画的常用起步尺寸。点一下就把 W/H 填好，用户仍然能手改。 */
const SIZE_PRESETS: { w: number; h: number; label: string }[] = [
  { w: 16, h: 16, label: "16×16" },
  { w: 32, h: 32, label: "32×32" },
  { w: 48, h: 48, label: "48×48" },
  { w: 64, h: 64, label: "64×64" },
  { w: 96, h: 96, label: "96×96" },
  { w: 128, h: 128, label: "128×128" },
  { w: 32, h: 16, label: "32×16" },
  { w: 64, h: 32, label: "64×32" },
  { w: 128, h: 64, label: "128×64" },
  { w: 320, h: 180, label: "320×180" },
];

export default function SessionSidebar() {
  const t = useT();
  const sessions = useStore((s) => s.sessions);
  const activeId = useStore((s) => s.activeId);
  const createSession = useStore((s) => s.createSession);
  const removeSession = useStore((s) => s.removeSession);
  const renameSession = useStore((s) => s.renameSession);
  const reorderSessions = useStore((s) => s.reorderSessions);

  const [sizedOpen, setSizedOpen] = useState(false);
  const [width, setWidth] = useState(64);
  const [height, setHeight] = useState(64);
  // 正在改名的会话与草稿。ref 是权威值：失焦提交时 Input 已卸载，
  // 闭包里读 state 会拿到已经清空的旧值，所以以 ref 为准（图层改名同款打法）。
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const renamingRef = useRef<string | null>(null);
  // 拖动排序：只管「谁被拖、现在悬在谁头上」，松手按新次序整批上报。
  const [dragId, setDragId] = useState<string | null>(null);
  const [overId, setOverId] = useState<string | null>(null);

  const beginRename = (id: string, current: string) => {
    renamingRef.current = id;
    setRenamingId(id);
    setRenameDraft(current);
  };
  const commitRename = () => {
    const id = renamingRef.current;
    renamingRef.current = null;
    setRenamingId(null);
    if (id) void renameSession(id, renameDraft);
  };
  const cancelRename = () => {
    renamingRef.current = null;
    setRenamingId(null);
    setRenameDraft("");
  };

  /** 松手落位：把拖着的会话搬到目标槽位， ids 整批交给 Rust 重排序位。 */
  function dropOn(targetId: string) {
    if (!dragId || dragId === targetId) return;
    const ids = sessions.map((session) => session.id);
    const from = ids.indexOf(dragId);
    const to = ids.indexOf(targetId);
    if (from < 0 || to < 0) return;
    ids.splice(to, 0, ...ids.splice(from, 1));
    setDragId(null);
    setOverId(null);
    void reorderSessions(ids);
  }

  return (
    <aside className="panel sidebar">
      <div className="panel-head">
        <strong>{t("sidebar.sessions")}</strong>
        <span className="grow" />
        <Tooltip title={t("sidebar.new")}>
          <Button
            size="small"
            type="text"
            icon={<Plus size={14} />}
            onClick={() => setSizedOpen(true)}
          />
        </Tooltip>
      </div>

      <div className="panel-body">
        <div className="session-list">
          {sessions.map((session) => (
            <div
              key={session.id}
              className={[
                "session-item",
                session.id === activeId ? "active" : "",
                dragId === session.id ? "dragging" : "",
                overId === session.id && dragId !== session.id ? "drop-target" : "",
              ]
                .filter(Boolean)
                .join(" ")}
              draggable={renamingId !== session.id}
              onClick={() => void useStore.getState().selectSession(session.id)}
              onDragStart={(event) => {
                setDragId(session.id);
                // 给拖影一点内容：不然 Firefox 下是个空块，看不出拖的是什么。
                event.dataTransfer.setData("text/plain", session.title ?? session.id);
                event.dataTransfer.effectAllowed = "move";
              }}
              onDragOver={(event) => {
                // 不 preventDefault 就没有 drop 事件，排序整个失效。
                event.preventDefault();
                event.dataTransfer.dropEffect = "move";
                setOverId(session.id);
              }}
              onDragLeave={() => setOverId((id) => (id === session.id ? null : id))}
              onDrop={(event) => {
                event.preventDefault();
                dropOn(session.id);
              }}
              onDragEnd={() => {
                setDragId(null);
                setOverId(null);
              }}
              onContextMenu={(event) =>
                openContextMenu(event, [
                  {
                    key: "rename",
                    label: t("sidebar.rename"),
                    icon: <Pencil size={13} />,
                    onSelect: () => beginRename(session.id, session.title ?? session.id),
                  },
                  {
                    key: "delete",
                    label: t("sidebar.delete"),
                    icon: <Trash2 size={13} />,
                    danger: true,
                    onSelect: () => void removeSession(session.id),
                  },
                ])
              }
            >
              {renamingId === session.id ? (
                <Input
                  autoFocus
                  size="small"
                  className="session-rename"
                  value={renameDraft}
                  maxLength={60}
                  placeholder={t("sidebar.rename_placeholder")}
                  // 行本身 draggable：不拦住拖拽的话，输入框里连光标都选不动。
                  onDragStart={(event) => event.preventDefault()}
                  onClick={(event) => event.stopPropagation()}
                  onChange={(event) => setRenameDraft(event.target.value)}
                  onPressEnter={commitRename}
                  onBlur={commitRename}
                  onKeyDown={(event) => {
                    if (event.key === "Escape") {
                      event.preventDefault();
                      event.stopPropagation();
                      cancelRename();
                    }
                  }}
                />
              ) : (
                <span
                  className="session-id"
                  onDoubleClick={() => beginRename(session.id, session.title ?? session.id)}
                >
                  {session.title ?? session.id}
                </span>
              )}
              <span className="session-actions">
                <Tooltip title={t("sidebar.rename")}>
                  <Button
                    size="small"
                    type="text"
                    icon={<Pencil size={13} />}
                    onClick={(event) => {
                      event.stopPropagation();
                      beginRename(session.id, session.title ?? session.id);
                    }}
                  />
                </Tooltip>
                <Tooltip title={t("sidebar.delete")}>
                  <Button
                    size="small"
                    type="text"
                    danger
                    icon={<Trash2 size={13} />}
                    onClick={(event) => {
                      event.stopPropagation();
                      void removeSession(session.id);
                    }}
                  />
                </Tooltip>
              </span>
              <span className="session-model">{session.model_label || t("sidebar.unbound")}</span>
              <span className="session-meta">
                <span>
                  {session.width}x{session.height}
                </span>
                <span>{t("sidebar.rev", { rev: session.revision })}</span>
              </span>
            </div>
          ))}
        </div>
      </div>

      <div className="sidebar-foot">
        <a className="repo-link" href={REPO_URL} target="_blank" rel="noreferrer">
          <Github size={12} />
          {t("sidebar.repo")}
        </a>
      </div>

      <Modal
        title={t("sidebar.modal_title")}
        open={sizedOpen}
        okText={t("sidebar.modal_ok")}
        cancelText={t("sidebar.modal_cancel")}
        onOk={async () => {
          setSizedOpen(false);
          await createSession(width, height);
        }}
        onCancel={() => setSizedOpen(false)}
        destroyOnHidden
        width={460}
        className="size-modal"
      >
        <div className="size-modal-body">
          <div
            className="size-preview"
            style={{ aspectRatio: `${width} / ${height}` }}
            aria-hidden
          />
          <div className="size-fields">
            <label className="size-field">
              <span className="k">{t("sidebar.width")}</span>
              <InputNumber min={8} max={512} value={width} onChange={(v) => setWidth(v ?? 64)} />
            </label>
            <span className="size-times">×</span>
            <label className="size-field">
              <span className="k">{t("sidebar.height")}</span>
              <InputNumber min={8} max={512} value={height} onChange={(v) => setHeight(v ?? 64)} />
            </label>
          </div>
        </div>

        <div className="size-readout">
          {t("sidebar.size_readout", { width, height, cells: width * height })}
        </div>

        <div className="size-presets">
          <span className="size-presets-label">{t("sidebar.presets")}</span>
          <div className="size-preset-row">
            {SIZE_PRESETS.map((preset) => (
              <Button
                key={preset.label}
                size="small"
                type={preset.w === width && preset.h === height ? "primary" : "default"}
                onClick={() => {
                  setWidth(preset.w);
                  setHeight(preset.h);
                }}
              >
                {preset.label}
              </Button>
            ))}
          </div>
        </div>

        <p className="size-hint">{t("sidebar.size_hint")}</p>
      </Modal>
    </aside>
  );
}
