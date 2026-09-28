import { useEffect, useState } from "react";
import {
  Alert,
  Button,
  Checkbox,
  Form,
  Input,
  InputNumber,
  Modal,
  Select,
  Segmented,
  Tooltip,
} from "antd";
import { KeyRound, Plus, RefreshCw, Star, Trash2 } from "lucide-react";

import { useStore } from "../lib/store";
import { maxTokensForModel, maxTokensHint } from "../lib/model-limits";
import { LANG_OPTIONS, type Lang } from "../lib/i18n";
import { useT } from "../lib/t";

/** 「关于」里摆出去的站点与源码仓库地址。 */
const PUBLISHER_SITE = "https://mutantcat.org";
const REPO_URL = "https://github.com/Mutantcat-Working-Group/AIPixel";
import type {
  Capabilities,
  ModelConfig,
  ModelRole,
  ModelView,
  Protocol,
} from "../lib/types";

const NO_CAPABILITIES: Capabilities = {
  vision: false,
  image_gen: false,
  video: false,
  reasoning: false,
};

interface FormShape {
  label: string;
  protocol: Protocol;
  base_url: string;
  api_key: string;
  model: string;
  max_tokens: number | null;
  temperature: number | null;
  capabilities: Capabilities;
}

/** 能另绑一个模型的三个角色，按工作流坞的条目顺序排。 */
const DETACHABLE_ROLES: {
  role: ModelRole;
  labelKey: "roles.image_gen" | "roles.vision" | "roles.video";
}[] = [
  { role: "image_gen", labelKey: "roles.image_gen" },
  { role: "vision", labelKey: "roles.vision" },
  { role: "video", labelKey: "roles.video" },
];

function toForm(model: ModelView | null): FormShape {
  if (!model) {
    return {
      label: "",
      protocol: "anthropic",
      base_url: "https://api.anthropic.com/v1",
      api_key: "",
      model: "claude-sonnet-4-5",
      max_tokens: 4096,
      temperature: null,
      capabilities: { ...NO_CAPABILITIES },
    };
  }
  return {
    label: model.label,
    protocol: model.protocol,
    base_url: model.base_url,
    api_key: "",
    model: model.model,
    // 老配置没存 max_tokens 时，先按模型名给个常用上限，别让字段空着。
    max_tokens: model.max_tokens ?? maxTokensForModel(model.model),
    temperature: model.temperature,
    capabilities: { ...model.capabilities },
  };
}

function ModelFragment({ title, hint }: { title: string; hint?: string }) {
  return (
    <div className="doc-section-title">
      <span>{title}</span>
      {hint ? <span className="grow" style={{ textTransform: "none" }}>{hint}</span> : null}
    </div>
  );
}

