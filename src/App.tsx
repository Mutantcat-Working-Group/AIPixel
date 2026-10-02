// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
import { useEffect, useState } from "react";
import { Button, Dropdown, Segmented, Select, Spin, Tooltip } from "antd";
import { Download, FolderOpen, ImagePlus, Save, Settings2, X } from "lucide-react";
import { open, save } from "@tauri-apps/plugin-dialog";

import ChatPanel from "./ui/ChatPanel";
import DocumentPanel from "./ui/DocumentPanel";
import BatchPanel from "./ui/BatchPanel";
import ModelSettingsModal from "./ui/ModelSettingsModal";
import SessionSidebar from "./ui/SessionSidebar";
import WorkflowDock from "./ui/WorkflowDock";
import ContextMenuHost from "./ui/ContextMenu";
import useEditShortcuts from "./ui/useEditShortcuts";
import { useStore } from "./lib/store";
import {
  modelChangeTarget,
  needsModelBanner,
  resolveBoundModelId,
} from "./lib/model-binding";
import { useT } from "./lib/t";
import type { PermissionMode } from "./lib/types";
import type { ExportFormat } from "./lib/bridge";

// 图标取 icon.png 抠掉白背景的版本；像素画素材声明成 URL，别让构建器给它改尺寸。
import brandIcon from "./assets/brand-icon.png";

/**
 * 顶栏模型选择器的「没绑模型」占位值。
 * 会话没配上模型时 Rust 给的 model_id 是 "unset"，用一个查不到的 id + 一条同名选项，
 * 选择器就能把「未设置模型」当选中项显示出来，而不是显示一串内部字面量。
 */
