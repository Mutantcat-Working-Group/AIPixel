// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
import { useEffect, useState } from "react";
import { Alert, Button, Checkbox, Input, Modal, Segmented, Tag, Tooltip } from "antd";
import { Plus, Trash2 } from "lucide-react";

import { useStore } from "../lib/store";
import { useT, type T } from "../lib/t";
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

export default function McpPanel() {
  const t = useT();
  const open = useStore((s) => s.mcpOpen);
  const servers = useStore((s) => s.mcpServers.entries);
  const busy = useStore((s) => s.mcpBusy);
  const closeMcp = useStore((s) => s.closeMcp);
  const upsertMcpServer = useStore((s) => s.upsertMcpServer);
  const removeMcpServer = useStore((s) => s.removeMcpServer);
  const connectMcpServer = useStore((s) => s.connectMcpServer);
  const disconnectMcpServer = useStore((s) => s.disconnectMcpServer);

  /** null = 列表态；"new" 或服务器名 = 表单态。 */
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState<DraftShape>(EMPTY_DRAFT);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) {
      setEditing(null);
      setError(null);
    }
  }, [open]);

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
    <Modal
      title={t("mcp.title")}
      open={open}
      onCancel={closeMcp}
      width={640}
      className="mcp-modal"
      footer={[
        <Button key="close" onClick={closeMcp}>
          {t("mcp.close")}
        </Button>,
        editing ? (
          <Button key="save" type="primary" loading={busy} onClick={save}>
            {t("mcp.save")}
          </Button>
        ) : null,
      ]}
    >
      <div className="mcp-list">
        {servers.length === 0 ? (
          <div className="mcp-empty">
            {t("mcp.empty")}
          </div>
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
                <Button
                  size="small"
                  type="primary"
                  disabled={busy}
                  onClick={() => void connectMcpServer(server.name)}
                >
                  {t("mcp.connect")}
                </Button>
              )}
              <Button size="small" disabled={busy} onClick={() => startEdit(server)}>
                {t("mcp.edit")}
              </Button>
              <Button
                size="small"
                danger
                icon={<Trash2 size={13} />}
                disabled={busy}
                onClick={() => void removeMcpServer(server.name)}
              />
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
          <span className="inline-note">
            {t("mcp.credentials_note")}
          </span>
          <div className="mcp-form-actions">
            <span className="grow" />
            <Button onClick={() => setEditing(null)}>{t("mcp.cancel")}</Button>
          </div>
        </div>
      ) : null}
    </Modal>
  );
}
