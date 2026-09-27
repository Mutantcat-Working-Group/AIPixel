// 工作流坞：目录里的六条 + 一条纯本机的量化。
// 目录跟着会话绑的模型算 readiness；被能力挡住的条目照样列出来，只是禁用并写清缺什么，
// 不然用户只看到灰按钮，不知道为什么。
// quantize 不在能力目录里（它不需要模型），但确实是常用的一条，所以单独挂在末尾。

import { useMemo, type ReactNode } from "react";
import { Alert, Button, Input, InputNumber, Segmented, Select, Slider, Switch, Tooltip } from "antd";
import {
  ArrowRight,
  Film,
  Frame as FrameIcon,
  FolderOpen,
  Grid2x2,
  Image,
  MessageSquare,
  Play,
  ScanEye,
  Sliders,
  Sparkles,
  X,
} from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";

import * as bridge from "../lib/bridge";
import { briefToText, probeSummary } from "../lib/dock-format";
import { useStore } from "../lib/store";
import type {
  DockKind,
  FitMode,
  LandSpot,
  MigrateOrder,
  PixelizeOptions,
  RefineTarget,
  TweenMode,
  VisionBrief,
  WorkflowEntry,
} from "../lib/types";

const KIND_ICONS: Record<DockKind, ReactNode> = {
  agent: <MessageSquare size={13} />,
  image_gen: <Image size={13} />,
  vision_brief: <ScanEye size={13} />,
  video_frames: <Film size={13} />,
  frame_tween: <FrameIcon size={13} />,
  prompt_refine: <Sparkles size={13} />,
  quantize: <Grid2x2 size={13} />,
};

const FIT_OPTIONS = [
  { label: "Contain", value: "contain" },
  { label: "Stretch", value: "stretch" },
];

const TWEEN_MODE_OPTIONS = [
  { label: "Migrate", value: "migrate" },
  { label: "Blend", value: "blend" },
  { label: "Copy", value: "copy" },
];

const TWEEN_ORDER_OPTIONS = [
  { label: "Scan", value: "scan" },
  { label: "Radial", value: "radial" },
  { label: "Scatter", value: "scatter" },
];

const SPOT_OPTIONS = [
  { label: "Active cel", value: "active_cel" },
  { label: "New frame", value: "new_frame" },
];

const REFINE_TARGET_OPTIONS = [
  { label: "Image gen", value: "image_gen" },
  { label: "Lua shader", value: "shader" },
];

const SIZE_OPTIONS = ["512x512", "768x768", "1024x1024", "1024x576", "576x1024"];

const IMAGE_EXTENSIONS = ["png", "jpg", "jpeg", "webp", "gif"];
const VIDEO_EXTENSIONS = ["mp4", "mov", "webm", "mkv", "avi", "m4v"];

interface DockRow {
  key: DockKind;
  title: string;
  summary: string;
  /** 跑完会得到什么，给 tooltip 用。 */
  output: string;
  readiness: { state: "ready" } | { state: "blocked"; missing: string[] };
}

const QUANTIZE_ROW: DockRow = {
  key: "quantize",
  title: "Quantize",
  summary: "Drop a bitmap onto the grid. Runs on this machine, no model needed.",
  output: "One cel of quantized pixels",
  readiness: { state: "ready" },
};

function baseName(path: string): string {
  const parts = path.split(/[/\\]/);
  return parts[parts.length - 1] || path;
}

async function pickFile(extensions: string[], name: string): Promise<string | null> {
  const picked = await open({ multiple: false, filters: [{ name, extensions }] });
  return typeof picked === "string" ? picked : null;
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="dock-field">
      <span className="dock-field-label">{label}</span>
      {children}
    </div>
  );
}

