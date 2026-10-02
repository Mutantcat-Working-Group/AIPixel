// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 工作流命令层：把 agent-core 的七条工作流接到 Tauri 上。
//!
//! 分成三类的道理：生图类要等模型回图，只能异步跑；本机类（插帧、量化）是纯计算，
//! 同步返回更快也更不容易中途改坏文档；探针类只回答「这段素材长什么样」。
//!
// 所有改文档的工作流都走 `AgentSession::with_document_mut`，和 agent 主循环共用同一把锁，
//! 因此不会出现「主循环正在跑，工作流插进去改了画布」的交织。改完统一发带
//! session_id 的 `AgentEvent::DocumentUpdated`，前端只有一条刷新路径。

use agent_core::video_brief::{
    brief_frame_indices, brief_video as brief_video_flow, source_note, BRIEF_THUMB_MAX_DIM,
};
use agent_core::{
    imagegen, refine as refine_flow, video as video_flow, vision, ActiveContext, AgentEvent,
    AgentEventEnvelope, AgentSession, Attachment, AttachmentRole, LandSpot, ModelRole,
    RefineRequest, RefineTarget, UiText,
};
use pixel_core::decode;
use pixel_core::document::Document;
use pixel_core::ops::{self, PixelOperation};
use pixel_core::pixelize::{self, PixelizeOptions, PixelizeReport};
use pixel_core::tween::{self, MigrateOrder, TweenMode, TweenOptions};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, State};

use crate::state::AppState;

/// 视频抽帧的临时落点。放在系统临时目录，不占用用户的工程目录。
pub const FRAME_STAGING_DIR: &str = "aipixel/video-frames";

/// 工作流跑完的统一回执：新 revision、一句人话摘要、以及给 UI 展开看的结构化细节。
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowOutcome {
    pub revision: u64,
    pub summary: UiText,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
}

/// 一条工作流的目录项 + 当前会话的模型跑不跑得动。
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowEntry {
    #[serde(flatten)]
    pub info: agent_core::WorkflowInfo,
    pub readiness: agent_core::Readiness,
    /// 干活那个模型的名字。回落主模型时就是主模型那个。
    pub served_by_label: String,
    /// 这个角色有没有单独绑过模型。false 表示正拿主模型凑，UI 要换个说法。
    pub served_by_detached: bool,
}

/// 视频探针结果。source 决定 UI 该说「抽帧」还是「这就是一串静帧」。
#[derive(Debug, Clone, Serialize)]
pub struct VideoProbeResult {
    pub probe: agent_core::VideoProbe,
    pub source: agent_core::ProbeSource,
}

/// 一次落图的结果：落在哪儿 + 量化的统计。
#[derive(Debug, Clone)]
struct LandedImage {
    layer: String,
    frame: String,
    report: PixelizeReport,
}

/// 抽帧临时目录的收尾。帧一旦读进内存，磁盘上那几百张 png 就没用了，
/// 而留着它们有两个后果：临时目录越用越大；下一次抽帧若沿用旧目录名，
/// 还会把残留帧当成这一回的成果收回来。用 Drop 兜住所有出口——报错、
/// 提前 return、panic 都由它收。
struct StagingGuard {
    dir: Option<PathBuf>,
}

impl StagingGuard {
    /// 只接「这一回自己抽出来的」临时目录。帧摆用户自己的静帧文件夹里时
    /// staging 是 None，收尾时一个字节都不能碰。
    fn new(staging: Option<PathBuf>) -> Self {
        Self { dir: staging }
    }
}

impl Drop for StagingGuard {
    fn drop(&mut self) {
        if let Some(dir) = &self.dir {
            agent_core::discard_staging(dir);
        }
    }
}

// ---------- 入参 ----------

