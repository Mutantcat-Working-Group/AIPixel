// 工作流坞：目录里的七条 + 一条纯本机的量化。
// 目录跟着会话绑的模型算 readiness；被能力挡住的条目照样列出来，只是禁用并写清缺什么，
// 不然用户只看到灰按钮，不知道为什么。
// quantize 不在能力目录里（它不需要模型），但确实是常用的一条，所以单独挂在末尾。
// 抽帧同理：它和补帧一样全程在本机，没有读视频模型也该跑得动。

import { useMemo, type ReactNode } from "react";
import { Alert, Button, Input, InputNumber, Segmented, Select, Slider, Switch, Tooltip } from "antd";
import {
  ArrowRight,
  Clapperboard,
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
import { briefToText, probeSummary, videoBriefToText } from "../lib/dock-format";
import { renderUiText, translate, translateText, type Lang } from "../lib/i18n";
import { useStore } from "../lib/store";
import { useT, type T } from "../lib/t";
import type {
  DockKind,
  DockDraft,
  FitMode,
  LandSpot,
  MigrateOrder,
  PixelizeOptions,
  RefineTarget,
  TweenMode,
  VisionBrief,
  VideoBrief,
  WorkflowEntry,
} from "../lib/types";

const KIND_ICONS: Record<DockKind, ReactNode> = {
  agent: <MessageSquare size={13} />,
  image_gen: <Image size={13} />,
  vision_brief: <ScanEye size={13} />,
  video_frames: <Film size={13} />,
  video_brief: <Clapperboard size={13} />,
  frame_tween: <FrameIcon size={13} />,
  prompt_refine: <Sparkles size={13} />,
  quantize: <Grid2x2 size={13} />,
};

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

/** 缺的能力名 -> 界面词。Rust 只给 snake_case 标识，中文要说人话。 */
const CAP_LABEL_KEY = {
  vision: "cap.vision",
  image_gen: "cap.image_gen",
  video: "cap.video",
} as const;

function missingLabel(name: string, t: T): string {
  const key = CAP_LABEL_KEY[name as keyof typeof CAP_LABEL_KEY];
  return key ? t(key) : name;
}

/** 并列能力名的连接词：中文用顿号，英文用 and。 */
function listSep(lang: Lang): string {
  return lang === "zh" ? "、" : " and ";
}

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
  const t = useT();
  return (
    <details className="dock-advanced">
      <summary>
        <Sliders size={12} />
        {t("dock.advanced")}
      </summary>
      <Field label={t("dock.max_colors", { count: value.max_colors })}>
        <Slider
          min={2}
          max={64}
          value={value.max_colors}
          onChange={(next) => onChange({ ...value, max_colors: next })}
        />
      </Field>
      <Field label={t("dock.alpha_cutoff", { count: value.alpha_threshold })}>
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
        <span>{t("dock.dither")}</span>
      </div>
      <div className="dock-flag">
        <Switch
          size="small"
          checked={value.expand_palette}
          onChange={(next) => onChange({ ...value, expand_palette: next })}
        />
        <span>{t("dock.expand_palette")}</span>
      </div>
      <Field label={t("dock.fit")}>
        <Segmented
          size="small"
          block
          value={value.fit}
          options={[
            { label: t("fit.contain"), value: "contain" },
            { label: t("fit.stretch"), value: "stretch" },
          ]}
          onChange={(next) => onChange({ ...value, fit: next as FitMode })}
        />
      </Field>
    </details>
  );
}