export default function ModelSettingsModal() {
  const t = useT();
  const lang = useStore((s) => s.lang);
  const setLang = useStore((s) => s.setLang);
  const open = useStore((s) => s.settingsOpen);
  const models = useStore((s) => s.models.entries);
  const activeId = useStore((s) => s.models.active_id);
  const closeSettings = useStore((s) => s.closeSettings);
  const upsertModel = useStore((s) => s.upsertModel);
  const removeModel = useStore((s) => s.removeModel);
  const activateModel = useStore((s) => s.activateModel);
  const fetchProviderModels = useStore((s) => s.fetchProviderModels);
  // 分工按会话走：读的是当前会话的分工快照，改的也是当前会话。
  const activeSessionId = useStore((s) => s.activeId);
  const sessions = useStore((s) => s.sessions);
  const bindSessionRole = useStore((s) => s.bindSessionRole);
  const clearSessionRole = useStore((s) => s.clearSessionRole);

  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [form] = Form.useForm<FormShape>();
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [fetching, setFetching] = useState(false);
  const [fetched, setFetched] = useState<string[]>([]);
  const [note, setNote] = useState<string | null>(null);

  const modelWatch = Form.useWatch<string>("model", form) ?? "";
  const maxWatch = Form.useWatch<number | null>("max_tokens", form) ?? null;
  const autoHint = maxTokensHint(modelWatch.trim());
  // 自动填过的记录：用户手动改过就不再抢，换模型才重新接手。
  const [autoMax, setAutoMax] = useState<{ model: string; value: number } | null>(null);

  const selected: ModelView | null = models.find((m) => m.id === selectedId) ?? null;
  const activeSession = sessions.find((s) => s.id === activeSessionId) ?? null;
  const roleBindings = activeSession?.roles ?? [];

  const protocolOptions: { label: string; value: Protocol }[] = [
    { label: t("settings.protocol.anthropic"), value: "anthropic" },
    { label: t("settings.protocol.openai"), value: "open_ai_compat" },
  ];

  const capabilityFields: { key: keyof Capabilities; label: string; hint: string }[] = [
    { key: "vision", label: t("settings.cap.vision"), hint: t("settings.cap.vision.hint") },
    { key: "image_gen", label: t("settings.cap.image_gen"), hint: t("settings.cap.image_gen.hint") },
    { key: "video", label: t("settings.cap.video"), hint: t("settings.cap.video.hint") },
    { key: "reasoning", label: t("settings.cap.reasoning"), hint: t("settings.cap.reasoning.hint") },
  ];

  // 只在开关时决定选中项；保存后不抢焦点，避免选中项跳回激活模型。
  useEffect(() => {
    if (!open) {
      setSelectedId(null);
      setError(null);
      return;
    }
    const current = useStore.getState().models;
    const fallback = current.entries.find((m) => m.id === current.active_id) ?? current.entries[0] ?? null;
    setSelectedId(fallback ? fallback.id : null);
  }, [open]);

  useEffect(() => {
    form.setFieldsValue(toForm(selected));
  }, [form, selected]);

  // 模型名一填好就把 Max tokens 顶到常用上限；手动敲过的字段不碰。
  useEffect(() => {
    if (!open) return;
    const name = modelWatch.trim();
    if (!name) return;
    const hint = maxTokensHint(name);
    if (!hint) return;
    // 手动改过的字段不抢：同一个模型下值 != 记下的自动值，就当用户自己在管。
    if (autoMax && autoMax.model === name && maxWatch !== autoMax.value) return;
    form.setFieldsValue({ max_tokens: hint });
    // 值没变就得把原对象交回去：每次渲染都造一个新对象，下游 useEffect 的依赖
    // 永远在变，自己会无限重跑下去。
    setAutoMax((prev) =>
      prev && prev.model === name && prev.value === hint ? prev : { model: name, value: hint },
    );
  }, [open, modelWatch, maxWatch, autoMax, form]);

  // 拉一份 provider 的模型清单。API key 留空时后端会用这个模型已存的密钥，所以不强制重填。
  async function fetchModels() {
    const values = form.getFieldsValue(true);
    const baseUrl = (values.base_url ?? "").trim();
    if (!baseUrl) {
      setFetched([]);
      setNote(t("settings.fetch_need_key"));
      return;
    }
    setFetching(true);
    setNote(null);
    try {
      const ids = await fetchProviderModels({
        id: selected?.id ?? null,
        baseUrl,
        apiKey: values.api_key ?? "",
        protocol: values.protocol ?? "anthropic",
      });
      setFetched(ids);
      setNote(
        ids.length > 0
          ? t("settings.model_count", { count: ids.length })
          : t("settings.fetch_empty"),
      );
    } catch (cause) {
      setFetched([]);
      setNote(t("settings.fetch_failed", { error: String(cause) }));
    } finally {
      setFetching(false);
    }
  }

  async function save() {
    const values = await form.validateFields();
    setSubmitting(true);
    setError(null);
    const id = selected?.id ?? `m${Date.now().toString(36)}`;
    const config: ModelConfig = {
      id,
      label: values.label.trim() || values.model,
      protocol: values.protocol,
      base_url: values.base_url.trim(),
      api_key: values.api_key,
      model: values.model.trim(),
      max_tokens: values.max_tokens ?? null,
      temperature: values.temperature ?? null,
      capabilities: values.capabilities ?? { ...NO_CAPABILITIES },
    };
    try {
      await upsertModel(config);
      setSelectedId(id);
      if (!useStore.getState().models.active_id) await activateModel(id);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Modal
      title={t("settings.title")}
      open={open}
      onCancel={closeSettings}
      width={720}
      className="model-modal"
      footer={[
        <Button key="close" onClick={closeSettings}>
          {t("settings.close")}
        </Button>,
        <Button key="save" type="primary" loading={submitting} onClick={save}>
          {t("settings.save")}
        </Button>,
      ]}
    >
      <div className="model-list">
        {models.length === 0 ? <div className="model-empty">{t("settings.empty")}</div> : null}
        {models.map((model) => (
          <div
            key={model.id}
            className={`model-row ${model.id === selectedId ? "active" : ""}`}
            onClick={() => setSelectedId(model.id)}
          >
            {model.id === activeId ? (
              <Tooltip title={t("settings.active")}>
                <Star size={12} />
              </Tooltip>
            ) : (
              <span className="row-dot" />
            )}
            <span className="name">{model.label}</span>
            {model.has_api_key ? null : <span className="no-key">{t("settings.no_key")}</span>}
          </div>
        ))}
        <Button
          block
          type="dashed"
          className="model-new"
          icon={<Plus size={13} />}
          onClick={() => setSelectedId(null)}
        >
          {t("settings.new")}
        </Button>
      </div>

      <div className="model-form">
        <div className="model-lang">
          <span className="model-lang-label">{t("settings.language")}</span>
          <Segmented
            size="small"
            value={lang}
            options={LANG_OPTIONS.map((option) => ({ label: option.label, value: option.value }))}
            onChange={(value) => setLang(value as Lang)}
          />
        </div>
        <Form<FormShape> form={form} layout="vertical" requiredMark={false} preserve={false}>
          <Form.Item
            name="label"
            label={t("settings.name")}
            rules={[{ required: true, message: t("settings.name_required") }]}
          >
            <Input placeholder={t("settings.name_placeholder")} spellCheck={false} />
          </Form.Item>

          <Form.Item name="protocol" label={t("settings.protocol")}>
            <Select options={protocolOptions} />
          </Form.Item>

          <Form.Item
            name="base_url"
            label={t("settings.base_url")}
            rules={[{ required: true, message: t("settings.base_url_required") }]}
          >
            <Input placeholder={t("settings.base_url_placeholder")} spellCheck={false} />
          </Form.Item>

          <Form.Item
            name="model"
            label={t("settings.model")}
            rules={[{ required: true, message: t("settings.model_required") }]}
          >
            <Input placeholder={t("settings.model_placeholder")} spellCheck={false} />
          </Form.Item>
          <Form.Item name="api_key" label={t("settings.api_key")}>
            <Input.Password
              placeholder={
                selected?.has_api_key
                  ? t("settings.api_key_keep")
                  : t("settings.api_key_placeholder")
              }
              autoComplete="new-password"
              spellCheck={false}
            />
          </Form.Item>

          <Form.Item
            name="model"
            label={t("settings.model")}
            rules={[{ required: true, message: t("settings.model_required") }]}
          >
            <Input placeholder={t("settings.model_placeholder")} spellCheck={false} />
          </Form.Item>
          <div className="model-fetch">
            <Button
              size="small"
              loading={fetching}
              icon={<RefreshCw size={13} />}
              onClick={() => void fetchModels()}
            >
              {fetching ? t("settings.fetching") : t("settings.fetch")}
            </Button>
            {fetched.length > 0 ? (
              <Select
                size="small"
                className="model-fetch-picker"
                value={undefined}
                options={fetched.map((id) => ({ label: id, value: id }))}
                placeholder={t("settings.pick_model")}
                onChange={(value: string) => {
                  form.setFieldsValue({ model: value });
                }}
              />
            ) : null}
            {note ? <span className="model-fetch-note">{note}</span> : null}
          </div>

          <ModelFragment title={t("settings.sampling")} />
          <Form.Item
            name="max_tokens"
            label={t("settings.max_tokens")}
            extra={
              autoHint && maxWatch === autoHint
                ? t("settings.max_tokens_auto", { model: modelWatch.trim() })
                : undefined
            }
          >
            <InputNumber min={256} max={200000} step={256} style={{ width: "100%" }} />
          </Form.Item>
          <Form.Item name="temperature" label={t("settings.temperature")}>
            <InputNumber min={0} max={2} step={0.1} style={{ width: "100%" }} />
          </Form.Item>
          <ModelFragment
            title={t("settings.capabilities")}
            hint={t("settings.capabilities_hint")}
          />
          <div className="cap-grid">
            {capabilityFields.map((field) => (
              <Tooltip key={field.key} title={field.hint}>
                <Form.Item name={["capabilities", field.key]} valuePropName="checked" noStyle>
                  <Checkbox>{field.label}</Checkbox>
                </Form.Item>
              </Tooltip>
            ))}
          </div>
        </Form>

        <ModelFragment title={t("roles.title")} hint={t("roles.hint")} />
        {activeSession ? (
          <div className="role-list">
            {DETACHABLE_ROLES.map(({ role, labelKey }) => {
              const binding = roleBindings.find((b) => b.role === role);
              const bound = binding?.detached ?? false;
              return (
                <div className="role-row" key={role}>
                  <span className="role-name">{t(labelKey)}</span>
                  <span className={`role-flag ${bound ? "" : "muted"}`}>
                    {bound ? t("roles.detached") : t("roles.following")}
                  </span>
                  <Select
                    size="small"
                    className="role-picker"
                    value={binding?.model_id}
                    placeholder={t("roles.pick")}
                    options={models.map((model) => ({ label: model.label, value: model.id }))}
                    onChange={(next) => void bindSessionRole(role, next)}
                  />
                  {bound ? (
                    <Button
                      size="small"
                      type="text"
                      onClick={() => void clearSessionRole(role)}
                    >
                      {t("roles.clear")}
                    </Button>
                  ) : null}
                </div>
              );
            })}
          </div>
        ) : (
          <div className="role-empty">{t("roles.session_needed")}</div>
        )}

        {error ? <Alert type="error" message={error} showIcon /> : null}

        <div className="model-form-actions">
          <span className="inline-note">
            <KeyRound size={11} />
            {selected?.has_api_key ? t("settings.key_stored") : t("settings.no_key_stored")}
          </span>
          <span className="grow" />
          {selected ? (
            <>
              {selected.id !== activeId ? (
                <Button size="small" onClick={() => void activateModel(selected.id)}>
                  {t("settings.set_active")}
                </Button>
              ) : null}
              <Button
                size="small"
                danger
                icon={<Trash2 size={13} />}
                onClick={() => {
                  void removeModel(selected.id);
                  setSelectedId(null);
                }}
              >
                {t("settings.delete")}
              </Button>
            </>
          ) : null}
        </div>

        <ModelFragment title={t("about.title")} />
        <div className="about-block">
          <p className="about-blurb">{t("about.blurb")}</p>
          <div className="about-row">
            <span className="about-key">{t("about.publisher")}</span>
            <span className="about-val">{t("about.publisher_value")}</span>
          </div>
          <div className="about-row">
            <span className="about-key">{t("about.site")}</span>
            <a className="about-val" href={PUBLISHER_SITE} target="_blank" rel="noreferrer">
              mutantcat.org
            </a>
          </div>
          <div className="about-row">
            <span className="about-key">{t("about.repo")}</span>
            <a className="about-val" href={REPO_URL} target="_blank" rel="noreferrer">
              {REPO_URL.replace("https://", "")}
            </a>
          </div>
          <div className="about-row">
            <span className="about-key">{t("about.version")}</span>
            <span className="about-val">{__APP_VERSION__}</span>
          </div>
        </div>
      </div>
    </Modal>
  );
}
