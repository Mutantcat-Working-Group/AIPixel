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
  Tooltip,
} from "antd";
import { KeyRound, Plus, Star, Trash2 } from "lucide-react";

import { useStore } from "../lib/store";
import type { Capabilities, ModelConfig, ModelView, Protocol } from "../lib/types";

const PROTOCOL_OPTIONS: { label: string; value: Protocol }[] = [
  { label: "Anthropic Messages", value: "anthropic" },
  { label: "OpenAI-compatible", value: "open_ai_compat" },
];

const NO_CAPABILITIES: Capabilities = { vision: false, image_gen: false, video: false };

const CAPABILITY_FIELDS: {
  key: keyof Capabilities;
  label: string;
  hint: string;
}[] = [
  {
    key: "vision",
    label: "Read images",
    hint: "Multimodal input: the model can look at a reference you attach.",
  },
  {
    key: "image_gen",
    label: "Generate images",
    hint: "The model can return an image bitmap instead of only text.",
  },
  {
    key: "video",
    label: "Read video",
    hint: "The model accepts video input, so frames can go straight to it.",
  },
];

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
    max_tokens: model.max_tokens,
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
  const open = useStore((s) => s.settingsOpen);
  const models = useStore((s) => s.models.entries);
  const activeId = useStore((s) => s.models.active_id);
  const closeSettings = useStore((s) => s.closeSettings);
  const upsertModel = useStore((s) => s.upsertModel);
  const removeModel = useStore((s) => s.removeModel);
  const activateModel = useStore((s) => s.activateModel);

  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [form] = Form.useForm<FormShape>();
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const selected: ModelView | null = models.find((m) => m.id === selectedId) ?? null;

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
      title="Models"
      open={open}
      onCancel={closeSettings}
      width={720}
      className="model-modal"
      footer={[
        <Button key="close" onClick={closeSettings}>
          Close
        </Button>,
        <Button key="save" type="primary" loading={submitting} onClick={save}>
          Save
        </Button>,
      ]}
    >
      <div className="model-list">
        {models.length === 0 ? <div className="model-empty">No models yet</div> : null}
        {models.map((model) => (
          <div
            key={model.id}
            className={`model-row ${model.id === selectedId ? "active" : ""}`}
            onClick={() => setSelectedId(model.id)}
          >
            {model.id === activeId ? (
              <Tooltip title="active model">
                <Star size={12} />
              </Tooltip>
            ) : (
              <span className="row-dot" />
            )}
            <span className="name">{model.label}</span>
            {model.has_api_key ? null : <span className="no-key">no key</span>}
          </div>
        ))}
        <Button
          block
          type="dashed"
          className="model-new"
          icon={<Plus size={13} />}
          onClick={() => setSelectedId(null)}
        >
          New
        </Button>
      </div>

      <div className="model-form">
        <Form<FormShape> form={form} layout="vertical" requiredMark={false} preserve={false}>
          <Form.Item
            name="label"
            label="Name"
            rules={[{ required: true, message: "Name is required" }]}
          >
            <Input placeholder="My Claude" spellCheck={false} />
          </Form.Item>

          <Form.Item name="protocol" label="Protocol">
            <Select options={PROTOCOL_OPTIONS} />
          </Form.Item>

          <Form.Item
            name="base_url"
            label="Base URL"
            rules={[{ required: true, message: "Base URL is required" }]}
          >
            <Input placeholder="https://api.anthropic.com/v1" spellCheck={false} />
          </Form.Item>

          <Form.Item
            name="model"
            label="Model"
            rules={[{ required: true, message: "Model is required" }]}
          >
            <Input placeholder="claude-sonnet-4-5" spellCheck={false} />
          </Form.Item>

          <Form.Item name="api_key" label="API key">
            <Input.Password
              placeholder={selected?.has_api_key ? "Leave blank to keep stored key" : "sk-..."}
              autoComplete="new-password"
              spellCheck={false}
            />
          </Form.Item>

          <ModelFragment title="Sampling" />
          <Form.Item name="max_tokens" label="Max tokens">
            <InputNumber min={256} max={64000} step={256} style={{ width: "100%" }} />
          </Form.Item>
          <Form.Item name="temperature" label="Temperature">
            <InputNumber min={0} max={2} step={0.1} style={{ width: "100%" }} />
          </Form.Item>
          <ModelFragment
            title="Capabilities"
            hint="declared by you, so the dock knows what this model can run"
          />
          <div className="cap-grid">
            {CAPABILITY_FIELDS.map((field) => (
              <Tooltip key={field.key} title={field.hint}>
                <Form.Item name={["capabilities", field.key]} valuePropName="checked" noStyle>
                  <Checkbox>{field.label}</Checkbox>
                </Form.Item>
              </Tooltip>
            ))}
          </div>
        </Form>

        {error ? <Alert type="error" message={error} showIcon /> : null}

        <div className="model-form-actions">
          <span className="inline-note">
            <KeyRound size={11} />
            {selected?.has_api_key ? "key stored locally" : "no key stored"}
          </span>
          <span className="grow" />
          {selected ? (
            <>
              {selected.id !== activeId ? (
                <Button size="small" onClick={() => void activateModel(selected.id)}>
                  Set active
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
                Delete
              </Button>
            </>
          ) : null}
        </div>
      </div>
    </Modal>
  );
}
