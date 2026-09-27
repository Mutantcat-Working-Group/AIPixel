import { useEffect, useState } from "react";
import { Button, Segmented, Select, Spin, Tooltip } from "antd";
import { FolderOpen, ImagePlus, Save, Settings2, X } from "lucide-react";
import { open, save } from "@tauri-apps/plugin-dialog";

import ChatPanel from "./ui/ChatPanel";
import DocumentPanel from "./ui/DocumentPanel";
import ModelSettingsModal from "./ui/ModelSettingsModal";
import SessionSidebar from "./ui/SessionSidebar";
import StarterGate from "./ui/StarterGate";
import WorkflowDock from "./ui/WorkflowDock";
import { useStore } from "./lib/store";
import type { PermissionMode } from "./lib/types";

/** 右栏两个视图：画布看结果，工作流跑流程。 */
const RAIL_OPTIONS = [
  { label: "Canvas", value: "canvas" },
  { label: "Workflows", value: "workflows" },
];

const PERMISSION_OPTIONS = [
  { label: "Auto", value: "auto", title: "Run tools straight away, never ask" },
  { label: "Chat", value: "chat", title: "Reads run free, writes ask one by one" },
  { label: "Ask", value: "ask", title: "Ask before every tool call" },
];

const AIP_FILTER = [{ name: "AIP", extensions: ["aip"] }];
const IMAGE_FILTER = [{ name: "图片", extensions: ["png", "jpg", "jpeg", "webp", "gif"] }];

export default function App() {
  const store = useStore();
  const [rail, setRail] = useState("canvas");

  useEffect(() => {
    void store.boot();
    // store 是模块级单例，只在挂载时启动一次
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  if (!store.booted) {
    return (
      <div className="gate">
        <Spin size="large" />
      </div>
    );
  }

  if (store.models.entries.length === 0) {
    return <StarterGate />;
  }

  const activeSession = store.sessions.find((s) => s.id === store.activeId) ?? null;
  const documentName = store.document?.name ?? "untitled";

  async function pickAndOpenAip() {
    const picked = await open({ multiple: false, filters: AIP_FILTER });
    if (typeof picked === "string") await store.openAip(picked);
  }

  async function pickAndSaveAip() {
    const target = await save({
      defaultPath: `${documentName}.aip`,
      filters: AIP_FILTER,
    });
    if (typeof target === "string") await store.saveAip(target);
  }

  async function pickReferenceImages() {
    const picked = await open({ multiple: true, filters: IMAGE_FILTER });
    if (!picked) return;
    const paths = Array.isArray(picked) ? picked : [picked];
    await store.attachReferenceImages(paths);
  }

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          <span className="brand-mark" aria-hidden>
            <span />
            <span />
            <span />
            <span />
            <span />
            <span />
            <span />
            <span />
            <span />
          </span>
          AIPixel
          <span className="brand-tag">AGENT</span>
        </div>

        <div className="topbar-spacer" />

        <div className="topbar-group">
          <Tooltip title="当前会话使用的模型（自带 Provider，无登录无计费）">
            <Select
              size="small"
              style={{ width: 210 }}
              value={activeSession?.model_id ?? undefined}
              options={store.models.entries.map((m) => ({
                label: `${m.label} · ${m.model}`,
                value: m.id,
              }))}
              onChange={(value: string) => void store.bindSessionModel(value)}
              placeholder="选择模型"
            />
          </Tooltip>

          <Tooltip title="Permission mode: how much the agent may do on its own">
            <Segmented
              size="small"
              value={store.permission}
              options={PERMISSION_OPTIONS.map((o) => ({
                label: o.label,
                value: o.value,
                title: o.title,
              }))}
              onChange={(value) =>
                void store.setPermissionMode(value as PermissionMode)
              }
            />
          </Tooltip>
        </div>

        <div className="topbar-group">
          <Tooltip title="打开 .aip 文件（v2 或旧版）">
            <Button size="small" type="text" icon={<FolderOpen size={14} />} onClick={pickAndOpenAip} />
          </Tooltip>
          <Tooltip title="把当前文档另存为 .aip v2 文本">
            <Button size="small" type="text" icon={<Save size={14} />} onClick={pickAndSaveAip} />
          </Tooltip>
          <Tooltip title="载入参考图，随下一条消息一起发给模型">
            <Button
              size="small"
              type="text"
              icon={<ImagePlus size={14} />}
              onClick={pickReferenceImages}
            />
          </Tooltip>
          <Tooltip title="模型与 Provider 设置">
            <Button
              size="small"
              type="text"
              icon={<Settings2 size={14} />}
              onClick={store.openSettings}
            />
          </Tooltip>
        </div>
      </header>

      {store.notice ? (
        <div className={`notice-bar ${store.notice.isError ? "error" : ""}`}>
          <span className="grow">{store.notice.text}</span>
          <Button size="small" type="text" icon={<X size={13} />} onClick={store.clearNotice} />
        </div>
      ) : null}

      <div className="app-body">
        <SessionSidebar />
        <ChatPanel />
        <div className="rail">
          <div className="rail-switch">
            <Segmented
              size="small"
              value={rail}
              options={RAIL_OPTIONS}
              onChange={(next) => setRail(next as string)}
            />
          </div>
          {rail === "canvas" ? <DocumentPanel /> : <WorkflowDock />}
        </div>
      </div>

      <ModelSettingsModal />
    </div>
  );
}
