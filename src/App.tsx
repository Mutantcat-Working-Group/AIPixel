import { useEffect, useState } from "react";
import { Button, Segmented, Select, Spin, Tooltip } from "antd";
import { FolderOpen, ImagePlus, Plug, Save, Settings2, X } from "lucide-react";
import { open, save } from "@tauri-apps/plugin-dialog";

import ChatPanel from "./ui/ChatPanel";
import DocumentPanel from "./ui/DocumentPanel";
import BatchPanel from "./ui/BatchPanel";
import ModelSettingsModal from "./ui/ModelSettingsModal";
import McpPanel from "./ui/McpPanel";
import SessionSidebar from "./ui/SessionSidebar";
import StarterGate from "./ui/StarterGate";
import WorkflowDock from "./ui/WorkflowDock";
import { useStore } from "./lib/store";
import { useT } from "./lib/t";
import type { PermissionMode } from "./lib/types";

export default function App() {
  const store = useStore();
  const [rail, setRail] = useState("canvas");

  const t = useT();

  /** 右栏三个视图：画布看结果，工作流跑流程，批量跑文件夹。 */
  const railOptions = [
    { label: t("rail.canvas"), value: "canvas" },
    { label: t("rail.workflows"), value: "workflows" },
    { label: t("rail.batch"), value: "batch" },
  ];

  /** 审批档位。文案要让用户一眼看出「问不问」的差别，所以每档都带 tooltip。 */
  const permissionOptions = [
    { label: t("perm.auto"), value: "auto", title: t("perm.auto.title") },
    { label: t("perm.chat"), value: "chat", title: t("perm.chat.title") },
    { label: t("perm.ask"), value: "ask", title: t("perm.ask.title") },
  ];

  const aipFilter = [{ name: t("dialog.aip"), extensions: ["aip"] }];
  const imageFilter = [{ name: t("dialog.image"), extensions: ["png", "jpg", "jpeg", "webp", "gif"] }];

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
    const picked = await open({ multiple: false, filters: aipFilter });
    if (typeof picked === "string") await store.openAip(picked);
  }

  async function pickAndSaveAip() {
    const target = await save({
      defaultPath: `${documentName}.aip`,
      filters: aipFilter,
    });
    if (typeof target === "string") await store.saveAip(target);
  }

  async function pickReferenceImages() {
    const picked = await open({ multiple: true, filters: imageFilter });
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
          <span className="brand-tag">{t("app.brand_tag")}</span>
        </div>

        <div className="topbar-spacer" />

        <div className="topbar-group">
          <Tooltip title={t("topbar.model")}>
            <Select
              size="small"
              style={{ width: 210 }}
              value={activeSession?.model_id ?? undefined}
              options={store.models.entries.map((m) => ({
                label: `${m.label} · ${m.model}`,
                value: m.id,
              }))}
              onChange={(value: string) => void store.bindSessionModel(value)}
              placeholder={t("topbar.select_model")}
            />
          </Tooltip>

          <Tooltip title={t("perm.tooltip")}>
            <Segmented
              size="small"
              value={store.permission}
              options={permissionOptions.map((o) => ({
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
          <Tooltip title={t("topbar.open_aip")}>
            <Button size="small" type="text" icon={<FolderOpen size={14} />} onClick={pickAndOpenAip} />
          </Tooltip>
          <Tooltip title={t("topbar.save_aip")}>
            <Button size="small" type="text" icon={<Save size={14} />} onClick={pickAndSaveAip} />
          </Tooltip>
          <Tooltip title={t("topbar.attach_reference")}>
            <Button
              size="small"
              type="text"
              icon={<ImagePlus size={14} />}
              onClick={pickReferenceImages}
            />
          </Tooltip>
          <Tooltip title={t("topbar.settings")}>
            <Button
              size="small"
              type="text"
              icon={<Settings2 size={14} />}
              onClick={store.openSettings}
            />
          </Tooltip>
          <Tooltip title={t("topbar.mcp")}>
            <Button
              size="small"
              type="text"
              icon={<Plug size={14} />}
              onClick={store.openMcp}
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
              options={railOptions}
              onChange={(next) => setRail(next as string)}
            />
          </div>
          {rail === "canvas" ? <DocumentPanel /> : null}
          {rail === "workflows" ? <WorkflowDock /> : null}
          {rail === "batch" ? <BatchPanel /> : null}
        </div>
      </div>

      <ModelSettingsModal />
      <McpPanel />
    </div>
  );
}
