import { useState } from "react";
import { Button, InputNumber, Modal, Tooltip } from "antd";
import { Github, Plus, Trash2 } from "lucide-react";

import { useStore } from "../lib/store";
import { useT } from "../lib/t";

const REPO_URL = "https://github.com/Mutantcat-Working-Group/AIPixel";

export default function SessionSidebar() {
  const t = useT();
  const sessions = useStore((s) => s.sessions);
  const activeId = useStore((s) => s.activeId);
  const createSession = useStore((s) => s.createSession);
  const removeSession = useStore((s) => s.removeSession);

  const [sizedOpen, setSizedOpen] = useState(false);
  const [width, setWidth] = useState(64);
  const [height, setHeight] = useState(64);

  async function createDefault() {
    await createSession();
  }

  return (
    <aside className="panel sidebar">
      <div className="panel-head">
        <strong>{t("sidebar.sessions")}</strong>
        <span className="grow" />
        <Tooltip title={t("sidebar.new")}>
          <Button size="small" type="text" icon={<Plus size={14} />} onClick={createDefault} />
        </Tooltip>
        <Tooltip title={t("sidebar.new_sized")}>
          <Button size="small" type="text" onClick={() => setSizedOpen(true)}>
            WxH
          </Button>
        </Tooltip>
      </div>

      <div className="panel-body">
        <div className="session-list">
          {sessions.map((session) => (
            <div
              key={session.id}
              className={`session-item ${session.id === activeId ? "active" : ""}`}
              onClick={() => void useStore.getState().selectSession(session.id)}
            >
              <span className="session-id">{session.id}</span>
              <span className="session-actions">
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
      >
        <div className="gate-points">
          <label className="gate-point">
            <span className="k">{t("sidebar.width")}</span>
            <InputNumber min={8} max={512} value={width} onChange={(v) => setWidth(v ?? 64)} />
          </label>
          <label className="gate-point">
            <span className="k">{t("sidebar.height")}</span>
            <InputNumber min={8} max={512} value={height} onChange={(v) => setHeight(v ?? 64)} />
          </label>
        </div>
      </Modal>
    </aside>
  );
}
