import { useEffect, useState, type ReactNode } from "react";
import {
  Alert,
  Button,
  Checkbox,
  Form,
  Input,
  InputNumber,
  Modal,
  Segmented,
  Select,
  Switch,
  Tooltip,
} from "antd";
import { KeyRound, Plus, RefreshCw, RotateCcw, ScanEye, Star, Trash2 } from "lucide-react";

import { useStore } from "../lib/store";
import { maxTokensForModel, maxTokensHint } from "../lib/model-limits";
import { DEFAULT_LOOP_LIMITS, type LoopLimits, type SettingsTab } from "../lib/types";
import { LANG_OPTIONS, type Lang } from "../lib/i18n";
import { useT, type T } from "../lib/t";
import type { TKey } from "../lib/i18n";

/** 「关于」里摆出去的站点与源码仓库地址。 */
const PUBLISHER_SITE = "https://mutantcat.org";
const REPO_URL = "https://github.com/Mutantcat-Working-Group/AIPixel";
import type {
  Capabilities,
  ImageSupport,
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
  /** 关掉思考的意愿。界面上只有两态：勾 = 每轮都关，不勾 = 交给程序判断。 */
  disable_thinking: boolean;
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

/** 三个页签的互斥顺序：模型 -> 护栏 -> 关于。 */
const TABS: { value: SettingsTab; label: TKey; hint: TKey }[] = [
  { value: "models", label: "settings.tab_models", hint: "settings.tab_models_hint" },
  { value: "guardrails", label: "settings.tab_guardrails", hint: "settings.tab_guardrails_hint" },
  { value: "about", label: "settings.tab_about", hint: "settings.tab_about_hint" },
];

function toForm(model: ModelView | null): FormShape {
  if (!model) {
    return {
      label: "",
      protocol: "anthropic",
      base_url: "https://api.anthropic.com/v1",
      api_key: "",
      model: "claude-sonnet-4-5",
      // 新模型一上来就给个像样的上限：填 4096 的话，一段分镜脚本
      // 连思考带正文写到一半就被掐断，用户只会以为模型笨。
      max_tokens: maxTokensForModel("claude-sonnet-4-5"),
      temperature: null,
      // 新模型默认「不干预」：有的模型关掉思考反而连工具都不会调，
      // 谁更好得试过才知道，所以先留 null，让 runner 那次自动翻盘兜底。
      disable_thinking: false,
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
    disable_thinking: model.disable_thinking ?? false,
    capabilities: { ...model.capabilities },
  };
}

/** 设置项分组。右栏所有条目都归到某一组里，组与组之间留缝，扫一眼就找得到。
 *  action 钉在标题右侧：放那种「看一眼结论」的按钮，不占内容区的行。 */
function SettingsSection({
  title,
  hint,
  action,
  children,
}: {
  title: string;
  hint?: string;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="settings-section">
      <div className="settings-section-title">
        <span>{title}</span>
        {hint ? <span className="grow section-hint">{hint}</span> : <span className="grow" />}
        {action}
      </div>
      {children}
    </section>
  );
}

/** 护栏的三个数字。Rust 会把越界的值夹回合理区间，回值才是真正生效的那份。 */
function LimitsSection() {
  const t = useT();
  const limits = useStore((s) => s.loopLimits);
  const saveLoopLimits = useStore((s) => s.saveLoopLimits);
  // 存一下闪一下：护栏是即改即存的，没这点反馈用户不知道自己碰上没有。
  const [justSaved, setJustSaved] = useState(false);

  useEffect(() => {
    if (!justSaved) return;
    const timer = setTimeout(() => setJustSaved(false), 1500);
    return () => clearTimeout(timer);
  }, [justSaved]);

  /** 改一个数就存一次：护栏的语义是「改完立刻生效」，不该等保存按钮。 */
  function patch(key: keyof LoopLimits, value: number) {
    void saveLoopLimits({ ...(limits ?? DEFAULT_LOOP_LIMITS), [key]: value });
    setJustSaved(true);
  }

  const fields: { key: keyof LoopLimits; label: string; hint: string; max: number }[] = [
    {
      key: "max_continuations",
      label: t("settings.limits.continuations"),
      hint: t("settings.limits.continuations_hint"),
      max: 50,
    },
    {
      key: "max_retries",
      label: t("settings.limits.retries"),
      hint: t("settings.limits.retries_hint"),
      max: 10,
    },
    {
      key: "max_reasoning_continuations",
      label: t("settings.limits.reasoning"),
      hint: t("settings.limits.reasoning_hint"),
      max: 10,
    },
  ];

  return (
    <SettingsSection title={t("settings.limits")}>
      <p className="settings-blurb">{t("settings.limits_hint")}</p>
      {fields.map((field) => (
        <div className="limit-row" key={field.key}>
          <div className="limit-text">
            <span className="limit-label">{field.label}</span>
            <span className="limit-hint">{field.hint}</span>
          </div>
          <InputNumber
            min={0}
            max={field.max}
            value={limits ? limits[field.key] : ""}
            disabled={!limits}
            style={{ width: 96 }}
            onChange={(value) => {
              // 清空 InputNumber 拿到的是空串，那是「一个都不要」，按 0 收。
              patch(field.key, typeof value === "number" ? value : 0);
            }}
          />
        </div>
      ))}
      <div className="limit-reset">
        <span className="grow inline-note">
          {justSaved ? t("settings.limits.saved") : null}
        </span>
        <Button
          size="small"
          icon={<RotateCcw size={12} />}
          onClick={() => {
            void saveLoopLimits(DEFAULT_LOOP_LIMITS);
            setJustSaved(true);
          }}
        >
          {t("settings.limits.reset")}
        </Button>
      </div>
    </SettingsSection>
  );
}

/** MCP 总闸：单个服务器去留在 MCP 面板里管，这里只说「用不用」。 */
function McpSection() {
  const t = useT();
  const enabled = useStore((s) => s.mcpEnabled);
  const setEnabled = useStore((s) => s.setMcpEnabled);
  return (
    <SettingsSection
      title={t("settings.mcp")}
      hint={enabled ? t("settings.mcp_on") : t("settings.mcp_off")}
      action={
        <Switch
          size="small"
          checked={enabled}
          onChange={(next) => void setEnabled(next)}
        />
      }
    >
      <p className="settings-blurb">{t("settings.mcp_hint")}</p>
    </SettingsSection>
  );
}

/** 界面说什么话。放「关于」里：跟着版本、发行者这些「这台机器长什么样」的信息一伙。 */
function LanguageSection() {
  const t = useT();
  const lang = useStore((s) => s.lang);
  const setLang = useStore((s) => s.setLang);
  return (
    <SettingsSection title={t("settings.language")}>
      <div className="settings-inline-row">
        <Segmented
          value={lang}
          options={LANG_OPTIONS.map((option) => ({ label: option.label, value: option.value }))}
          onChange={(value) => setLang(value as Lang)}
        />
      </div>
    </SettingsSection>
  );
}

function AboutSection() {
  const t = useT();
  return (
    <SettingsSection title={t("about.title")}>
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
          <span className="about-key">{t("about.license")}</span>
          <span className="about-val">{t("about.license_value")}</span>
        </div>
        <div className="about-row">
          <span className="about-key">{t("about.version")}</span>
          <span className="about-val">{__APP_VERSION__}</span>
        </div>
      </div>
    </SettingsSection>
  );
}

/** 探测结论翻译成人话。transport 是 Rust 记下的实际通路，顺手也译一下。 */
const TRANSPORT_LABELS: Record<string, TKey> = {
  images: "settings.probe.transport.images",
  images_edits: "settings.probe.transport.images_edits",
  chat_modalities: "settings.probe.transport.chat_modalities",
};

/** 探测结论翻译成人话。transport 是 Rust 记下的实际通路，顺手也译一下。 */
function describeProbe(t: T, result: ImageSupport): string {
  if (result.state === "yes") {
    const key = TRANSPORT_LABELS[result.transport];
    return t("settings.probe_yes", { transport: key ? t(key) : result.transport });
  }
  if (result.state === "no") {
    return t("settings.probe_no", { reason: result.reason });
  }
  return t("settings.probe_unknown", { reason: result.reason });
}

export default function ModelSettingsModal() {
  const t = useT();
  const open = useStore((s) => s.settingsOpen);
  const tab = useStore((s) => s.settingsTab);
  const setSettingsTab = useStore((s) => s.setSettingsTab);
  const models = useStore((s) => s.models.entries);
  const activeId = useStore((s) => s.models.active_id);
  const closeSettings = useStore((s) => s.closeSettings);
  const upsertModel = useStore((s) => s.upsertModel);
  const removeModel = useStore((s) => s.removeModel);
  const activateModel = useStore((s) => s.activateModel);
  const fetchProviderModels = useStore((s) => s.fetchProviderModels);
  const probeImage = useStore((s) => s.probeImage);
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
  // 探测结论只摆着看，不动表单：能力是用户声明的，探测只是份建议。
  const [probing, setProbing] = useState(false);
  const [probe, setProbe] = useState<{ ok: boolean; text: string } | null>(null);

  const modelWatch = Form.useWatch<string>("model", form) ?? "";
  const maxWatch = Form.useWatch<number | null>("max_tokens", form) ?? null;
  const capsWatch = Form.useWatch<Capabilities>("capabilities", form);
  const autoHint = maxTokensHint(modelWatch.trim());
  // 自动填过的记录：用户手动改过就不再抢，换模型才重新接手。
  const [autoMax, setAutoMax] = useState<{ model: string; value: number } | null>(null);

  const selected: ModelView | null = models.find((m) => m.id === selectedId) ?? null;
  const activeSession = sessions.find((s) => s.id === activeSessionId) ?? null;
  const roleBindings = activeSession?.roles ?? [];
  const activeTab = TABS.find((item) => item.value === tab) ?? TABS[0];

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
      setProbe(null);
      return;
    }
    const current = useStore.getState().models;
    const fallback = current.entries.find((m) => m.id === current.active_id) ?? current.entries[0] ?? null;
    setSelectedId(fallback ? fallback.id : null);
  }, [open]);

  useEffect(() => {
    form.setFieldsValue(toForm(selected));
  }, [form, selected]);

  /** 换模型时上一份探测结论就作废了：别拿着旧结论误导人。 */
  function selectModel(id: string | null) {
    setSelectedId(id);
    setProbe(null);
  }

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

  /** 探一次这个模型出不出图。只回话，不改任何勾选。 */
  async function probeCapability() {
    const values = form.getFieldsValue(true);
    const model = (values.model ?? "").trim();
    if (!model) {
      setProbe({ ok: false, text: t("settings.probe_need_model") });
      return;
    }
    setProbing(true);
    try {
      const result: ImageSupport = await probeImage({
        id: selected?.id ?? null,
        baseUrl: (values.base_url ?? "").trim(),
        apiKey: values.api_key ?? "",
        protocol: values.protocol ?? "anthropic",
        model,
      });
      setProbe({ ok: result.state === "yes", text: describeProbe(t, result) });
    } catch (cause) {
      setProbe({ ok: false, text: t("settings.probe_unknown", { reason: String(cause) }) });
    } finally {
      setProbing(false);
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
      // 勾上 = Some(true)（每轮都关）；不勾 = null（交给 runner 的自动翻盘）。
      // Some(false) 没有对应控件：那等于「明知它光想不动笔也要让它想」，
      // 想看思考过程的人留着「会思考」勾选就够了。
      disable_thinking: values.disable_thinking ? true : null,
      capabilities: values.capabilities ?? { ...NO_CAPABILITIES },
    };
    try {
      await upsertModel(config);
      selectModel(id);
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
      footer={
        // 只有模型页有要保存的表单；护栏和关于都是当场生效的，摆个关闭就够。
        tab === "models"
          ? [
              <Button key="close" onClick={closeSettings}>
                {t("settings.close")}
              </Button>,
              <Button key="save" type="primary" loading={submitting} onClick={save}>
                {t("settings.save")}
              </Button>,
            ]
          : [
              <Button key="close" type="primary" onClick={closeSettings}>
                {t("settings.close")}
              </Button>,
            ]
      }
    >
      <div className="settings-tabbar">
        <Segmented
          value={activeTab.value}
          options={TABS.map((item) => ({ label: t(item.label), value: item.value }))}
          onChange={(value) => setSettingsTab(value as SettingsTab)}
        />
        <span className="settings-tab-hint">{t(activeTab.hint)}</span>
      </div>

      {tab === "models" ? (
        <>
          <div className="model-list">
            {models.length === 0 ? <div className="model-empty">{t("settings.empty")}</div> : null}
            {models.map((model) => (
              <div
                key={model.id}
                className={`model-row ${model.id === selectedId ? "active" : ""}`}
                onClick={() => selectModel(model.id)}
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
              onClick={() => selectModel(null)}
            >
              {t("settings.new")}
            </Button>
          </div>

          <div className="model-pane">
            {/* 右栏整块滚：分组标题、分工、能力都在里面，动作条钉在下面。 */}
            <Form<FormShape>
              className="model-scroll"
              form={form}
              layout="vertical"
              requiredMark={false}
              preserve={false}
            >
              <SettingsSection title={t("settings.connection")} hint={t("settings.connection_hint")}>
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
                  name="model"
                  label={t("settings.model")}
                  rules={[{ required: true, message: t("settings.model_required") }]}
                >
                  <Input placeholder={t("settings.model_placeholder")} spellCheck={false} />
                </Form.Item>

                <Form.Item
                  name="base_url"
                  label={t("settings.base_url")}
                  rules={[{ required: true, message: t("settings.base_url_required") }]}
                >
                  <Input placeholder={t("settings.base_url_placeholder")} spellCheck={false} />
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
                        // 一次只填一个：选中即写进模型标识，下拉自己收起。
                        form.setFieldsValue({ model: value });
                      }}
                    />
                  ) : null}
                  {note ? <span className="model-fetch-note">{note}</span> : null}
                </div>
              </SettingsSection>

              <SettingsSection title={t("settings.sampling")} hint={t("settings.sampling_hint")}>
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
                <div className="settings-switch-row">
                  <Form.Item name="disable_thinking" valuePropName="checked" noStyle>
                    <Checkbox>{t("settings.disable_thinking")}</Checkbox>
                  </Form.Item>
                </div>
                <div className="settings-switch-hint">{t("settings.disable_thinking_hint")}</div>
              </SettingsSection>

              <SettingsSection
                title={t("settings.capabilities")}
                hint={t("settings.capabilities_hint")}
                action={
                  <Button
                    size="small"
                    loading={probing}
                    icon={<ScanEye size={13} />}
                    onClick={() => void probeCapability()}
                  >
                    {probing ? t("settings.probe_running") : t("settings.probe")}
                  </Button>
                }
              >
                <div className="cap-grid">
                  {capabilityFields.map((field) => (
                    <Tooltip key={field.key} title={field.hint}>
                      <Form.Item name={["capabilities", field.key]} valuePropName="checked" noStyle>
                        <Checkbox>{field.label}</Checkbox>
                      </Form.Item>
                    </Tooltip>
                  ))}
                </div>
                {probe ? (
                  <div className={`probe-note ${probe.ok ? "ok" : ""}`}>
                    <span className="grow">{probe.text}</span>
                    {probe.ok && !capsWatch?.image_gen ? (
                      <Button
                        size="small"
                        type="link"
                        onClick={() => {
                          // 探测说行、用户也认，就替他把「生图」勾上；反悔自己取消。
                          form.setFieldsValue({
                            capabilities: { ...(capsWatch ?? NO_CAPABILITIES), image_gen: true },
                          });
                        }}
                      >
                        {t("settings.probe_apply")}
                      </Button>
                    ) : null}
                  </div>
                ) : null}
              </SettingsSection>

              <SettingsSection title={t("roles.title")} hint={t("roles.hint")}>
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
              </SettingsSection>

              {error ? <Alert type="error" message={error} showIcon /> : null}
            </Form>

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
                      selectModel(null);
                    }}
                  >
                    {t("settings.delete")}
                  </Button>
                </>
              ) : null}
            </div>
          </div>
        </>
      ) : (
        <div className="settings-wide">
          {tab === "guardrails" ? (
            <>
              <LimitsSection />
              <McpSection />
            </>
          ) : (
            <>
              <LanguageSection />
              <AboutSection />
            </>
          )}
        </div>
      )}
    </Modal>
  );
}
