import { useState } from "react";
import { Button, InputNumber, Modal, Tooltip } from "antd";
import { Github, Plus, Trash2 } from "lucide-react";

import { useStore } from "../lib/store";

const REPO_URL = "https://github.com/Mutantcat-Working-Group/AIPixel";

export default function SessionSidebar() {
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
        <strong>Sessions</strong>
        <span className="grow" />
        <Tooltip title="新建空白会话">
          <Button size="small" type="text" icon={<Plus size={14} />} onClick={createDefault} />
        </Tooltip>
        <Tooltip title="指定画布尺寸新建">
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
                <Tooltip title="删除会话">
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
              <span className="session-model">{session.model_label || "unbound"}</span>
              <span className="session-meta">
                <span>
                  {session.width}x{session.height}
                </span>
                <span>rev {session.revision}</span>
              </span>
            </div>
          ))}
        </div>
      </div>

      <div className="sidebar-foot">
        <a className="repo-link" href={REPO_URL} target="_blank" rel="noreferrer">
          <Github size={12} />
          AIPixel on GitHub
        </a>
      </div>

      <Modal
        title="New canvas"
        open={sizedOpen}
        okText="Create"
        cancelText="Cancel"
        onOk={async () => {
          setSizedOpen(false);
          await createSession(width, height);
        }}
        onCancel={() => setSizedOpen(false)}
        destroyOnHidden
      >
        <div className="gate-points">
          <label className="gate-point">
            <span className="k">W</span>
            <InputNumber min={8} max={512} value={width} onChange={(v) => setWidth(v ?? 64)} />
          </label>
          <label className="gate-point">
            <span className="k">H</span>
            <InputNumber min={8} max={512} value={height} onChange={(v) => setHeight(v ?? 64)} />
          </label>
        </div>
      </Modal>
    </aside>
  );
}
