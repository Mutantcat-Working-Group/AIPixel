// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
import { useState } from "react";
import {
  Alert,
  Button,
  Checkbox,
  Input,
  InputNumber,
  Popconfirm,
  Segmented,
  Switch,
  Tag,
  Tooltip,
} from "antd";
import { ClipboardCopy, Plus, Trash2 } from "lucide-react";

import { useStore } from "../lib/store";
import { useT, type T } from "../lib/t";
import type { McpServerStatusView } from "../lib/types";
import type {
  McpServerConfig,
  McpServerView,
  McpTransportConfig,
} from "../lib/types";

interface DraftShape {
  name: string;
  kind: "stdio" | "http";
  command: string;
  /** 每行一个参数。 */
  args: string;
  /** 每行一个 KEY=VALUE；值留空表示沿用本机已存值。 */
  env: string;
  url: string;
  headers: string;
  auto_connect: boolean;
}

const EMPTY_DRAFT: DraftShape = {
  name: "",
  kind: "stdio",
  command: "",
  args: "",
  env: "",
  url: "",
  headers: "",
  auto_connect: false,
};

const KIND_OPTIONS = [
  { label: "Stdio", value: "stdio" },
  { label: "HTTP", value: "http" },
];

/** 服务端对外那些工具的清单。名字必须与 Rust 侧 `TOOL_SPECS` 逐字对齐，
 *  这里只负责把说明摊给用户看，不参与调用。 */
const SERVER_TOOLS: { name: string; hint: string }[] = [
  { name: "list_sessions", hint: "列出全部画布会话，含宽高与帧数" },
  { name: "create_canvas", hint: "按指定宽高开一个新画布" },
  { name: "drop_canvas", hint: "关掉并丢弃一个画布会话" },
  { name: "rename_canvas", hint: "给画布会话改个显示名" },
  { name: "get_canvas", hint: "读画布全貌：图层、帧、调色板、像素统计" },
  { name: "canvas_preview", hint: "导出当前帧的 PNG base64，给视觉模型看" },
  { name: "paint_stroke", hint: "在指定图层画一条折线笔触" },
  { name: "fill_region", hint: "从某个像素开始做四邻域填充" },
  { name: "apply_ops", hint: "批量套用点/线/矩形/填充等算子，带整体回滚" },
  { name: "resize_canvas", hint: "改画布宽高，可选是否保留原内容" },
  { name: "list_export_formats", hint: "列出全部可导出的文件格式" },
  { name: "export_canvas", hint: "把画布写到调用方指定的磁盘路径" },
  { name: "save_project", hint: "把当前工程存成 .aip 文件" },
  { name: "import_project", hint: "读入一个 .aip 工程文件" },
  { name: "import_image", hint: "把一张位图降采样量化进画布（外部 AI 自己的生图模型）" },
  { name: "prompt_agent", hint: "用主智能体干一件事（生成、修改、分析）" },
  { name: "interrupt_agent", hint: "让正在跑的智能体立刻停下" },
];

function linesToList(text: string): string[] {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

function linesToMap(text: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const line of linesToList(text)) {
    const at = line.indexOf("=");
    if (at <= 0) continue;
    out[line.slice(0, at).trim()] = line.slice(at + 1).trim();
  }
  return out;
}

function statusTag(server: McpServerView, t: T) {
  if (server.connected) {
    return <Tag color="green">{t("mcp.connected")}</Tag>;
  }
  if (server.last_error) {
    return <Tag color="red">{t("mcp.error")}</Tag>;
  }
  return <Tag>{t("mcp.offline")}</Tag>;
}

/** 运行状态的小牌子。running / enabled / enabled-but-broken 三态要说人话。 */
function serverStateTag(status: McpServerStatusView, t: T) {
  if (status.running) {
    return <Tag color="green">{t("mcps.running")}</Tag>;
  }
  if (status.enabled && status.last_error) {
    return <Tag color="red">{t("mcp.error")}</Tag>;
  }
  return <Tag>{t("mcps.stopped")}</Tag>;
}