function PathField({
  label,
  buttonLabel,
  extensions,
  value,
  onPick,
  onClear,
}: {
  label: string;
  buttonLabel: string;
  extensions: string[];
  value: string | null;
  onPick: (path: string) => void;
  onClear: () => void;
}) {
  if (value) {
    return (
      <Field label={label}>
        <div className="dock-path">
          <span className="grow" title={value}>
            {baseName(value)}
          </span>
          <Button size="small" type="text" icon={<X size={12} />} onClick={onClear} />
        </div>
      </Field>
    );
  }
  return (
    <Field label={label}>
      <Button
        block
        size="small"
        icon={<FolderOpen size={13} />}
        onClick={() => {
          void pickFile(extensions, buttonLabel).then((picked) => {
            if (picked) onPick(picked);
          });
        }}
      >
        {buttonLabel}
      </Button>
    </Field>
  );
}

function QuantizeFields({
  value,
  onChange,
}: {
  value: PixelizeOptions;
  onChange: (next: PixelizeOptions) => void;
}) {
  return (
    <details className="dock-advanced">
      <summary>
        <Sliders size={12} />
        Quantize
      </summary>
      <Field label={`Max colors ${value.max_colors}`}>
        <Slider
          min={2}
          max={64}
          value={value.max_colors}
          onChange={(next) => onChange({ ...value, max_colors: next })}
        />
      </Field>
      <Field label={`Alpha cutoff ${value.alpha_threshold}`}>
        <Slider
          min={0}
          max={255}
          value={value.alpha_threshold}
          onChange={(next) => onChange({ ...value, alpha_threshold: next })}
        />
      </Field>
      <div className="dock-flag">
        <Switch
          size="small"
          checked={value.dither}
          onChange={(next) => onChange({ ...value, dither: next })}
        />
        <span>Dither</span>
      </div>
      <div className="dock-flag">
        <Switch
          size="small"
          checked={value.expand_palette}
          onChange={(next) => onChange({ ...value, expand_palette: next })}
        />
        <span>Expand palette</span>
      </div>
      <Field label="Fit">
        <Segmented
          size="small"
          block
          value={value.fit}
          options={FIT_OPTIONS}
          onChange={(next) => onChange({ ...value, fit: next as FitMode })}
        />
      </Field>
    </details>
  );
}

function BriefView({ brief }: { brief: VisionBrief }) {
  const rows: [string, string][] = [
    ["Subject", brief.subject],
    ["Silhouette", brief.silhouette],
    ["Pose", brief.pose_notes],
    ["Proportions", brief.proportions],
    ["Craft", brief.craft_notes],
  ];
  return (
    <div className="dock-brief">
      {rows.map(([label, text]) =>
        text.trim() === "" ? null : (
          <div className="dock-brief-row" key={label}>
            <span className="dock-brief-label">{label}</span>
            <span>{text}</span>
          </div>
        ),
      )}
      {brief.palette.length > 0 ? (
        <div className="dock-brief-row">
          <span className="dock-brief-label">Palette</span>
          <span className="brief-swatches">
            {brief.palette.map((hex, index) => (
              <span
                key={`${hex}-${index}`}
                className="brief-swatch"
                style={{ background: hex }}
                title={hex}
              />
            ))}
          </span>
        </div>
      ) : null}
    </div>
  );
}
// ---------- 坞 ----------

