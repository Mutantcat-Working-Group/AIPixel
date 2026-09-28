import { useState } from "react";
import { Button, InputNumber, Modal, Tooltip } from "antd";
import { Github, Plus, Trash2 } from "lucide-react";

import { useStore } from "../lib/store";
import { useT } from "../lib/t";

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