/** 端点地址行：右边一个复制按钮，省得用户在浏览器和编辑器之间手抄。 */
function EndpointRow({ status }: { status: McpServerStatusView }) {
  const t = useT();
  const [copied, setCopied] = useState(false);

  async function copy() {
    try {
      await navigator.clipboard.writeText(status.endpoint);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    } catch {
      // 剪贴板不给就安静退场：地址本身还摆在那儿，用户看得见。
    }
  }

  return (
    <div className="mcps-endpoint">
      <code>{status.endpoint}</code>
      <Tooltip title={copied ? t("mcps.copied") : t("mcps.copy")}>
        <Button
          size="small"
          aria-label={t("mcps.copy")}
          icon={<ClipboardCopy size={13} />}
          onClick={() => void copy()}
        />
      </Tooltip>
    </div>
  );
}

/** 「本程序当服务端」卡片：让外部 AI / 游戏引擎反过来驱动本程序的画布。
 *  开关默认是关的——监听端口等于把写文件的能力递出去，得由人点头。 */
function McpServerCard() {
  const t = useT();
  const status = useStore((s) => s.mcpServerStatus);
  const busy = useStore((s) => s.mcpServerBusy);
  const setEnabled = useStore((s) => s.setMcpServerEnabled);
  const setPort = useStore((s) => s.setMcpServerPort);
  const restart = useStore((s) => s.restartMcpServer);

  if (!status) return null;

  return (
    <div className="mcps-card">
      <div className="mcp-row-head">
        <span className="mcp-row-name">{t("mcps.title")}</span>
        {serverStateTag(status, t)}
        <span className="grow" />
        <Switch
          size="small"
          checked={status.enabled}
          loading={busy}
          onChange={(next) => void setEnabled(next)}
        />
      </div>
      <p className="settings-blurb">{t("mcps.hint")}</p>
        <div className="mcps-grid">
          <Tooltip title={t("mcps.port_hint")}>
          <label className="mcps-field">
            <span>{t("mcps.port")}</span>
            <InputNumber
              size="small"
            min={0}
            max={65535}
            value={status.port}
            disabled={busy}
            onChange={(value) => void setPort(Number(value) || 0)}
          />
          </label>
          </Tooltip>
        <div className="mcps-field">
          <span>{t("mcps.requests")}</span>
          <span className="mcps-count">{status.requests}</span>
        </div>
        <div className="mcps-field">
          <span>{t("mcps.restart")}</span>
          <Button size="small" disabled={busy || !status.enabled} onClick={() => void restart()}>
            {t("mcps.restart_action")}
          </Button>
        </div>
      </div>
      {status.endpoint ? <EndpointRow status={status} /> : null}
      {status.last_error ? <div className="mcp-error">{status.last_error}</div> : null}
      <div className="mcps-tools">
        <span className="mcps-tools-title">
          {t("mcps.tools", { count: SERVER_TOOLS.length })}
        </span>
        {SERVER_TOOLS.map((tool) => (
          <Tooltip key={tool.name} title={tool.hint}>
            <span className="mcp-tool">{tool.name}</span>
          </Tooltip>
        ))}
      </div>
    </div>
  );
}

/** MCP 一整块：总闸加服务器去留。以前顶栏那个独立弹窗并进了设置，
 *  所以草稿跟着设置弹窗一起生死——关掉设置就当没填过，下次重新开。 */