#[derive(Debug, Clone, Deserialize)]
pub struct TweenParams {
    pub from_frame: String,
    pub to_frame: String,
    #[serde(default = "default_count")]
    pub count: usize,
    #[serde(default = "default_tween_mode")]
    pub mode: TweenMode,
    #[serde(default = "default_migrate_order")]
    pub order: MigrateOrder,
    #[serde(default = "default_true")]
    pub ease: bool,
    #[serde(default = "default_frame_duration")]
    pub duration_ms: u32,
    #[serde(default)]
    pub layer: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PixelizeParams {
    /// base64 位图，带不带 `data:` 前缀都行。
    pub image_base64: String,
    /// 裸 base64 时必须给；带 data: 前缀时忽略。
    #[serde(default)]
    pub media_type: Option<String>,
    #[serde(default)]
    pub options: Option<PixelizeOptions>,
    #[serde(default)]
    pub layer: Option<String>,
    #[serde(default)]
    pub frame: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImageGenParams {
    pub prompt: String,
    /// 只有 chat modalities 传输认这个，形如 "1024x1024"。
    #[serde(default)]
    pub size: Option<String>,
    /// 垫图路径：用户拿一张图让模型照着改。
    #[serde(default)]
    pub reference_path: Option<String>,
    /// 垫图帧：把文档里这一帧合成一张图交给模型。
    /// 「改这一帧」和「照这一帧再长一帧」都走这里，画布才是活着的真值。
    /// 与 reference_path 互斥，都填属于调用方的歧义，直接报错而不是猜一个。
    #[serde(default)]
    pub reference_frame: Option<String>,
    #[serde(default)]
    pub options: Option<PixelizeOptions>,
    /// 落到哪个图层。省略用当前激活图层；多图层文档里这是常改的一项。
    #[serde(default)]
    pub layer: Option<String>,
    #[serde(default)]
    pub spot: LandSpot,
    #[serde(default = "default_frame_duration")]
    pub duration_ms: u32,
    /// 输入区那个画风下拉点的一项。与主循环同一条让位规则：解析只走
    /// `agent_core::pins`，认不出的 id 当场报错，不静默退回「不限」——
    /// 那会让用户以为规矩上了路，其实这一张什么都没多带。
    #[serde(default)]
    pub style: Option<String>,
    /// 同时生效的收尾预设 id，可以叠几条。工作流坞手动生图和主循环问的是
    /// 同一个问题（「这一句照什么规矩收尾」），mount 也只挂一份。
    #[serde(default)]
    pub presets: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VideoFramesParams {
    pub path: String,
    /// 0 表示「全都要」，仍受 agent_core::video::MAX_EXTRACT_FRAMES 限制。
    #[serde(default)]
    pub count: usize,
    #[serde(default)]
    pub options: Option<PixelizeOptions>,
    #[serde(default = "default_frame_duration")]
    pub duration_ms: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VideoBriefParams {
    pub path: String,
    /// 从素材里抽多少帧给模型看。0 走默认值；上限由 `brief_frame_indices` 统一收口。
    #[serde(default = "default_brief_count")]
    pub count: usize,
}

fn default_brief_count() -> usize {
    8
}

fn default_count() -> usize {
    4
}

fn default_true() -> bool {
    true
}

fn default_frame_duration() -> u32 {
    83
}

fn default_tween_mode() -> TweenMode {
    TweenMode::Migrate
}

fn default_migrate_order() -> MigrateOrder {
    MigrateOrder::Scan
}

// ---------- 目录与只读查询 ----------

/// 六条工作流 + 当前会话模型的能力判断。能力跟着会话而不是全局激活模型：
/// 用户随时可以把会话改绑到另一个模型。
#[tauri::command]
pub fn workflow_catalog(
    state: State<'_, AppState>,
    id: String,
) -> Result<Vec<WorkflowEntry>, String> {
    let session = state.session(&id)?;
    // 能力按合起来算：生图模型单独绑了也算这个会话会生图，
    // 不然用户明明配了生图模型，面板里那条流程还是灰的。
    let caps = session.effective_capabilities();
    let bindings = session.role_bindings();
    Ok(agent_core::catalog()
        .into_iter()
        .map(|info| {
            let readiness = agent_core::readiness(info.kind, &caps);
            let role = ModelRole::for_workflow(info.kind);
            // 回落主模型时 label 给主模型的名字，detached 给 false：
            // 界面上「由 X 跑」和「用主模型 X 跑」是两回事，别让用户以为藏了个模型。
            let (served, detached) = bindings
                .iter()
                .find(|binding| binding.role == role)
                .map(|binding| (binding.model_label.clone(), binding.detached))
                .unwrap_or_default();
            WorkflowEntry {
                info,
                readiness,
                served_by_label: served,
                served_by_detached: detached,
            }
        })
        .collect())
}

/// 探一段素材：视频走 ffprobe，目录走静帧枚举。只读，不碰文档。
#[tauri::command]
pub async fn video_probe(path: String) -> Result<VideoProbeResult, String> {
    let target = PathBuf::from(&path);
    let (probe, source) = video_flow::probe(&target).await?;
    Ok(VideoProbeResult { probe, source })
}

// ---------- 生图与读图（要等模型） ----------

/// 提示词微调。只产文本，不碰文档：这条工作流的结果是要给用户逐行改的。
// 签名即 IPC 契约：每个参数都从桥那边按名字递进来。收成结构体就要改前端调用
// 形状和 mock，收益只是一处 lint 安静，不值当。
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn prompt_refine(
    state: State<'_, AppState>,
    id: String,
    idea: String,
    width: u32,
    height: u32,
    target: Option<RefineTarget>,
    // 输入区那个画风下拉点的一项。原样交给 agent-core：认不出的 id 在
    // `refine_system_prompt` 里报错。解析只留 `agent_core::pins` 那一份，
    // 三处各写一遍就意味着补了一处、另外两处还站在原地。
    style: Option<String>,
    // 同时生效的收尾预设 id，可以叠几条。同上，原样透传。
    presets: Option<Vec<String>>,
) -> Result<agent_core::RefinedPrompt, String> {
    let session = state.session(&id)?;
    let config = session.model_for_role(ModelRole::Chat);
    let (w, h) = {
        let doc = session.document();
        (doc.width, doc.height)
    };
    let req = RefineRequest {
        idea,
        width: if width == 0 { w } else { width },
        height: if height == 0 { h } else { height },
        target: target.unwrap_or_default(),
        style,
        presets: presets.unwrap_or_default(),
    };
    refine_flow::refine(&config, &req)
        .await
        .map_err(|e| e.to_string())
}

/// 参考图简报：读图模型把图读成结构化文字。刻意不落文档，
/// 因为简报是要给用户改的中间产物，改完再由用户决定发不发去画。
#[tauri::command]
pub async fn vision_brief(
    state: State<'_, AppState>,
    id: String,
    path: String,
) -> Result<vision::VisionBrief, String> {
    let session = state.session(&id)?;
    let config = session.model_for_role(ModelRole::Vision);
    if !config.capabilities.vision {
        return Err(
            "this model is not marked as able to read images; pick a vision model for this session or turn on vision in model settings"
                .into(),
        );
    }
    let attachment = read_reference(&path)?;
    let (w, h) = {
        let doc = session.document();
        (doc.width, doc.height)
    };
    vision::brief_reference(&config, &attachment, w, h)
        .await
        .map_err(|e| e.to_string())
}

/// 生图：模型出位图 -> 量化落到画布上。位图只是原料，权威状态仍是文本网格。
#[tauri::command]
pub async fn workflow_image_gen(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    params: ImageGenParams,
) -> Result<WorkflowOutcome, String> {
    let session = state.session(&id)?;
    let config = session.model_for_role(ModelRole::ImageGen);
    if !config.capabilities.image_gen {
        return Err(
            "this model is not marked as able to generate images; pick an image model for this session or turn on image generation in model settings"
                .into(),
        );
    }
    if params.prompt.trim().is_empty() {
        return Err("image generation needs a prompt".into());
    }
    // 界面钉上来的画风与收尾预设，走 plan 那一份挂载规矩（与画风 id 重合的预设
    // 顶替不发、啥都没点就整段不上、解析只认 pins 那一份）。工作流坞手动生图和
    // 主循环共用同一套收尾要求，不然同一条规矩在主循环里上车、在这里悄悄丢了。
    let pinned_style = agent_core::pins::pinned_style(params.style.as_deref())?;
    let pinned_presets = agent_core::pins::pinned_presets(&params.presets)?;
    let prompt = agent_core::plan::image_prompt(&params.prompt, pinned_style, &pinned_presets);
    // 垫图二选一。画布上的帧优先于磁盘文件：用户要改的是眼前这一帧，
    // 而磁盘上那张可能是好几轮之前的导出。两个都填是调用方的歧义，
    // 报错让它说清楚，不替它猜一个。
    let reference = session.with_document(|doc| {
        resolve_reference(
            doc,
            params.reference_frame.as_deref(),
            params.reference_path.as_deref(),
        )
    })?;
    let generator = imagegen::build_image_generator(&config);
    let request = imagegen::ImageGenParams {
        prompt,
        size: params.size,
        reference,
    };
    emit_status(
        &app,
        &session,
        UiText::new("status.asking_image", "asking the model for an image"),
    );
    let image = generator
        .generate(&request)
        .await
        .map_err(|e| e.to_string())?;
    emit_status(
        &app,
        &session,
        UiText::new(
            "status.quantizing",
            "quantizing the generated image onto the grid",
        ),
    );

    let (rgba, width, height) =
        decode::decode_image(&image.bytes, &image.media_type).map_err(|e| e.to_string())?;
    let opts = params.options.clone().unwrap_or_default();
    let active = session.active();
    let landed = session.with_document_mut(|doc| {
        land_bitmap(
            doc,
            &active,
            LandRequest {
                layer: params.layer.as_deref(),
                spot: params.spot,
                after: None,
                duration_ms: params.duration_ms,
                rgba: &rgba,
                width,
                height,
                opts: &opts,
            },
        )
    })?;

    // 新帧落图后把激活帧挪过去：否则模型下一轮还在旧帧上画，用户看到的也对不上。
    if params.spot == LandSpot::NewFrame {
        let mut next = active.clone();
        next.frame = landed.frame.clone();
        session.set_active(next);
    }
    let revision = emit_document(&app, &session);
    Ok(WorkflowOutcome {
        revision,
        summary: UiText::new(
            "outcome.bitmap_landed",
            "{transport} landed on layer {layer} frame {frame} ({colors} colors, {added} new)",
        )
        .with("transport", image.transport)
        .with("layer", landed.layer.clone())
        .with("colors", landed.report.colors_used as u64)
        .with("frame", landed.frame.clone())
        .with("added", landed.report.palette_added as u64),
        detail: Some(landed_detail(&landed)),
    })
}

/// 视频抽帧并逐帧量化。ffmpeg 不在时退回「目录里的一串静帧」。
#[tauri::command]
pub async fn workflow_video_frames(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    params: VideoFramesParams,
) -> Result<WorkflowOutcome, String> {
    let session = state.session(&id)?;
    let source = PathBuf::from(&params.path);
    let (probe, origin) = video_flow::probe(&source).await?;
    let staging = std::env::temp_dir().join(FRAME_STAGING_DIR);
    let pulled = video_flow::extract_frames(&source, &staging, params.count).await?;
    let _staging = StagingGuard::new(pulled.staging.clone());
    let frames = pulled.frames;
    if frames.is_empty() {
        return Err("no frames came out of that source".into());
    }
    let opts = params.options.clone().unwrap_or_default();
    let active = session.active();

    let mut after: Option<String> = None;
    let mut landed: Vec<LandedImage> = Vec::with_capacity(frames.len());
    let mut failures: Vec<String> = Vec::new();
    for (index, path) in frames.iter().enumerate() {
        emit_status(
            &app,
            &session,
            UiText::new("status.reading_frame", "reading frame {index} of {total}")
                .with("index", (index + 1) as u64)
                .with("total", frames.len() as u64),
        );
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                failures.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        let media_type = media_type_for(path);
        let decoded = match decode::decode_image(&bytes, &media_type) {
            Ok(d) => d,
            Err(e) => {
                failures.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        let (rgba, width, height) = decoded;
        match session.with_document_mut(|doc| {
            land_bitmap(
                doc,
                &active,
                LandRequest {
                    layer: None,
                    spot: LandSpot::NewFrame,
                    after: after.clone(),
                    duration_ms: params.duration_ms,
                    rgba: &rgba,
                    width,
                    height,
                    opts: &opts,
                },
            )
        }) {
            Ok(one) => {
                after = Some(one.frame.clone());
                landed.push(one);
            }
            Err(e) => failures.push(format!("{}: {e}", path.display())),
        }
    }
    if landed.is_empty() {
        let reason = failures.join("; ");
        return Err(if reason.is_empty() {
            "every frame failed to land".into()
        } else {
            reason
        });
    }
    let last_frame = landed[landed.len() - 1].frame.clone();
    let mut next = active.clone();
    next.frame = last_frame;
    session.set_active(next);
    let revision = emit_document(&app, &session);

    // 六种回执：来源三种、有没有跳过两种。键由 Rust 选，措辞由前端说。
    let mut summary = match (origin, failures.is_empty()) {
        (agent_core::ProbeSource::Ffprobe, true) => UiText::new(
            "outcome.video_landed.ffprobe",
            "{count} frame(s) from ffmpeg landed on layer {layer} frames {from}-{to}",
        ),
        (agent_core::ProbeSource::Ffprobe, false) => UiText::new(
            "outcome.video_skipped.ffprobe",
            "{count} frame(s) from ffmpeg landed on layer {layer} frames {from}-{to} ({skipped} skipped)",
        ),
        (agent_core::ProbeSource::Directory, true) => UiText::new(
            "outcome.video_landed.directory",
            "{count} frame(s) from a stills directory landed on layer {layer} frames {from}-{to}",
        ),
        (agent_core::ProbeSource::Directory, false) => UiText::new(
            "outcome.video_skipped.directory",
            "{count} frame(s) from a stills directory landed on layer {layer} frames {from}-{to} ({skipped} skipped)",
        ),
        (_, true) => UiText::new(
            "outcome.video_landed.none",
            "{count} frame(s) landed on layer {layer} frames {from}-{to}",
        ),
        (_, false) => UiText::new(
            "outcome.video_skipped.none",
            "{count} frame(s) landed on layer {layer} frames {from}-{to} ({skipped} skipped)",
        ),
    }
    .with("count", landed.len() as u64)
    .with("layer", landed[0].layer.clone())
    .with("from", landed[0].frame.clone())
    .with("to", landed[landed.len() - 1].frame.clone());
    if !failures.is_empty() {
        summary = summary.with("skipped", failures.len() as u64);
    }
    let detail = json!({
        "source": origin_label(origin),
        "probe_width": probe.width,
        "probe_height": probe.height,
        "duration_s": probe.duration_s,
        "fps": probe.fps,
        "frames": landed.iter().map(|l| l.frame.clone()).collect::<Vec<_>>(),
        "skipped": failures,
    });
    Ok(WorkflowOutcome {
        revision,
        summary,
        detail: Some(detail),
    })
}

/// 读视频简报：把关键帧缩成小图喂给读视频模型，换一份可编辑的运动简报。
///
/// 和 `vision_brief` 一样刻意不落文档：简报是给人改的中间产物，改完由用户
/// 决定发去画，还是丢给 agent 主循环。抽帧在这里只是「给模型准备看的料」，
/// 量化不参与，所以它跟视频抽帧那条工作流是两回事。
#[tauri::command]
pub async fn video_brief(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    params: VideoBriefParams,
) -> Result<agent_core::VideoBrief, String> {
    let session = state.session(&id)?;
    let config = session.model_for_role(ModelRole::Video);
    if !config.capabilities.video {
        return Err(
            "this model is not marked as able to read video; pick a video model for this session or turn on video in model settings"
                .into(),
        );
    }
    let source = PathBuf::from(&params.path);
    let (probe, origin) = video_flow::probe(&source).await?;
    let staging = std::env::temp_dir().join(FRAME_STAGING_DIR);
    let pull = if params.count == 0 {
        default_brief_count()
    } else {
        params.count
    };
    let pulled = video_flow::extract_frames(&source, &staging, pull).await?;
    let _staging = StagingGuard::new(pulled.staging.clone());
    let frames = pulled.frames;
    if frames.is_empty() {
        return Err("no frames came out of that source".into());
    }
    let (w, h) = {
        let doc = session.document();
        (doc.width, doc.height)
    };
    // 抽出来的帧可能远多于模型该看的数量，也可能有的读坏了。
    // 挑下标在读文件之前就定好：省掉无用的 IO，也保证顺序仍是时间序。
    let picked = brief_frame_indices(frames.len());
    let mut attachments: Vec<Attachment> = Vec::with_capacity(picked.len());
    let mut failures: Vec<String> = Vec::new();
    for &index in picked.iter() {
        let path = &frames[index];
        emit_status(
            &app,
            &session,
            UiText::new("status.reading_frame", "reading frame {index} of {total}")
                .with("index", (index + 1) as u64)
                // 报的是「要读几帧」，不是素材里一共有多少帧：只读挑中的那一小撮，
                // 拿总帧数当分母会让进度条永远走不到头。
                .with("total", picked.len() as u64),
        );
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                failures.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        let media_type = media_type_for(path);
        let (thumb, thumb_type) =
            match decode::thumbnail_png(&bytes, &media_type, BRIEF_THUMB_MAX_DIM) {
                Ok(t) => t,
                Err(e) => {
                    failures.push(format!("{}: {e}", path.display()));
                    continue;
                }
            };
        attachments.push(Attachment {
            role: AttachmentRole::Reference,
            media_type: thumb_type,
            data_base64: pixel_core::png::base64_encode(&thumb),
        });
    }
    // 一帧都没读出来就别发请求：模型看到的会是一场缺帧的戏，timing 会全错。
    if attachments.is_empty() {
        let reason = failures.join("; ");
        return Err(if reason.is_empty() {
            "no readable frame came out of that source".into()
        } else {
            reason
        });
    }
    let note = source_note(&probe, origin);
    brief_video_flow(&config, &attachments, w, h, note.as_deref())
        .await
        .map_err(|e| e.to_string())
}

// ---------- 本机计算（同步） ----------

/// 插帧。不吃模型额度，纯粹本机算，所以同步返回。
#[tauri::command]
pub fn workflow_tween(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    params: TweenParams,
) -> Result<WorkflowOutcome, String> {
    let session = state.session(&id)?;
    if params.count == 0 {
        return Err("tween needs at least 1 frame".into());
    }
    let active = session.active();
    let layer = params.layer.clone().unwrap_or_else(|| active.layer.clone());
    let opts = TweenOptions {
        mode: params.mode,
        order: params.order,
        ease: params.ease,
        duration_ms: params.duration_ms,
    };
    let (report, onion) = session
        .with_document_mut(|doc| {
            let report = tween::insert_tween_frames(
                doc,
                &layer,
                &params.from_frame,
                &params.to_frame,
                params.count,
                &opts,
            )?;
            let onion =
                tween::onion_summary(doc, &report.layer, &report.from_frame, &report.to_frame);
            Ok::<_, String>((report, onion))
        })
        .map_err(|e| format!("tween failed: {e}"))?;
    let revision = emit_document(&app, &session);
    Ok(WorkflowOutcome {
        revision,
        summary:
            UiText::new(
                "outcome.tween_inserted",
                "{count} frame(s) inserted between {from} and {to} on layer {layer}; {changed} px changed",
            )
            .with("count", report.created.len() as u64)
            .with("from", report.from_frame.clone())
            .with("to", report.to_frame.clone())
            .with("layer", report.layer.clone())
            .with("changed", report.changed_pixels as u64),
        detail: Some(json!({
            "created": report.created,
            "changed_pixels": report.changed_pixels,
            "palette_added": report.palette_added,
            "layer": report.layer,
            "onion": onion,
        })),
    })
}

/// 把一张位图量化到指定 cel。用户的图、刚生成的图、抽出来的静帧都走这条。
#[tauri::command]
pub fn workflow_pixelize(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    params: PixelizeParams,
) -> Result<WorkflowOutcome, String> {
    let session = state.session(&id)?;
    let active = session.active();
    let layer = params.layer.clone().unwrap_or_else(|| active.layer.clone());
    let frame = params.frame.clone().unwrap_or_else(|| active.frame.clone());
    let opts = params.options.clone().unwrap_or_default();

    let landed = session.with_document_mut(|doc| {
        let report = pixelize_onto_cel(
            doc,
            &layer,
            &frame,
            &params.image_base64,
            params.media_type.as_deref(),
            &opts,
        )?;
        Ok::<_, String>(LandedImage {
            layer,
            frame,
            report,
        })
    })?;
    let revision = emit_document(&app, &session);
    Ok(WorkflowOutcome {
        revision,
        summary: UiText::new(
            "outcome.pixelize_landed",
            "quantized onto layer {layer} frame {frame} ({colors} colors, {added} new)",
        )
        .with("layer", landed.layer.clone())
        .with("frame", landed.frame.clone())
        .with("colors", landed.report.colors_used as u64)
        .with("added", landed.report.palette_added as u64),
        detail: Some(landed_detail(&landed)),
    })
}

// ---------- 内部工具 ----------

/// 文档被工作流改过之后，把新文档推回前端。和主循环的 DocumentUpdated 同一个通道，
/// 前端因此只有一条刷新路径，不需要区分「这次是谁改的」。
pub(crate) fn emit_document(app: &AppHandle, session: &AgentSession) -> u64 {
    // 走增量：整份文档序列化一次够把 47MB JSON 灌进 IPC，webview 直接卡死。
    let patch = session.document_patch();
    let revision = patch.revision;
    let _ = app.emit(
        "agent-event",
        AgentEventEnvelope {
            session_id: session.id().to_string(),
            event: AgentEvent::DocumentUpdated { revision, patch },
        },
    );
    revision
}

/// 发一条状态行。发送失败直接忽略：状态只是旁证，webview 不在场时
/// 这一轮该跑完还是要跑完，不该为了没人看的一行字中断 agent。
fn emit_status(app: &AppHandle, session: &AgentSession, message: UiText) {
    let _ = app.emit(
        "agent-event",
        AgentEventEnvelope {
            session_id: session.id().to_string(),
            event: AgentEvent::Status { message },
        },
    );
}

/// 落地回执 -> 工具结果。数字全部取 report 的原值，不二次加工：
/// 模型要拿「实际用了几色、新增几色」决定下一笔怎么收敛调色板，
/// 而前端显示的是同一份数字，两边才对得上。
fn landed_detail(landed: &LandedImage) -> Value {
    json!({
        "layer": landed.layer,
        "frame": landed.frame,
        "colors_used": landed.report.colors_used,
        "palette_added": landed.report.palette_added,
        "opaque_pixels": landed.report.opaque_pixels,
        "transparent_pixels": landed.report.transparent_pixels,
        "fit": landed.report.fit,
    })
}

/// 视频信息的来源 -> i18n key。给键名不给中文：文案由前端按当前语种取，
/// Rust 侧多一种语言就得多维护一份词表。
fn origin_label(origin: agent_core::ProbeSource) -> &'static str {
    match origin {
        agent_core::ProbeSource::Ffprobe => "origin.ffprobe",
        agent_core::ProbeSource::Directory => "origin.directory",
        agent_core::ProbeSource::None => "origin.none",
    }
}

/// 把一张位图量化到指定 cel。cel 的校验刻意排在解码之前：
/// 一个写错的帧 id 不该让用户赔上一次几百 KB 的解码，错误也应该先说帧的事。
fn pixelize_onto_cel(
    doc: &mut Document,
    layer: &str,
    frame: &str,
    encoded: &str,
    media_type: Option<&str>,
    opts: &PixelizeOptions,
) -> Result<PixelizeReport, String> {
    if doc.cel(layer, frame).is_none() {
        return Err(format!("unknown cel: {layer}/{frame}"));
    }
    let (rgba, width, height) = decode_payload(encoded, media_type)?;
    pixelize::pixelize_into_cel(doc, layer, frame, &rgba, width, height, opts)
}

/// 位图入参：优先按 data URL 解，退化到裸 base64 + media_type。
fn decode_payload(encoded: &str, media_type: Option<&str>) -> Result<(Vec<u8>, u32, u32), String> {
    let trimmed = encoded.trim();
    if trimmed.is_empty() {
        return Err("image payload is empty".into());
    }
    if trimmed.starts_with("data:") {
        return decode::decode_data_url(trimmed);
    }
    let media_type = media_type
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .ok_or("media_type is required when the image is bare base64")?;
    let bytes = decode::decode_base64(trimmed)?;
    decode::decode_image(&bytes, media_type)
}

/// 读一张参考图成附件。简报与垫图共用，角色恒为 reference：
/// 快照是上下文，参考图才是真值，混起来模型会照着截图临摹。
fn read_reference(path: &str) -> Result<Attachment, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    Ok(Attachment {
        role: AttachmentRole::Reference,
        media_type: media_type_for(PathBuf::from(path).as_path()),
        data_base64: pixel_core::png::base64_encode(&bytes),
    })
}

/// 定下这次生图的垫图。画布帧与磁盘文件二选一，
/// 两个都给说明调用方自己没想清楚，报错比猜一个更负责。
fn resolve_reference(
    doc: &Document,
    frame: Option<&str>,
    path: Option<&str>,
) -> Result<Option<Attachment>, String> {
    match (frame, path) {
        (Some(frame), None) => Ok(Some(imagegen::frame_reference(doc, frame)?)),
        (None, Some(path)) => Ok(Some(read_reference(path)?)),
        (None, None) => Ok(None),
        (Some(_), Some(_)) => Err("pick one reference: a canvas frame or a file, not both".into()),
    }
}

/// 按扩展名猜 media type；认不出来就交给 decode 做内容嗅探。
fn media_type_for(path: &std::path::Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        _ => "image/png",
    }
    .to_string()
}

/// 一次落图的全部输入。合成一个结构而不是散一串参数：
/// 「落在哪儿」「按什么量化」「图是什么」本来就是同一件事。
struct LandRequest<'a> {
    /// 目标图层；None 表示当前激活图层。
    layer: Option<&'a str>,
    spot: LandSpot,
    /// NewFrame 时新帧的锚点；None 表示追加到激活帧之后。
    /// 连续抽帧必须一帧接一帧，不能每次都插回同一个锚点后面。
    after: Option<String>,
    duration_ms: u32,
    rgba: &'a [u8],
    width: u32,
    height: u32,
    opts: &'a PixelizeOptions,
}

/// 把一张位图落到文档上。
fn land_bitmap(
    doc: &mut Document,
    active: &ActiveContext,
    req: LandRequest<'_>,
) -> Result<LandedImage, String> {
    // 图层显式指定就用指定的：用户在面板里挑的那一层，别拿激活层覆盖他的选择。
    let layer = match req.layer {
        Some(id) if doc.layers.iter().any(|l| l.id == id) => id.to_string(),
        Some(id) => return Err(format!("unknown layer: {id}")),
        None => active.layer.clone(),
    };
    let (layer, frame) = match req.spot {
        LandSpot::ActiveCel => (layer, active.frame.clone()),
        LandSpot::NewFrame => {
            let anchor = req.after.as_deref().unwrap_or(&active.frame);
            let position = doc
                .frames
                .iter()
                .position(|f| f.id == anchor)
                .ok_or_else(|| format!("unknown frame: {anchor}"))?;
            ops::apply_batch(
                doc,
                &[PixelOperation::CreateFrame {
                    after: Some(anchor.to_string()),
                    duration_ms: req.duration_ms.clamp(1, 60_000),
                    id: None,
                }],
            )
            .map_err(|e| e.to_string())?;
            // CreateFrame 把新帧插在锚点之后，所以位置就在 position + 1。
            let created = doc
                .frames
                .get(position + 1)
                .ok_or("the new frame did not land after the anchor")?
                .id
                .clone();
            (layer, created)
        }
    };
    let report = pixelize::pixelize_into_cel(
        doc, &layer, &frame, req.rgba, req.width, req.height, req.opts,
    )?;
    Ok(LandedImage {
        layer,
        frame,
        report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixel_core::document::Document;

    fn doc() -> Document {
        Document::new("t", 8, 8).unwrap()
    }

    fn active() -> ActiveContext {
        ActiveContext {
            layer: "L0".into(),
            frame: "F0".into(),
            color: None,
        }
    }

    /// 一张 2x2 的纯红 PNG，2x2 是 image crate 无条件支持的尺寸。
    fn red_png_b64() -> String {
        let mut d = doc();
        d.palette = vec![pixel_core::Rgba::rgb(255, 0, 0)];
        if let Some(cel) = d.cel_mut("L0", "F0") {
            cel.indices.iter_mut().for_each(|i| *i = 1);
        }
        let bytes = pixel_core::png::document_to_png(&d).unwrap();
        pixel_core::png::base64_encode(&bytes)
    }

    #[test]
    fn lands_a_bitmap_on_a_new_frame_after_the_anchor() {
        let mut d = doc();
        let opts = PixelizeOptions::default();
        let bytes = decode_base64_bare(&red_png_b64());
        let (rgba, w, h) = decode::decode_image(&bytes, "image/png").unwrap();
        let first = land_bitmap(
            &mut d,
            &active(),
            LandRequest {
                layer: None,
                spot: LandSpot::NewFrame,
                after: None,
                duration_ms: 100,
                rgba: &rgba,
                width: w,
                height: h,
                opts: &opts,
            },
        )
        .unwrap();
        assert_eq!(first.frame, "F1");
        assert_eq!(d.frames.len(), 2);
        // 第二次要接着第一次往下排，不能又插回 F0 后面
        let second = land_bitmap(
            &mut d,
            &active(),
            LandRequest {
                layer: None,
                spot: LandSpot::NewFrame,
                after: Some(first.frame.clone()),
                duration_ms: 100,
                rgba: &rgba,
                width: w,
                height: h,
                opts: &opts,
            },
        )
        .unwrap();
        assert_eq!(second.frame, "F2");
        assert_eq!(d.frames[1].id, "F1");
        assert_eq!(d.frames[2].id, "F2");
    }

    #[test]
    fn active_cel_spot_leaves_the_frame_list_alone() {
        let mut d = doc();
        let opts = PixelizeOptions::default();
        let bytes = decode_base64_bare(&red_png_b64());
        let (rgba, w, h) = decode::decode_image(&bytes, "image/png").unwrap();
        let landed = land_bitmap(
            &mut d,
            &active(),
            LandRequest {
                layer: None,
                spot: LandSpot::ActiveCel,
                after: None,
                duration_ms: 100,
                rgba: &rgba,
                width: w,
                height: h,
                opts: &opts,
            },
        )
        .unwrap();
        assert_eq!(landed.frame, "F0");
        assert_eq!(landed.layer, "L0");
        assert_eq!(d.frames.len(), 1);
        assert_eq!(landed.report.colors_used, 1);
    }

    #[test]
    fn a_named_layer_lands_where_the_user_pointed_not_on_the_active_one() {
        // 面板里挑的那一层大于激活层：用户指向哪儿就落哪儿。
        let mut d = doc();
        ops::apply_batch(
            &mut d,
            &[PixelOperation::CreateLayer {
                after: None,
                name: Some("outline".into()),
                id: None,
                palette_id: None,
                locked: None,
            }],
        )
        .unwrap();
        let target = d
            .layers
            .last()
            .expect("create_layer appends one")
            .id
            .clone();
        let opts = PixelizeOptions::default();
        let bytes = decode_base64_bare(&red_png_b64());
        let (rgba, w, h) = decode::decode_image(&bytes, "image/png").unwrap();
        let landed = land_bitmap(
            &mut d,
            &active(),
            LandRequest {
                layer: Some(&target),
                spot: LandSpot::ActiveCel,
                after: None,
                duration_ms: 100,
                rgba: &rgba,
                width: w,
                height: h,
                opts: &opts,
            },
        )
        .unwrap();
        assert_eq!(landed.layer, target);
        assert_eq!(landed.frame, "F0");
        // 落进去的是那一层，激活层上还是空的。
        assert!(d.cels["L0"]["F0"].indices.iter().all(|i| *i == 0));
        assert!(d.cels[&target]["F0"].indices.iter().any(|i| *i != 0));
    }

    #[test]
    fn an_unknown_layer_is_reported_before_anything_is_drawn() {
        let mut d = doc();
        let opts = PixelizeOptions::default();
        let err = land_bitmap(
            &mut d,
            &active(),
            LandRequest {
                layer: Some("L9"),
                spot: LandSpot::ActiveCel,
                after: None,
                duration_ms: 100,
                rgba: &[0, 0, 0, 0],
                width: 0,
                height: 0,
                opts: &opts,
            },
        )
        .unwrap_err();
        assert!(err.contains("unknown layer"), "{err}");
    }

    #[test]
    fn a_bad_frame_is_reported_before_the_payload_is_even_looked_at() {
        // 载荷是空的，仍然要先报 cel 的错：校验顺序本身就是这条断言在守的东西。
        let mut d = doc();
        let err = pixelize_onto_cel(
            &mut d,
            "L0",
            "F9",
            "",
            Some("image/png"),
            &PixelizeOptions::default(),
        )
        .unwrap_err();
        assert!(err.contains("L0/F9"), "{err}");
    }

    #[test]
    fn bare_base64_demands_a_media_type_but_a_data_url_does_not() {
        let mut d = doc();
        let bare = decode_payload("AAAA", None).unwrap_err();
        assert!(bare.contains("media_type"), "{bare}");
        let url = format!("data:image/png;base64,{}", red_png_b64());
        let report =
            pixelize_onto_cel(&mut d, "L0", "F0", &url, None, &PixelizeOptions::default()).unwrap();
        assert_eq!(report.colors_used, 1);
        assert_eq!(report.palette_added, 1);
    }

    #[test]
    fn media_type_falls_back_to_png_for_unknown_extensions() {
        assert_eq!(media_type_for(std::path::Path::new("a.webp")), "image/webp");
        assert_eq!(media_type_for(std::path::Path::new("a.jpeg")), "image/jpeg");
        assert_eq!(media_type_for(std::path::Path::new("a.xyz")), "image/png");
        assert_eq!(media_type_for(std::path::Path::new("a")), "image/png");
    }

    #[test]
    fn a_frame_reference_beats_a_stale_file_and_rejects_getting_both() {
        let d = doc();
        // 帧优先：文档里只有 F0，所以这条走得通。
        let chosen = resolve_reference(&d, Some("F0"), None).unwrap();
        assert_eq!(
            chosen.expect("frame reference").role,
            AttachmentRole::Reference
        );

        // 两个都给 = 调用方没想清楚。宁可报错也不猜。
        let both = resolve_reference(&d, Some("F0"), Some("/tmp/never-read.png")).unwrap_err();
        assert!(both.contains("not both"), "{both}");

        // 帧 id 写错时，错话说的是帧，不是文件。
        let missing = resolve_reference(&d, Some("F7"), None).unwrap_err();
        assert!(missing.contains("F7"), "{missing}");
    }

    #[test]
    fn a_reference_file_is_read_as_png_bytes() {
        use std::io::Write;

        let dir = std::env::temp_dir().join("aipixel/reference-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ref.png");
        let bytes = decode_base64_bare(&red_png_b64());
        std::fs::File::create(&path)
            .unwrap()
            .write_all(&bytes)
            .unwrap();

        let attachment = resolve_reference(&doc(), None, Some(path.to_str().unwrap())).unwrap();
        let got = attachment.expect("file reference");
        assert_eq!(got.media_type, "image/png");
        assert_eq!(got.role, AttachmentRole::Reference);
        assert_eq!(
            pixel_core::decode::decode_base64(&got.data_base64).unwrap(),
            bytes
        );
    }

    fn decode_base64_bare(text: &str) -> Vec<u8> {
        decode::decode_base64(text).unwrap()
    }
}