const UNSET_MODEL = "__unset__";

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

  // 撤销 / 重做的键盘入口：画布改一步就能一键反悔，不依赖右上角那两个小方块。
  useEditShortcuts();

  if (!store.booted) {
    return (
      <div className="gate">
        <Spin size="large" />
      </div>
    );
  }

  const activeSession = store.sessions.find((s) => s.id === store.activeId) ?? null;
  // 顶栏显示会话绑的模型；没有会话（允许的空态）或那个模型被删了，就落到全局激活模型。
  // 谁都不剩才是真的「未设置模型」——不能把「还没建会话」也算成没配模型。
  const boundModelId =
    resolveBoundModelId(
      activeSession?.model_id,
      store.models.active_id,
      store.models.entries.map((m) => m.id),
    ) ?? UNSET_MODEL;
  const documentName = store.document?.name ?? "untitled";
  // 「没有可用模型」只在真的一个都没配时出现。空会话下配好了模型却挂这条横幅，
  // 是以前最误导人的一桩：用户照它点进设置，只会发现模型早就在那儿。
  const unbound = needsModelBanner(store.models.entries.length);

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

  /**
   * 导出表：格式、默认文件名后缀、对话框过滤器一次说清，剩下的只是「存哪儿」。
   * 单帧导出带当前帧号，免得每次导出都覆盖上一帧的图。
   */
  const exportEntries: {
    format: ExportFormat;
    label: string;
    extension: string;
    filter: string;
    suffix: string;
  }[] = [
    {
      format: "gif",
      label: t("topbar.export_gif"),
      extension: "gif",
      filter: t("dialog.gif"),
      suffix: "",
    },
    {
      format: "frame",
      label: t("topbar.export_frame"),
      extension: "png",
      filter: t("dialog.png"),
      suffix: `-f${store.frameIndex + 1}`,
    },
    {
      format: "strip",
      label: t("topbar.export_strip"),
      extension: "png",
      filter: t("dialog.png"),
      suffix: "-strip",
    },
    {
      format: "sheet",
      label: t("topbar.export_sheet"),
      extension: "png",
      filter: t("dialog.png"),
      suffix: "-sheet",
    },
    {
      format: "ase",
      label: t("topbar.export_ase"),
      extension: "ase",
      filter: t("dialog.aseprite"),
      suffix: "",
    },
  ];

  async function exportAs(entry: (typeof exportEntries)[number]) {
    const target = await save({
      defaultPath: `${documentName}${entry.suffix}.${entry.extension}`,
      filters: [{ name: entry.filter, extensions: [entry.extension] }],
    });
    if (typeof target !== "string") return;
    await store.exportDocument(entry.format, target, {
      // 只有单帧导出认 frame；别的格式传了 Rust 也当没看见。
      frame: entry.format === "frame" ? store.frameIndex : undefined,
    });
  }

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          <img
            className="brand-mark"
            src={brandIcon}
            alt=""
            width={20}
            height={20}
          />
          AIPixel
          <span className="brand-tag">{t("app.brand_tag")}</span>
        </div>

        <div className="topbar-spacer" />

        <div className="topbar-group">
          <Tooltip title={t("topbar.model")}>
            <Select
              size="small"
              style={{ width: 210 }}
              value={boundModelId}
              options={[
                ...(unbound
                  ? [
                      {
                        label: t("topbar.model_unset"),
                        value: UNSET_MODEL,
                        disabled: true,
                      },
                    ]
                  : []),
                ...store.models.entries.map((m) => ({
                  label: `${m.label} · ${m.model}`,
                  value: m.id,
                })),
              ]}
              onChange={(value: string) => {
                // 有会话只改这个会话的绑定；空会话时这句话无处可绑，改全局激活模型。
                if (modelChangeTarget(activeSession !== null) === "session") {
                  void store.bindSessionModel(value);
                } else {
                  void store.activateModel(value);
                }
              }}
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
            {/* aria-label 与悬停提示同文：纯图标按钮对读屏软件得有个名字，
                光靠 Tooltip 的话鼠标没点上去之前它什么都不是。 */}
            <Button
              size="small"
              type="text"
              aria-label={t("topbar.open_aip")}
              icon={<FolderOpen size={14} />}
              onClick={pickAndOpenAip}
            />
          </Tooltip>
          <Tooltip title={t("topbar.save_aip")}>
            <Button
              size="small"
              type="text"
              aria-label={t("topbar.save_aip")}
              icon={<Save size={14} />}
              onClick={pickAndSaveAip}
            />
          </Tooltip>
          <Tooltip title={t("topbar.export")}>
            <Dropdown
              trigger={["click"]}
              menu={{
                items: exportEntries.map((entry) => ({
                  key: entry.format,
                  label: entry.label,
                })),
                onClick: ({ key }) => {
                  const entry = exportEntries.find(
                    (item) => item.format === key,
                  );
                  if (entry) void exportAs(entry);
                },
              }}
            >
              <Button
                size="small"
                type="text"
                aria-label={t("topbar.export")}
                icon={<Download size={14} />}
                disabled={!store.document || store.busy}
              />
            </Dropdown>
          </Tooltip>
          <Tooltip title={t("topbar.attach_reference")}>
            <Button
              size="small"
              type="text"
              icon={<ImagePlus size={14} />}
              onClick={pickReferenceImages}
              aria-label={t("topbar.attach_reference")}
            />
          </Tooltip>
          <Tooltip title={t("topbar.settings")}>
            <Button
              size="small"
              type="text"
              aria-label={t("topbar.settings")}
              icon={<Settings2 size={14} />}
              onClick={store.openSettings}
            />
          </Tooltip>
        </div>
      </header>

      {/* 两条横幅共用一个 wrapper：.app 只留两行网格时，多余的直接子节点会掉进隐式行、
          把正文整块往下顶；包起来后没有横幅时这里高度为 0，只见顶栏一条栏。 */}
      <div className="app-notices">
        {unbound ? (
          <div className="notice-bar">
            <span className="grow">{t("store.no_model")}</span>
            <Button size="small" type="link" onClick={store.openSettings}>
              {t("gate.add_first")}
            </Button>
          </div>
        ) : null}

        {store.notice ? (
          <div className={`notice-bar ${store.notice.isError ? "error" : ""}`}>
            <span className="grow">{store.notice.text}</span>
            <Button
              size="small"
              type="text"
              aria-label={t("notice.dismiss")}
              icon={<X size={13} />}
              onClick={store.clearNotice}
            />
          </div>
        ) : null}
      </div>

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
      <ContextMenuHost />
    </div>
  );
}
