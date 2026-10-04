// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
import { useCallback, useEffect, useRef, useState } from "react";
import { Button, Input, Modal, Tooltip } from "antd";
import { Github, Pencil, Plus, Trash2 } from "lucide-react";

import { useStore } from "../lib/store";
import type { SessionInfo } from "../lib/types";
import { useT } from "../lib/t";
import { openContextMenu } from "./ContextMenu";

const REPO_URL = "https://github.com/Mutantcat-Working-Group/AIPixel";

export default function SessionSidebar() {
  const t = useT();
  const sessions = useStore((s) => s.sessions);
  const activeId = useStore((s) => s.activeId);
  // 新建走 openCreatePrompt：弹窗挂在 App 上，开机空会话那次自己会弹。
  const openCreatePrompt = useStore((s) => s.openCreatePrompt);
  const removeSession = useStore((s) => s.removeSession);
  const renameSession = useStore((s) => s.renameSession);
  const reorderSessions = useStore((s) => s.reorderSessions);

  // 待确认删除的会话。存整条而不是只存 id：确认弹窗要把名字和画布尺寸一并
  // 摆给用户看（「删的到底是哪一张」是这类确认唯一有意义的信息）。
  const [pendingDrop, setPendingDrop] = useState<SessionInfo | null>(null);
  // 正在改名的会话与草稿。ref 是权威值：失焦提交时 Input 已卸载，
  // 闭包里读 state 会拿到已经清空的旧值，所以以 ref 为准（图层改名同款打法）。
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const renamingRef = useRef<string | null>(null);
  // 拖动排序：只管「谁被拖、现在悬在谁头上」，松手按新次序整批上报。
  const [dragId, setDragId] = useState<string | null>(null);
  const [overId, setOverId] = useState<string | null>(null);

  // 收摊：拖动态清零。dragging 那 45% 不透明度只是视觉，可 dragId 一卡住，
  // 排序本身也跟着失灵（dropOn 见 !dragId 直接 return），所以这个清理必须是
  // 硬保证，不能只依赖元素上的 onDragEnd。
  const clearDrag = useCallback(() => {
    setDragId(null);
    setOverId(null);
  }, []);

  // macOS 上 dragDropEnabled 开着（要收文件拖放），WKWebView 会把页面里的
  // HTML5 拖拽整个交给系统拖拽会话：松手之后 dragend / drop 不一定回得来。
  // 于是 dragId 永远停在那个会话上，.dragging 的半透明跟着卡住——用户看到
  // 的就是「第一项一直是灰的」。这里挂几层兜底：dragend、鼠标松手、窗口失焦、
  // Esc，任何一样到了都收摊。活着的拖拽期间浏览器不会派发 mouseup，所以这些
  // 兜底不会把正常排序掐断。
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") clearDrag();
    };
    window.addEventListener("dragend", clearDrag, true);
    window.addEventListener("mouseup", clearDrag, true);
    window.addEventListener("blur", clearDrag);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("dragend", clearDrag, true);
      window.removeEventListener("mouseup", clearDrag, true);
      window.removeEventListener("blur", clearDrag);
      window.removeEventListener("keydown", onKey);
    };
  }, [clearDrag]);

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
    // 读实时 state 而不是渲染时的闭包：连着拖两次时，后一次的闭包可能还是
    // 上一轮 dragstart 时的列表，搬出来的次序就漏掉中间那次。
    const ids = useStore.getState().sessions.map((session) => session.id);
    const from = ids.indexOf(dragId);
    const to = ids.indexOf(targetId);
    if (from < 0 || to < 0) return;
    ids.splice(to, 0, ...ids.splice(from, 1));
    clearDrag();
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
            onClick={openCreatePrompt}
            aria-label={t("sidebar.new")}
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
              onClick={() => {
                // 点一下就收摊：万一上一轮拖拽的 dragend 丢了（WKWebView 老毛病），
                // 用户点任意会话也能把那条半透明立刻拍掉，不用重启。
                clearDrag();
                void useStore.getState().selectSession(session.id);
              }}
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
                clearDrag();
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
                    onSelect: () => setPendingDrop(session),
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
                    aria-label={t("sidebar.rename")}
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
                      setPendingDrop(session);
                    }}
                    aria-label={t("sidebar.delete")}
                  />
                </Tooltip>
              </span>
              {/* 模型名直接占位：没绑定就显示「未绑定」。留空反而像没加载完，
                  用户会对着它发呆。 */}
              <span className="session-model">{session.model_label || t("sidebar.unbound")}</span>
              <span className="session-meta">
                <span>
                  {session.width}x{session.height}
                </span>
                <span>{t("sidebar.rev", { rev: session.revision })}</span>
              </span>
            </div>
          ))}
          {sessions.length === 0 ? (
            <p className="session-empty">{t("sidebar.empty")}</p>
          ) : null}
        </div>
      </div>

      <div className="sidebar-foot">
        <a className="repo-link" href={REPO_URL} target="_blank" rel="noreferrer">
          <Github size={12} />
          {t("sidebar.repo")}
        </a>
      </div>

      {/* 删会话是整个软件里唯一救不回来的操作：聊天记录和画布一起没，
          撤销栈是编辑器 op 级的，够不着它。所以这里必须先问清楚。 删帧、删图
          层就不用问，那些走编辑器 op，落撤销栈，一步 undo 就回来了。 */}
      <Modal
        open={pendingDrop !== null}
        title={t("sidebar.delete_title")}
        okText={t("sidebar.delete_ok")}
        cancelText={t("sidebar.modal_cancel")}
        okButtonProps={{ danger: true }}
        onOk={async () => {
          const target = pendingDrop;
          if (!target) return;
          try {
            await removeSession(target.id);
            // 只有真的删掉了才收窗：Rust 那头失败的话 store 会把原因顶成一条通知，
            // 窗子留着，用户才好接着重试或者干脆收手。
            setPendingDrop(null);
          } catch {
            // 错误已经由 store 转成通知，这里别再说一遍。
          }
        }}
        onCancel={() => setPendingDrop(null)}
        // 关窗就拆内容：删成功会清掉 pendingDrop，正文那句引用它的插值就不该
        // 还挂在 DOM 上跑第二遍。和另两个弹窗保持一个写法。
        destroyOnHidden
      >
        {pendingDrop ? (
          <p className="session-drop-body">
            {t("sidebar.delete_body", {
              name: pendingDrop.title ?? pendingDrop.id,
              width: pendingDrop.width,
              height: pendingDrop.height,
            })}
          </p>
        ) : null}
      </Modal>

    </aside>
  );
}