/** 坞的主体：条目列表 + 当前条目的表单 + 最近一次回执。 */
export default function WorkflowDock() {
  const workflows = useStore((s) => s.workflows);
  const catalogReady = useStore((s) => s.catalogReady);
  const kind = useStore((s) => s.kind);
  const setKind = useStore((s) => s.setKind);
  const busy = useStore((s) => s.workflowBusy);
  const outcome = useStore((s) => s.outcome);
  const outcomeError = useStore((s) => s.outcomeError);
  const document = useStore((s) => s.document);
  const openSettings = useStore((s) => s.openSettings);

  const rows = useMemo<DockRow[]>(
    () => [
      ...workflows.map((entry: WorkflowEntry) => ({
        key: entry.kind,
        title: entry.title,
        summary: entry.summary,
        output: entry.output,
        readiness: entry.readiness,
      })),
      QUANTIZE_ROW,
    ],
    [workflows],
  );

  const active = rows.find((row) => row.key === kind) ?? QUANTIZE_ROW;
  const missing = active.readiness.state === "blocked" ? active.readiness.missing : null;
  // quantize 纯本机，目录没取回来也跑得动；其余六条要等 readiness 说话。
  const pending = kind !== "quantize" && !catalogReady;
  const gated = busy || missing !== null || pending;
  const frameCount = document?.frames.length ?? 0;

  return (
    <section className="panel dock">
      <div className="panel-head">
        <Play size={13} />
        <strong>Workflows</strong>
        <span className="grow" />
        {pending ? <span className="dock-pending">readiness</span> : null}
      </div>

      <div className="kind-list">
        {rows.map((row) => {
          const flag =
            row.readiness.state === "blocked" ? row.readiness.missing.join(", ") : "";
          const hint = flag === "" ? row.output : `${row.summary} Missing: ${flag}.`;
          return (
            <Tooltip key={row.key} title={hint} placement="right">
              <button
                type="button"
                className={`kind-row ${kind === row.key ? "active" : ""} ${flag === "" ? "" : "blocked"}`}
                onClick={() => setKind(row.key)}
              >
                <span className="kind-icon">{KIND_ICONS[row.key]}</span>
                <span className="kind-title">{row.title}</span>
                {flag === "" ? null : <span className="kind-flag">needs {flag}</span>}
              </button>
            </Tooltip>
          );
        })}
      </div>

      <div className="panel-body dock-body">
        <div className="dock-panel">
          <div className="dock-heading">
            <span className="kind-icon">{KIND_ICONS[active.key]}</span>
            <strong>{active.title}</strong>
          </div>
          <p className="dock-note">{active.summary}</p>

          {pending ? (
            <Alert
              type="info"
              showIcon
              className="dock-alert"
              message="Reading what the bound model can do..."
            />
          ) : null}

          {missing ? (
            <Alert
              type="warning"
              showIcon
              className="dock-alert"
              message={`This model cannot ${active.title.toLowerCase()}`}
              description={
                <>
                  It is missing {missing.join(" and ")}. Switch the model, or turn the
                  capability on in settings.
                  <Button size="small" type="link" onClick={openSettings}>
                    Model settings
                  </Button>
                </>
              }
            />
          ) : null}

          {kind === "agent" ? <AgentPanel /> : null}
          {kind === "prompt_refine" ? <RefinePanel gated={gated} /> : null}
          {kind === "image_gen" ? <ImageGenPanel gated={gated} /> : null}
          {kind === "vision_brief" ? <VisionPanel gated={gated} /> : null}
          {kind === "video_frames" ? <VideoPanel gated={gated} /> : null}
          {kind === "frame_tween" ? (
            <TweenPanel gated={gated} frameCount={frameCount} />
          ) : null}
          {kind === "quantize" ? <QuantizePanel gated={busy} /> : null}
        </div>

        {outcome ? (
          <div className="dock-result">
            <div className="dock-result-summary">{outcome.summary}</div>
            {outcome.detail ? (
              <details className="dock-advanced">
                <summary>Detail</summary>
                <pre>{JSON.stringify(outcome.detail, null, 2)}</pre>
              </details>
            ) : null}
          </div>
        ) : null}

        {outcomeError ? (
          <Alert type="error" showIcon className="dock-alert" message={outcomeError} />
        ) : null}
      </div>
    </section>
  );
}

/** 聊天那条没有 Run 按钮：它本身就是聊天面板，这里只负责把用户引过去。 */
function AgentPanel() {
  const requestCompose = useStore((s) => s.requestCompose);

  return (
    <>
      <p className="dock-note">
        Ask in the chat panel. The agent reads the canvas first, then writes one sandboxed Lua
        script per edit, so every change stays inside the document you see here.
      </p>
      <Button
        block
        size="small"
        icon={<MessageSquare size={13} />}
        onClick={() =>
          requestCompose(
            "Read the canvas first, then draw this as a pixel edit. Keep the palette tight and match the existing frames.",
          )
        }
      >
        Put an ask in the composer
      </Button>
    </>
  );
}