export default function McpSection() {
  const t = useT();
  const settingsOpen = useStore((s) => s.settingsOpen);
  const enabled = useStore((s) => s.mcpEnabled);
  const setEnabled = useStore((s) => s.setMcpEnabled);
  const servers = useStore((s) => s.mcpServers.entries);
  const busy = useStore((s) => s.mcpBusy);
  const upsertMcpServer = useStore((s) => s.upsertMcpServer);
  const removeMcpServer = useStore((s) => s.removeMcpServer);
  const connectMcpServer = useStore((s) => s.connectMcpServer);
  const disconnectMcpServer = useStore((s) => s.disconnectMcpServer);

  /** null = 列表态；"new" 或服务器名 = 表单态。 */
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState<DraftShape>(EMPTY_DRAFT);
  const [error, setError] = useState<string | null>(null);

  // 设置窗一关就收回编辑态：settingsOpen 由真转假的那次渲染里就地收，
  // 不进 effect——effect 体内同步 setState 会多一次提交。
  const [openSeen, setOpenSeen] = useState(settingsOpen);
  if (settingsOpen !== openSeen) {
    setOpenSeen(settingsOpen);
    if (!settingsOpen) {
      setEditing(null);
      setError(null);
    }
  }

  function startNew() {
    setError(null);
    setDraft(EMPTY_DRAFT);
    setEditing("new");
  }

  function startEdit(server: McpServerView) {
    setError(null);
    // 凭据只回键名：预填 KEY= 空值，保存时留空即沿用本机旧值。
    setDraft({
      name: server.name,
      kind: server.transport.kind,
      command: server.transport.command,
      args: server.transport.args.join("\n"),
      env: server.transport.env_keys.map((key) => `${key}=`).join("\n"),
      url: server.transport.url,
      headers: server.transport.header_keys.map((key) => `${key}=`).join("\n"),
      auto_connect: server.auto_connect,
    });
    setEditing(server.name);
  }

  function patch(patchValue: Partial<DraftShape>) {
    setDraft((current) => ({ ...current, ...patchValue }));
  }

  async function save() {
    setError(null);
    if (!draft.name.trim()) {
      setError(t("mcp.name_required"));
      return;
    }
    let transport: McpTransportConfig;
    if (draft.kind === "stdio") {
      if (!draft.command.trim()) {
        setError(t("mcp.command_required"));
        return;
      }
      transport = {
        kind: "stdio",
        command: draft.command.trim(),
        args: linesToList(draft.args),
        env: linesToMap(draft.env),
      };
    } else {
      if (!/^https?:\/\//.test(draft.url.trim())) {
        setError(t("mcp.url_required"));
        return;
      }
      transport = {
        kind: "http",
        url: draft.url.trim(),
        headers: linesToMap(draft.headers),
      };
    }
    const config: McpServerConfig = {
      name: draft.name.trim(),
      transport,
      auto_connect: draft.auto_connect,
    };
    try {
      await upsertMcpServer(config);
      setEditing(null);
    } catch (cause) {
      setError(String(cause));
    }
  }

  return (
  <section className="settings-section mcp-section">
      <div className="settings-section-title">
        <span>{t("settings.mcp")}</span>
        <span className="grow section-hint">
          {enabled ? t("settings.mcp_on") : t("settings.mcp_off")}
        </span>
        <Switch
          size="small"
          checked={enabled}
          onChange={(next) => void setEnabled(next)}
        />
      </div>
      <p className="settings-blurb">{t("settings.mcp_hint")}</p>

      <McpServerCard />

      <div className="mcp-list">
        {servers.length === 0 ? (
          <div className="mcp-empty">{t("mcp.empty")}</div>
        ) : null}
        {servers.map((server) => (
          <div key={server.name} className={`mcp-row ${editing === server.name ? "active" : ""}`}>
            <div className="mcp-row-head">
              <span className="mcp-row-name">{server.name}</span>
              <Tag>{server.transport.kind}</Tag>
              {statusTag(server, t)}
              {server.auto_connect ? (
                <Tooltip title={t("mcp.auto_hint")}>
                  <Tag color="blue">{t("mcp.auto")}</Tag>
                </Tooltip>
              ) : null}
              <span className="grow" />
              {server.connected ? (
                <Button
                  size="small"
                  disabled={busy}
                  onClick={() => void disconnectMcpServer(server.name)}
                >
                  {t("mcp.disconnect")}
                </Button>
              ) : (
                <Tooltip title={enabled ? "" : t("mcp.connect_disabled")}>
                  <Button
                    size="small"
                    type="primary"
                    disabled={busy || !enabled}
                    onClick={() => void connectMcpServer(server.name)}
                  >
                    {t("mcp.connect")}
                  </Button>
                </Tooltip>
              )}
              <Button size="small" disabled={busy} onClick={() => startEdit(server)}>
                {t("mcp.edit")}
              </Button>
              {/* 服务器连上之后它的工具已经进了 agent 的工具箱，删掉等于把一整套
                  工具从流程里抽走，所以也先问一句。行内弹一下即可，不开大窗。 */}
              <Popconfirm
                title={t("mcp.delete_confirm", { name: server.name })}
                okText={t("mcp.delete_ok")}
                cancelText={t("mcp.delete_cancel")}
                okButtonProps={{ danger: true }}
                disabled={busy}
                onConfirm={() => void removeMcpServer(server.name)}
              >
                <Button
                  size="small"
                  danger
                  aria-label={t("mcp.delete")}
                  icon={<Trash2 size={13} />}
                  disabled={busy}
                />
              </Popconfirm>
            </div>
            {server.last_error ? <div className="mcp-error">{server.last_error}</div> : null}
            {server.tools.length > 0 ? (
              <div className="mcp-tools">
                {server.tools.map((tool) => (
                  <Tooltip
                    key={tool.name}
                    title={tool.description || t("mcp.no_description")}
                  >
                    <span className="mcp-tool">{tool.name}</span>
                  </Tooltip>
                ))}
              </div>
            ) : null}
          </div>
        ))}
        <Button
          block
          type="dashed"
          icon={<Plus size={13} />}
          disabled={busy}
          onClick={startNew}
        >
          {t("mcp.add_server")}
        </Button>
      </div>

      {editing ? (
        <div className="mcp-form">
          <div className="mcp-form-title">
            {editing === "new"
              ? t("mcp.new_server")
              : t("mcp.edit_server", { name: editing })}
          </div>
          <label className="mcp-field">
            <span>{t("mcp.name")}</span>
            <Input
              value={draft.name}
              disabled={editing !== "new"}
              placeholder={t("mcp.name_placeholder")}
              spellCheck={false}
              onChange={(event) => patch({ name: event.target.value })}
            />
          </label>
          <label className="mcp-field">
            <span>{t("mcp.transport")}</span>
            <Segmented
              value={draft.kind}
              options={KIND_OPTIONS}
              onChange={(value) => patch({ kind: value as DraftShape["kind"] })}
            />
          </label>
          {draft.kind === "stdio" ? (
            <>
              <label className="mcp-field">
                <span>{t("mcp.command")}</span>
                <Input
                  value={draft.command}
                  placeholder={t("mcp.command_placeholder")}
                  spellCheck={false}
                  onChange={(event) => patch({ command: event.target.value })}
                />
              </label>
              <label className="mcp-field">
                <span>{t("mcp.args")}</span>
                <Input.TextArea
                  value={draft.args}
                  rows={2}
                  placeholder={t("mcp.args_placeholder")}
                  spellCheck={false}
                  onChange={(event) => patch({ args: event.target.value })}
                />
              </label>
              <label className="mcp-field">
                <span>{t("mcp.env")}</span>
                <Input.TextArea
                  value={draft.env}
                  rows={2}
                  placeholder={t("mcp.env_placeholder")}
                  spellCheck={false}
                  onChange={(event) => patch({ env: event.target.value })}
                />
              </label>
            </>
          ) : (
            <>
              <label className="mcp-field">
                <span>{t("mcp.url")}</span>
                <Input
                  value={draft.url}
                  placeholder={t("mcp.url_placeholder")}
                  spellCheck={false}
                  onChange={(event) => patch({ url: event.target.value })}
                />
              </label>
              <label className="mcp-field">
                <span>{t("mcp.headers")}</span>
                <Input.TextArea
                  value={draft.headers}
                  rows={2}
                  placeholder={t("mcp.headers_placeholder")}
                  spellCheck={false}
                  onChange={(event) => patch({ headers: event.target.value })}
                />
              </label>
            </>
          )}
          <Checkbox
            checked={draft.auto_connect}
            onChange={(event) => patch({ auto_connect: event.target.checked })}
          >
            {t("mcp.auto_connect")}
          </Checkbox>
          {error ? <Alert type="error" message={error} showIcon /> : null}
          <span className="inline-note">{t("mcp.credentials_note")}</span>
          <div className="mcp-form-actions">
            <span className="grow" />
            <Button onClick={() => setEditing(null)}>{t("mcp.cancel")}</Button>
            <Button type="primary" loading={busy} onClick={() => void save()}>
              {t("mcp.save")}
            </Button>
          </div>
        </div>
      ) : null}
    </section>
  );
}