/** 两种简报共用的骨架：空字段整行不画，配色永远画色块。 */
function BriefRows({ rows, palette }: { rows: [string, string][]; palette: string[] }) {
  const t = useT();
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
      {palette.length > 0 ? (
        <div className="dock-brief-row">
          <span className="dock-brief-label">{t("brief.palette")}</span>
          <span className="brief-swatches">
            {palette.map((hex, index) => (
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

function BriefView({ brief }: { brief: VisionBrief }) {
  const t = useT();
  return (
    <BriefRows
      rows={[
        [t("brief.subject"), brief.subject],
        [t("brief.silhouette"), brief.silhouette],
        [t("brief.pose"), brief.pose_notes],
        [t("brief.proportions"), brief.proportions],
        [t("brief.craft"), brief.craft_notes],
      ]}
      palette={brief.palette}
    />
  );
}

/** 运动简报没有剪影和比例可言，行换成运动和节奏。 */
function MotionBriefView({ brief }: { brief: VideoBrief }) {
  const t = useT();
  return (
    <BriefRows
      rows={[
        [t("brief.subject"), brief.subject],
        [t("brief.motion"), brief.motion],
        [t("brief.key_poses"), brief.key_poses.join(" | ")],
        [t("brief.timing"), brief.timing],
        [t("brief.craft"), brief.craft_notes],
      ]}
      palette={brief.palette}
    />
  );
}
// ---------- 坞 ----------

/** 坞的主体：条目列表 + 当前条目的表单 + 最近一次回执。 */
export default function WorkflowDock() {
  const t = useT();
  const workflows = useStore((s) => s.workflows);
  const catalogReady = useStore((s) => s.catalogReady);
  const kind = useStore((s) => s.kind);
  const setKind = useStore((s) => s.setKind);
  const busy = useStore((s) => s.workflowBusy);
  const outcome = useStore((s) => s.outcome);
  const outcomeError = useStore((s) => s.outcomeError);
  const document = useStore((s) => s.document);
  const openSettings = useStore((s) => s.openSettings);
  const lang = useStore((s) => s.lang);

  // 目录文案以字典为准，Rust 的英文原句只当缺键时的后备；quantize 不在目录里，单独补。
  const rows = useMemo<DockRow[]>(
    () => [
      ...workflows.map((entry: WorkflowEntry) => ({
        key: entry.kind,
        title: translateText(lang, `wf.${entry.kind}.title`, undefined, entry.title),
        summary: translateText(lang, `wf.${entry.kind}.summary`, undefined, entry.summary),
        output: translateText(lang, `wf.${entry.kind}.output`, undefined, entry.output),
        readiness: entry.readiness,
      })),
      {
        key: "quantize" as const,
        title: translate(lang, "wf.quantize.title"),
        summary: translate(lang, "wf.quantize.summary"),
        output: translate(lang, "wf.quantize.output"),
        readiness: { state: "ready" as const },
      },
    ],
    [workflows, lang],
  );

  const active = rows.find((row) => row.key === kind) ?? rows[rows.length - 1];
  const missing = active.readiness.state === "blocked" ? active.readiness.missing : null;
  const missingText = missing ? missing.map((name) => missingLabel(name, t)).join(listSep(lang)) : "";
  // quantize、抽帧、补间都是纯本机的，不会因为缺能力被挡住；但 readiness 的消息
  // 来自目录，所以目录没取回来之前照样要点不了。
  const pending = kind !== "quantize" && !catalogReady;
  const gated = busy || missing !== null || pending;
  const frameCount = document?.frames.length ?? 0;

  return (
    <section className="panel dock">
      <div className="panel-head">
        <Play size={13} />
        <strong>{t("dock.title")}</strong>
        <span className="grow" />
        {pending ? <span className="dock-pending">{t("dock.pending")}</span> : null}
      </div>

      <div className="kind-list">
        {rows.map((row) => {
          const flag =
            row.readiness.state === "blocked"
              ? row.readiness.missing.map((name) => missingLabel(name, t)).join(listSep(lang))
              : "";
          const hint =
            flag === "" ? row.output : t("dock.blocked_tooltip", { summary: row.summary, missing: flag });
          return (
            <Tooltip key={row.key} title={hint} placement="right">
              <button
                type="button"
                className={`kind-row ${kind === row.key ? "active" : ""} ${flag === "" ? "" : "blocked"}`}
                onClick={() => setKind(row.key)}
              >
                <span className="kind-icon">{KIND_ICONS[row.key]}</span>
                <span className="kind-title">{row.title}</span>
                {flag === "" ? null : (
                  <span className="kind-flag">{t("dock.needs", { missing: flag })}</span>
                )}
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
              message={t("dock.readiness")}
            />
          ) : null}

          {missing ? (
            <Alert
              type="warning"
              showIcon
              className="dock-alert"
              message={t("dock.blocked_title", { title: active.title })}
              description={
                <>
                  {t("dock.blocked_hint", { missing: missingText })}
                  <Button size="small" type="link" onClick={openSettings}>
                    {t("dock.settings_link")}
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
          {kind === "video_brief" ? <VideoBriefPanel gated={gated} /> : null}
          {kind === "frame_tween" ? (
            <TweenPanel gated={gated} frameCount={frameCount} />
          ) : null}
          {kind === "quantize" ? <QuantizePanel gated={busy} /> : null}
        </div>

        {outcome ? (
          <div className="dock-result">
            <div className="dock-result-summary">{renderUiText(lang, outcome.summary)}</div>
            {outcome.detail ? (
              <details className="dock-advanced">
                <summary>{t("dock.detail")}</summary>
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
  const t = useT();

  return (
    <>
      <p className="dock-note">
        {t("dock.agent_note")}
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
        {t("dock.agent_composer")}
      </Button>
    </>
  );
}

function RefinePanel({ gated }: { gated: boolean }) {
  const t = useT();
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
      <Field label={t("dock.idea")}>
        <Input.TextArea
          value={idea}
          size="small"
          autoSize={{ minRows: 2, maxRows: 6 }}
          placeholder={t("dock.idea_placeholder")}
          onChange={(event) => patchDraft({ idea: event.target.value })}
        />
      </Field>
      <Field label={t("dock.target")}>
        <Segmented
          size="small"
          block
          value={target}
          options={[
            { label: t("target.image_gen"), value: "image_gen" },
            { label: t("target.shader"), value: "shader" },
          ]}
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
        {t("dock.refine")}
      </Button>

      {refined ? (
        <>
          <Field label={t("dock.refined_prompt")}>
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
              {t("dock.use_in_gen")}
            </Button>
            <Button
              size="small"
              icon={<MessageSquare size={13} />}
              onClick={() => requestCompose(prompt)}
            >
              {t("dock.send_to_chat")}
            </Button>
          </div>
          {refined.raw.trim() !== refined.prompt.trim() ? (
            <details className="dock-advanced">
              <summary>{t("dock.model_said_more")}</summary>
              <pre>{refined.raw}</pre>
            </details>
          ) : null}
        </>
      ) : null}
    </>
  );
}

function ImageGenPanel({ gated }: { gated: boolean }) {
  const t = useT();
  const draft = useStore((s) => s.dockDraft);
  const document = useStore((s) => s.document);
  const active = useStore((s) => s.active);
  const busy = useStore((s) => s.workflowBusy);
  const patchDraft = useStore((s) => s.patchDraft);
  const runWorkflow = useStore((s) => s.runWorkflow);

  // 帧 id 属于文档，换会话后旧选择会失效：直接推导合法值，不做同步 effect。
  const frameIds = (document?.frames ?? []).map((frame) => frame.id);
  const referenceFrame = frameIds.includes(draft.genFrame)
    ? draft.genFrame
    : active.frame || frameIds[0] || "";

  return (
    <>
      <Field label={t("dock.prompt")}>
        <Input.TextArea
          value={draft.prompt}
          size="small"
          autoSize={{ minRows: 3, maxRows: 10 }}
          placeholder={t("dock.prompt_placeholder")}
          onChange={(event) => patchDraft({ prompt: event.target.value })}
        />
      </Field>
      <Field label={t("dock.size")}>
        <Select
          size="small"
          value={draft.size}
          options={SIZE_OPTIONS.map((size) => ({ label: size, value: size }))}
          onChange={(next) => patchDraft({ size: next })}
        />
      </Field>
      <Field label={t("dock.reference_source")}>
        <Segmented
          size="small"
          block
          value={draft.genSource}
          options={[
            { label: t("ref.canvas_frame"), value: "frame" },
            { label: t("ref.file"), value: "file" },
            { label: t("ref.none"), value: "none" },
          ]}
          onChange={(next) =>
            patchDraft({ genSource: next as DockDraft["genSource"] })
          }
        />
      </Field>
      {draft.genSource === "frame" ? (
        <Field label={t("dock.reference_frame")}>
          <Select
            size="small"
            value={referenceFrame}
            options={frameIds.map((id) => ({ label: id, value: id }))}
            onChange={(next) => patchDraft({ genFrame: next })}
          />
        </Field>
      ) : null}
      {draft.genSource === "file" ? (
        <PathField
          label={t("dock.reference")}
          buttonLabel={t("dock.pick_reference")}
          extensions={IMAGE_EXTENSIONS}
          value={draft.genPath}
          onPick={(path) => patchDraft({ genPath: path })}
          onClear={() => patchDraft({ genPath: null })}
        />
      ) : null}
      <Field label={t("dock.landing")}>
        <Segmented
          size="small"
          block
          value={draft.spot}
          options={[
            { label: t("spot.active_cel"), value: "active_cel" },
            { label: t("spot.new_frame"), value: "new_frame" },
          ]}
          onChange={(next) => patchDraft({ spot: next as LandSpot })}
        />
      </Field>
      <Field label={t("dock.frame_duration")}>
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
            // 只把选中的那一源发出去：两个都给会让 Rust 报歧义，那是调用方的错。
            reference_frame: draft.genSource === "frame" ? referenceFrame : null,
            reference_path: draft.genSource === "file" ? draft.genPath : null,
            spot: draft.spot,
            duration_ms: draft.durationMs,
            options: draft.options,
          })
        }
      >
        {t("dock.generate")}
      </Button>
    </>
  );
}

function VisionPanel({ gated }: { gated: boolean }) {
  const t = useT();
  const path = useStore((s) => s.dockDraft.visionPath);
  const vision = useStore((s) => s.vision);
  const patchDraft = useStore((s) => s.patchDraft);
  const briefReference = useStore((s) => s.briefReference);
  const usePromptInGen = useStore((s) => s.usePromptInGen);
  const requestCompose = useStore((s) => s.requestCompose);

  return (
    <>
      <PathField
        label={t("dock.reference")}
        buttonLabel={t("dock.pick_reference")}
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
          {t("dock.draw_from_this")}
        </Button>
        <Button
          size="small"
          icon={<MessageSquare size={13} />}
          disabled={vision === null}
          onClick={() => {
            if (vision) requestCompose(briefToText(vision));
          }}
        >
          {t("dock.send_to_chat")}
        </Button>
      </div>
    </>
  );
}

function VideoPanel({ gated }: { gated: boolean }) {
  const t = useT();
  const draft = useStore((s) => s.dockDraft);
  const probe = useStore((s) => s.probe);
  const busy = useStore((s) => s.workflowBusy);
  const patchDraft = useStore((s) => s.patchDraft);
  const probeVideo = useStore((s) => s.probeVideo);
  const runWorkflow = useStore((s) => s.runWorkflow);

  return (
    <>
      <PathField
        label={t("dock.clip")}
        buttonLabel={t("dock.pick_video")}
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
      <Field label={t("dock.frames_to_pull")}>
        <InputNumber
          size="small"
          style={{ width: "100%" }}
          min={0}
          max={256}
          value={draft.videoCount}
          addonAfter={draft.videoCount === 0 ? t("dock.all_frames") : t("dock.frames_unit")}
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
        {t("dock.pull_frames")}
      </Button>
    </>
  );
}

/**
 * 读视频模型的画板：挑一段视频，模型逐帧看缩略图，吐出可编辑的运动简报。
 * 和 VideoPanel 的分工是这边不落帧：结果是一段文本，交给生图或对话去画。
 */
function VideoBriefPanel({ gated }: { gated: boolean }) {
  const t = useT();
  const path = useStore((s) => s.dockDraft.videoPath);
  const count = useStore((s) => s.dockDraft.briefCount);
  const probe = useStore((s) => s.probe);
  const brief = useStore((s) => s.videoBrief);
  const busy = useStore((s) => s.workflowBusy);
  const patchDraft = useStore((s) => s.patchDraft);
  const probeVideo = useStore((s) => s.probeVideo);
  const briefVideo = useStore((s) => s.briefVideo);
  const usePromptInGen = useStore((s) => s.usePromptInGen);
  const requestCompose = useStore((s) => s.requestCompose);

  return (
    <>
      <PathField
        label={t("dock.clip")}
        buttonLabel={t("dock.pick_video")}
        extensions={VIDEO_EXTENSIONS}
        value={path}
        onPick={(picked) => {
          patchDraft({ videoPath: picked });
          void probeVideo(picked);
        }}
        onClear={() => patchDraft({ videoPath: null })}
      />
      {path && probe ? (
        <p className="dock-probe">
          {baseName(path)} - {probeSummary(probe.probe, probe.source)}
        </p>
      ) : null}
      <Field label={t("dock.frames_to_pull")}>
        <InputNumber
          size="small"
          style={{ width: "100%" }}
          min={1}
          max={12}
          value={count}
          addonAfter={t("dock.frames_unit")}
          onChange={(next) => patchDraft({ briefCount: next ?? 8 })}
        />
      </Field>
      <Button
        block
        size="small"
        type="primary"
        icon={<Clapperboard size={12} />}
        loading={busy}
        disabled={gated || path === null}
        onClick={() => {
          if (path) void briefVideo(path, count);
        }}
      >
        {t("dock.read_clip")}
      </Button>
      {brief ? <MotionBriefView brief={brief} /> : null}
      <div className="dock-actions">
        <Button
          size="small"
          icon={<ArrowRight size={13} />}
          disabled={gated || brief === null}
          onClick={() => {
            if (brief) usePromptInGen(videoBriefToText(brief));
          }}
        >
          {t("dock.draw_from_this")}
        </Button>
        <Button
          size="small"
          icon={<MessageSquare size={13} />}
          disabled={brief === null}
          onClick={() => {
            if (brief) requestCompose(videoBriefToText(brief));
          }}
        >
          {t("dock.send_to_chat")}
        </Button>
      </div>
    </>
  );
}

function TweenPanel({ gated, frameCount }: { gated: boolean; frameCount: number }) {
  const t = useT();
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
        {t("dock.tween_need_frames")}
      </p>
    );
  }

  return (
    <>
      <Field label={t("dock.from")}>
        <Segmented
          size="small"
          block
          value={from}
          options={frameIds.map((id) => ({ label: id, value: id }))}
          onChange={(next) => patchDraft({ tweenFrom: next })}
        />
      </Field>
      <Field label={t("dock.to")}>
        <Segmented
          size="small"
          block
          value={to}
          options={frameIds.map((id) => ({ label: id, value: id }))}
          onChange={(next) => patchDraft({ tweenTo: next })}
        />
      </Field>
      <Field label={t("dock.frames_in_between", { count: draft.tweenCount })}>
        <Slider
          min={1}
          max={32}
          value={draft.tweenCount}
          onChange={(next) => patchDraft({ tweenCount: next })}
        />
      </Field>
      <Field label={t("dock.mode")}>
        <Segmented
          size="small"
          block
          value={draft.tweenMode}
          options={[
            { label: t("tween.migrate"), value: "migrate" },
            { label: t("tween.blend"), value: "blend" },
            { label: t("tween.copy"), value: "copy" },
          ]}
          onChange={(next) => patchDraft({ tweenMode: next as TweenMode })}
        />
      </Field>
      <Field label={t("dock.order")}>
        <Segmented
          size="small"
          block
          value={draft.tweenOrder}
          options={[
            { label: t("order.scan"), value: "scan" },
            { label: t("order.radial"), value: "radial" },
            { label: t("order.scatter"), value: "scatter" },
          ]}
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
        <span>{t("dock.ease")}</span>
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
        {t("dock.insert")}
      </Button>
    </>
  );
}

function QuantizePanel({ gated }: { gated: boolean }) {
  const t = useT();
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
        label={t("dock.source")}
        buttonLabel={t("dock.pick_image")}
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
        {t("dock.quantize")}
      </Button>
    </>
  );
}