function RefinePanel({ gated }: { gated: boolean }) {
  const idea = useStore((s) => s.dockDraft.idea);
  const target = useStore((s) => s.refineTarget);
  const refined = useStore((s) => s.refined);
  const prompt = useStore((s) => s.refinedDraft);
  const patchDraft = useStore((s) => s.patchDraft);
  const setRefineTarget = useStore((s) => s.setRefineTarget);
  const setRefinedPrompt = useStore((s) => s.setRefinedPrompt);
  const refinePrompt = useStore((s) => s.refinePrompt);
  const usePromptInGen = useStore((s) => s.usePromptInGen);
  const requestCompose = useStore((s) => s.requestCompose);

  return (
    <>
      <Field label="Idea">
        <Input.TextArea
          value={idea}
          size="small"
          autoSize={{ minRows: 2, maxRows: 6 }}
          placeholder="a fox blacksmith hammering at a forge"
          onChange={(event) => patchDraft({ idea: event.target.value })}
        />
      </Field>
      <Field label="Target">
        <Segmented
          size="small"
          block
          value={target}
          options={REFINE_TARGET_OPTIONS}
          onChange={(next) => setRefineTarget(next as RefineTarget)}
        />
      </Field>
      <Button
        block
        size="small"
        type="primary"
        icon={<Sparkles size={13} />}
        disabled={gated || idea.trim() === ""}
        onClick={() => void refinePrompt(idea)}
      >
        Refine
      </Button>

      {refined ? (
        <>
          <Field label="Refined prompt">
            <Input.TextArea
              value={prompt}
              size="small"
              autoSize={{ minRows: 4, maxRows: 12 }}
              onChange={(event) => setRefinedPrompt(event.target.value)}
            />
          </Field>
          <div className="dock-actions">
            <Button
              size="small"
              icon={<ArrowRight size={13} />}
              onClick={() => usePromptInGen(prompt)}
            >
              Use in image gen
            </Button>
            <Button
              size="small"
              icon={<MessageSquare size={13} />}
              onClick={() => requestCompose(prompt)}
            >
              Send to chat
            </Button>
          </div>
          {refined.raw.trim() !== refined.prompt.trim() ? (
            <details className="dock-advanced">
              <summary>The model said more</summary>
              <pre>{refined.raw}</pre>
            </details>
          ) : null}
        </>
      ) : null}
    </>
  );
}

function ImageGenPanel({ gated }: { gated: boolean }) {
  const draft = useStore((s) => s.dockDraft);
  const busy = useStore((s) => s.workflowBusy);
  const patchDraft = useStore((s) => s.patchDraft);
  const runWorkflow = useStore((s) => s.runWorkflow);

  return (
    <>
      <Field label="Prompt">
        <Input.TextArea
          value={draft.prompt}
          size="small"
          autoSize={{ minRows: 3, maxRows: 10 }}
          placeholder="16x16 sprite, hard edges, four colors, no antialiasing"
          onChange={(event) => patchDraft({ prompt: event.target.value })}
        />
      </Field>
      <Field label="Size">
        <Select
          size="small"
          value={draft.size}
          options={SIZE_OPTIONS.map((size) => ({ label: size, value: size }))}
          onChange={(next) => patchDraft({ size: next })}
        />
      </Field>
      <PathField
        label="Reference"
        buttonLabel="Pick a reference image"
        extensions={IMAGE_EXTENSIONS}
        value={draft.genPath}
        onPick={(path) => patchDraft({ genPath: path })}
        onClear={() => patchDraft({ genPath: null })}
      />
      <Field label="Landing">
        <Segmented
          size="small"
          block
          value={draft.spot}
          options={SPOT_OPTIONS}
          onChange={(next) => patchDraft({ spot: next as LandSpot })}
        />
      </Field>
      <Field label="Frame duration">
        <InputNumber
          size="small"
          style={{ width: "100%" }}
          min={16}
          max={2000}
          value={draft.durationMs}
          addonAfter="ms"
          onChange={(next) => patchDraft({ durationMs: next ?? 83 })}
        />
      </Field>
      <QuantizeFields
        value={draft.options}
        onChange={(options) => patchDraft({ options })}
      />
      <Button
        block
        size="small"
        type="primary"
        icon={<Play size={12} />}
        loading={busy}
        disabled={gated || draft.prompt.trim() === ""}
        onClick={() =>
          void runWorkflow("image_gen", {
            prompt: draft.prompt,
            size: draft.size,
            reference_path: draft.genPath,
            spot: draft.spot,
            duration_ms: draft.durationMs,
            options: draft.options,
          })
        }
      >
        Generate
      </Button>
    </>
  );
}

function VisionPanel({ gated }: { gated: boolean }) {
  const path = useStore((s) => s.dockDraft.visionPath);
  const vision = useStore((s) => s.vision);
  const patchDraft = useStore((s) => s.patchDraft);
  const briefReference = useStore((s) => s.briefReference);
  const usePromptInGen = useStore((s) => s.usePromptInGen);
  const requestCompose = useStore((s) => s.requestCompose);

  return (
    <>
      <PathField
        label="Reference"
        buttonLabel="Pick a reference image"
        extensions={IMAGE_EXTENSIONS}
        value={path}
        onPick={(picked) => {
          patchDraft({ visionPath: picked });
          void briefReference(picked);
        }}
        onClear={() => patchDraft({ visionPath: null })}
      />
      {vision ? <BriefView brief={vision} /> : null}
      <div className="dock-actions">
        <Button
          size="small"
          icon={<ArrowRight size={13} />}
          disabled={gated || vision === null}
          onClick={() => {
            if (vision) usePromptInGen(briefToText(vision));
          }}
        >
          Draw from this
        </Button>
        <Button
          size="small"
          icon={<MessageSquare size={13} />}
          disabled={vision === null}
          onClick={() => {
            if (vision) requestCompose(briefToText(vision));
          }}
        >
          Send to chat
        </Button>
      </div>
    </>
  );
}

function VideoPanel({ gated }: { gated: boolean }) {
  const draft = useStore((s) => s.dockDraft);
  const probe = useStore((s) => s.probe);
  const busy = useStore((s) => s.workflowBusy);
  const patchDraft = useStore((s) => s.patchDraft);
  const probeVideo = useStore((s) => s.probeVideo);
  const runWorkflow = useStore((s) => s.runWorkflow);

  return (
    <>
      <PathField
        label="Clip"
        buttonLabel="Pick a video"
        extensions={VIDEO_EXTENSIONS}
        value={draft.videoPath}
        onPick={(picked) => {
          patchDraft({ videoPath: picked });
          void probeVideo(picked);
        }}
        onClear={() => patchDraft({ videoPath: null })}
      />
      {draft.videoPath && probe ? (
        <p className="dock-probe">
          {baseName(draft.videoPath)} - {probeSummary(probe.probe, probe.source)}
        </p>
      ) : null}
      <Field label="Frames to pull">
        <InputNumber
          size="small"
          style={{ width: "100%" }}
          min={0}
          max={256}
          value={draft.videoCount}
          addonAfter={draft.videoCount === 0 ? "= all" : "frame(s)"}
          onChange={(next) => patchDraft({ videoCount: next ?? 0 })}
        />
      </Field>
      <QuantizeFields
        value={draft.options}
        onChange={(options) => patchDraft({ options })}
      />
      <Button
        block
        size="small"
        type="primary"
        icon={<Play size={12} />}
        loading={busy}
        disabled={gated || draft.videoPath === null}
        onClick={() =>
          void runWorkflow("video_frames", {
            path: draft.videoPath ?? "",
            count: draft.videoCount,
            duration_ms: draft.durationMs,
            options: draft.options,
          })
        }
      >
        Pull frames
      </Button>
    </>
  );
}

function TweenPanel({ gated, frameCount }: { gated: boolean; frameCount: number }) {
  const document = useStore((s) => s.document);
  const draft = useStore((s) => s.dockDraft);
  const busy = useStore((s) => s.workflowBusy);
  const patchDraft = useStore((s) => s.patchDraft);
  const runWorkflow = useStore((s) => s.runWorkflow);

  // 帧 id 属于文档，换文档后旧选择会失效：直接推导合法值，不做同步 effect。
  const frameIds = (document?.frames ?? []).map((frame) => frame.id);
  const from = frameIds.includes(draft.tweenFrom)
    ? draft.tweenFrom
    : frameIds[frameIds.length - 2] ?? "";
  const to = frameIds.includes(draft.tweenTo)
    ? draft.tweenTo
    : frameIds[frameIds.length - 1] ?? "";
  const sameEnd = from === "" || from === to;

  if (frameCount < 2) {
    return (
      <p className="dock-note">
        Draw at least two frames first. The end frames are the ground truth; this only fills
        what is in between.
      </p>
    );
  }

  return (
    <>
      <Field label="From">
        <Segmented
          size="small"
          block
          value={from}
          options={frameIds.map((id) => ({ label: id, value: id }))}
          onChange={(next) => patchDraft({ tweenFrom: next })}
        />
      </Field>
      <Field label="To">
        <Segmented
          size="small"
          block
          value={to}
          options={frameIds.map((id) => ({ label: id, value: id }))}
          onChange={(next) => patchDraft({ tweenTo: next })}
        />
      </Field>
      <Field label={`Frames in between: ${draft.tweenCount}`}>
        <Slider
          min={1}
          max={32}
          value={draft.tweenCount}
          onChange={(next) => patchDraft({ tweenCount: next })}
        />
      </Field>
      <Field label="Mode">
        <Segmented
          size="small"
          block
          value={draft.tweenMode}
          options={TWEEN_MODE_OPTIONS}
          onChange={(next) => patchDraft({ tweenMode: next as TweenMode })}
        />
      </Field>
      <Field label="Order">
        <Segmented
          size="small"
          block
          value={draft.tweenOrder}
          options={TWEEN_ORDER_OPTIONS}
          disabled={draft.tweenMode !== "migrate"}
          onChange={(next) => patchDraft({ tweenOrder: next as MigrateOrder })}
        />
      </Field>
      <div className="dock-flag">
        <Switch
          size="small"
          checked={draft.tweenEase}
          onChange={(next) => patchDraft({ tweenEase: next })}
        />
        <span>Ease in and out</span>
      </div>
      <Button
        block
        size="small"
        type="primary"
        icon={<Play size={12} />}
        loading={busy}
        disabled={gated || sameEnd}
        onClick={() =>
          void runWorkflow("frame_tween", {
            from_frame: from,
            to_frame: to,
            count: draft.tweenCount,
            mode: draft.tweenMode,
            order: draft.tweenOrder,
            ease: draft.tweenEase,
            duration_ms: draft.durationMs,
          })
        }
      >
        Insert
      </Button>
    </>
  );
}

function QuantizePanel({ gated }: { gated: boolean }) {
  const path = useStore((s) => s.dockDraft.quantizePath);
  const options = useStore((s) => s.dockDraft.options);
  const patchDraft = useStore((s) => s.patchDraft);
  const runPixelize = useStore((s) => s.runPixelize);
  const surfaceError = useStore((s) => s.surfaceError);

  async function quantize() {
    if (!path) return;
    try {
      // 位图要过一遍 read_image_context：base64 + media_type 都由 Rust 判，前端不猜。
      const attachment = await bridge.readImageContext(path);
      await runPixelize({
        image_base64: attachment.data_base64,
        media_type: attachment.media_type,
        options,
      });
    } catch (error) {
      surfaceError(error instanceof Error ? error.message : String(error));
    }
  }

  return (
    <>
      <PathField
        label="Source"
        buttonLabel="Pick an image"
        extensions={IMAGE_EXTENSIONS}
        value={path}
        onPick={(picked) => patchDraft({ quantizePath: picked })}
        onClear={() => patchDraft({ quantizePath: null })}
      />
      <QuantizeFields value={options} onChange={(next) => patchDraft({ options: next })} />
      <Button
        block
        size="small"
        type="primary"
        icon={<Play size={12} />}
        loading={gated}
        disabled={gated || path === null}
        onClick={() => void quantize()}
      >
        Quantize
      </Button>
    </>
  );
}
