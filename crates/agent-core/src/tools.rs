//! 三个落地工具的封装，把模型入参翻译成 pixel-core 的类型化操作。
//! 「不让模型手写矩阵」的契约就在这一层收口。

use super::imagegen::LandSpot;
use super::models::{ActiveContext, ToolSpec};
use pixel_core::context;
use pixel_core::document::Document;
use pixel_core::ops::{self, PixelOperation};
use pixel_core::rle::{self, CanvasView};
use pixel_core::shader::{self, ShaderBudget};
use serde_json::{json, Value};

use pixel_core::decode;
use pixel_core::pixelize::{self, FitMode, PixelizeOptions};
use pixel_core::tween::{self, MigrateOrder, TweenMode, TweenOptions};

/// 工具回填给模型的 RLE 网格字符预算。
const TOOL_GRID_CHARS: usize = 3000;

/// agent 生图工具名。同步工具走 `tools::execute`；这一个要等模型回图，
/// 由 runner 分流到异步路径，所以名字单独抽出来给规格、execute 兜底、runner 共用。
pub const IMAGE_GEN_TOOL: &str = "pixel_generate_image";

#[derive(Debug, Clone)]
pub struct ToolOutcome {
    pub content: String,
    pub is_error: bool,
}

fn ok(content: String) -> ToolOutcome {
    ToolOutcome {
        content,
        is_error: false,
    }
}

fn err(content: String) -> ToolOutcome {
    ToolOutcome {
        content,
        is_error: true,
    }
}

/// 三个工具的对外规格（协议无关，schema 为 JSON Schema）。
pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "pixel_apply_operations".into(),
            description: "Apply ONE transaction of typed pixel/layer/frame/palette operations. Structure ops (create/move/delete/duplicate/rename layers and frames, set duration, add palette colors) and tiny precise pixel patches (set_pixels, stamp_grid, draw_shape, bucket_fill, clear_region). Fails atomically if any operation is invalid; the error names the failing operation index.".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "operations": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "op": {
                                    "type": "string",
                                    "enum": ["create_frame","delete_frame","duplicate_frame","move_frame","set_frame_duration","create_layer","delete_layer","move_layer","rename_layer","set_layer_properties","add_palette_colors","set_pixels","bucket_fill","draw_shape","clear_region","stamp_grid"]
                                },
                                "id": {"type": "string"},
                                "after": {"type": "string"},
                                "duration_ms": {"type": "integer"},
                                "name": {"type": "string"},
                                "layer": {"type": "string"},
                                "frame": {"type": "string"},
                                "x": {"type": "integer"}, "y": {"type": "integer"},
                                "x0": {"type": "integer"}, "y0": {"type": "integer"}, "x1": {"type": "integer"}, "y1": {"type": "integer"},
                                "w": {"type": "integer"}, "h": {"type": "integer"},
                                "to_index": {"type": "integer"},
                                "color": {"type": "string", "description": "#RRGGBB or #RRGGBBAA"},
                                "filled": {"type": "boolean"},
                                "shape": {"type": "string", "enum": ["line","rect","ellipse"]},
                                "cells": {"type": "array", "items": {"type": "object", "properties": {"x": {"type":"integer"}, "y": {"type":"integer"}, "color": {"type":"string"}}, "required": ["x","y","color"]}},
                                "rows": {"type": "array", "items": {"type": "string"}},
                                "legend": {"type": "object", "additionalProperties": {"type": "string"}},
                                "colors": {"type": "array", "items": {"type": "string"}},
                                "visible": {"type": "boolean"},
                                "opacity": {"type": "integer"}
                            },
                            "required": ["op"]
                        }
                    }
                },
                "required": ["operations"]
            }),
        },
        ToolSpec {
            name: "pixel_read_canvas".into(),
            description: "Read the current canvas as an RLE grid. Pass {\"overview\": true} for a downsampled whole-canvas map on large canvases, or {\"region\": {x,y,width,height}} (up to 128x128) for an exact window. With no arguments it returns the active-layer window.".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "overview": {"type": "boolean"},
                    "region": {
                        "type": "object",
                        "properties": {
                            "x": {"type": "integer"},
                            "y": {"type": "integer"},
                            "width": {"type": "integer"},
                            "height": {"type": "integer"}
                        },
                        "required": ["x", "y", "width", "height"]
                    }
                }
            }),
        },
        ToolSpec {
            name: "pixel_run_shader".into(),
            description: "Draw by running ONE sandboxed Lua script whose size is independent of the canvas. Supports pset/line/rect/ellipse/circle/flood/stamp/replace/outline/clear and pal/mix/hsv/alpha/hex/noise/rand. Pass animate=true to render every existing frame driven by phase/time. Use this for all artwork.".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "script": {"type": "string"},
                    "animate": {"type": "boolean"},
                    "layer": {"type": "string"}
                },
                "required": ["script"]
            }),
        },
        ToolSpec {
            name: "pixel_tween_frames".into(),
            description: "Insert in-between frames between two existing frames. Use it when the user asks for tweening, in-betweens, motion between two poses, or a dissolve/fade between frame A and frame B. mode 'migrate' flips differing pixels in order (pixel-art-correct deformation), 'blend' interpolates colors, 'copy' is a placeholder holding the start frame.".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "from_frame": {"type": "string"},
                    "to_frame": {"type": "string"},
                    "count": {"type": "integer", "minimum": 1, "maximum": 64},
                    "mode": {"type": "string", "enum": ["copy","blend","migrate"]},
                    "order": {"type": "string", "enum": ["scan","radial","scatter"]},
                    "ease": {"type": "boolean", "description": "smoothstep the migration progress"},
                    "duration_ms": {"type": "integer", "description": "duration stamped on every created frame"},
                    "layer": {"type": "string"}
                },
                "required": ["from_frame", "to_frame", "count"]
            }),
        },
        ToolSpec {
            name: "pixel_pixelize_image".into(),
            description: "Turn a bitmap (base64 PNG/JPEG, typically the output of an image-generation model) into indexed pixels on the target cel. Quantizes onto the canvas palette, reusing colors that are already close instead of bloating it. Use this to land a generated image onto the grid rather than describing it pixel by pixel.".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "image_base64": {"type": "string", "description": "base64 image data, with or without a data: prefix"},
                    "media_type": {"type": "string", "description": "required only when image_base64 has no data: prefix, e.g. image/png"},
                    "max_colors": {"type": "integer", "minimum": 2, "maximum": 256},
                    "dither": {"type": "boolean"},
                    "fit": {"type": "string", "enum": ["contain","stretch"]},
                    "snap_tolerance": {"type": "integer", "minimum": 0, "maximum": 128},
                    "expand_palette": {"type": "boolean"},
                    "alpha_threshold": {"type": "integer", "minimum": 0, "maximum": 255},
                    "layer": {"type": "string"},
                    "frame": {"type": "string"}
                },
                "required": ["image_base64"]
            }),
        },
        ToolSpec {
            name: IMAGE_GEN_TOOL.into(),
            description: "Ask the image-generation model to paint a bitmap, then quantize it onto the canvas grid. Use it for painterly, richly shaded, or photorealistic results that a Lua shader or typed ops cannot express. By default it overwrites the active cel. To MODIFY the current artwork (eg 'make the headdress bigger'), pass the current frame id in reference_frame so the model sees it as a reference image. Set spot='new_frame' to land the result on a freshly created frame instead. The model must have image generation enabled.".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "prompt": {"type": "string", "description": "what to draw: subject, proportions, palette, pose"},
                    "reference_frame": {"type": "string", "description": "frame id to send as a reference image; pass the current frame id to edit it in place"},
                    "size": {"type": "string", "description": "optional size hint like 1024x1024; honored by the image endpoints, folded into an aspect ratio on chat-modalities models"},
                    "spot": {"type": "string", "enum": ["active_cel","new_frame"], "description": "overwrite the active cel (default) or land on a new_frame"},
                    "layer": {"type": "string"},
                    "frame": {"type": "string", "description": "target cel for active_cel, or the anchor a new_frame is inserted after"},
                    "duration_ms": {"type": "integer", "description": "duration stamped on a new_frame"},
                    "max_colors": {"type": "integer", "minimum": 2, "maximum": 256},
                    "dither": {"type": "boolean"},
                    "expand_palette": {"type": "boolean"},
                    "alpha_threshold": {"type": "integer", "minimum": 0, "maximum": 255},
                    "snap_tolerance": {"type": "integer", "minimum": 0, "maximum": 128},
                    "fit": {"type": "string", "enum": ["contain","stretch"]}
                },
                "required": ["prompt"]
            }),
        },
    ]
}

/// 执行一个工具调用，返回回填给模型的结果文本。
pub fn execute(
    doc: &mut Document,
    active: &ActiveContext,
    name: &str,
    input: &Value,
) -> ToolOutcome {
    match name {
        "pixel_apply_operations" => tool_apply_operations(doc, active, input),
        "pixel_read_canvas" => tool_read_canvas(doc, active, input),
        "pixel_run_shader" => tool_run_shader(doc, active, input),
        "pixel_tween_frames" => tool_tween_frames(doc, active, input),
        "pixel_pixelize_image" => tool_pixelize_image(doc, active, input),
        // 生图要等模型回图，正常由 runner 直接分流、不会进这里；万一有人直接调
        // execute，也回一句能听懂的话，而不是「unknown tool」。
        IMAGE_GEN_TOOL => err(format!(
            "{IMAGE_GEN_TOOL} runs asynchronously in the agent loop and cannot run inside tools::execute"
        )),
        other => err(format!("unknown tool: {other}")),
    }
}

fn tool_apply_operations(doc: &mut Document, active: &ActiveContext, input: &Value) -> ToolOutcome {
    let Some(arr) = input.get("operations").and_then(|o| o.as_array()) else {
        return err("pixel_apply_operations: missing 'operations' array".into());
    };
    let mut ops: Vec<PixelOperation> = Vec::with_capacity(arr.len());
    for (i, v) in arr.iter().enumerate() {
        match serde_json::from_value::<PixelOperation>(v.clone()) {
            Ok(op) => ops.push(op),
            Err(e) => return err(format!("operation[{i}] invalid: {e}")),
        }
    }
    match ops::apply_batch(doc, &ops) {
        Ok(rev) => {
            let mut content = format!(
                "applied {} operation(s); revision is now {rev}\n",
                ops.len()
            );
            content.push_str(&active_grid(doc, &active.layer, &active.frame));
            ok(content)
        }
        Err(e) => err(format!("operation failed: {e}")),
    }
}

fn tool_read_canvas(doc: &Document, active: &ActiveContext, input: &Value) -> ToolOutcome {
    let overview = input
        .get("overview")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if overview {
        return match context::overview_context(doc, &active.layer, &active.frame, 4) {
            Ok(s) => ok(s),
            Err(e) => err(format!("read_canvas overview: {e}")),
        };
    }
    if let Some(region) = input.get("region") {
        let x = region.get("x").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let y = region.get("y").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let w = region
            .get("width")
            .and_then(|v| v.as_u64())
            .unwrap_or(doc.width as u64) as u32;
        let h = region
            .get("height")
            .and_then(|v| v.as_u64())
            .unwrap_or(doc.height as u64) as u32;
        return match rle::render_window(
            doc,
            &active.layer,
            &active.frame,
            Some((x, y, w, h)),
            TOOL_GRID_CHARS,
        ) {
            Some(view) => ok(format_view(&view)),
            None => err("read_canvas: cel not found for active layer/frame".into()),
        };
    }
    match context::cel_context(doc, &active.layer, &active.frame, TOOL_GRID_CHARS) {
        Ok(s) => ok(s),
        Err(e) => err(format!("read_canvas: {e}")),
    }
}

fn tool_run_shader(doc: &mut Document, active: &ActiveContext, input: &Value) -> ToolOutcome {
    let Some(script) = input.get("script").and_then(|s| s.as_str()) else {
        return err("pixel_run_shader: missing 'script'".into());
    };
    if script.trim().is_empty() {
        return err("pixel_run_shader: empty script".into());
    }
    let animate = input
        .get("animate")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let layer = input
        .get("layer")
        .and_then(|s| s.as_str())
        .unwrap_or(&active.layer);
    match shader::run_shader(doc, layer, script, animate, &ShaderBudget::default()) {
        Ok(outcome) => {
            let mut content = format!(
                "shader ok on layer {layer}: {} frame(s) rendered, {} opaque px, +{} palette color(s)\n",
                outcome.frames_rendered, outcome.opaque_pixels, outcome.palette_additions
            );
            content.push_str(&active_grid(doc, layer, &active.frame));
            ok(content)
        }
        Err(e) => err(format!("shader error: {e}")),
    }
}

fn tool_tween_frames(doc: &mut Document, active: &ActiveContext, input: &Value) -> ToolOutcome {
    let Some(from) = input.get("from_frame").and_then(|s| s.as_str()) else {
        return err("pixel_tween_frames: missing 'from_frame'".into());
    };
    let Some(to) = input.get("to_frame").and_then(|s| s.as_str()) else {
        return err("pixel_tween_frames: missing 'to_frame'".into());
    };
    let count = input.get("count").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
    let layer = input
        .get("layer")
        .and_then(|s| s.as_str())
        .unwrap_or(&active.layer);
    let opts = TweenOptions {
        mode: match input.get("mode").and_then(|s| s.as_str()) {
            Some("copy") => TweenMode::Copy,
            Some("blend") => TweenMode::Blend,
            _ => TweenMode::Migrate,
        },
        order: match input.get("order").and_then(|s| s.as_str()) {
            Some("radial") => MigrateOrder::Radial,
            Some("scatter") => MigrateOrder::Scatter,
            _ => MigrateOrder::Scan,
        },
        ease: input.get("ease").and_then(|v| v.as_bool()).unwrap_or(true),
        duration_ms: input
            .get("duration_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(83) as u32,
    };
    match tween::insert_tween_frames(doc, layer, from, to, count, &opts) {
        Ok(report) => {
            let mut content = format!(
                "tween {} -> {} on layer {}: {} frame(s) inserted ({}), {} px changed, +{} palette color(s)\n",
                report.from_frame,
                report.to_frame,
                report.layer,
                report.created.len(),
                report.created.join(", "),
                report.changed_pixels,
                report.palette_added
            );
            content.push_str(&tween::onion_summary(doc, &report.layer, from, to));
            content.push('\n');
            content.push_str(&active_grid(doc, layer, to));
            ok(content)
        }
        Err(e) => err(format!("tween failed: {e}")),
    }
}

fn tool_pixelize_image(doc: &mut Document, active: &ActiveContext, input: &Value) -> ToolOutcome {
    let Some(encoded) = input.get("image_base64").and_then(|s| s.as_str()) else {
        return err("pixel_pixelize_image: missing 'image_base64'".into());
    };
    if encoded.trim().is_empty() {
        return err("pixel_pixelize_image: empty image data".into());
    }
    let layer = input
        .get("layer")
        .and_then(|s| s.as_str())
        .unwrap_or(&active.layer);
    let frame = input
        .get("frame")
        .and_then(|s| s.as_str())
        .unwrap_or(&active.frame);
    // 先确认落点再解码：几百 KB 的解码不该挡在一个拼错的帧 id 前面。
    if doc.cel(layer, frame).is_none() {
        return err(format!("unknown cel: {layer}/{frame}"));
    }
    // 允许带 data: 前缀（生图接口常见），也允许裸 base64 + media_type。
    let (rgba, src_w, src_h) = if encoded.trim_start().starts_with("data:") {
        match decode::decode_data_url(encoded) {
            Ok(v) => v,
            Err(e) => return err(format!("pixel_pixelize_image: {e}")),
        }
    } else {
        let media = input.get("media_type").and_then(|s| s.as_str());
        let Some(media) = media else {
            return err(
                "pixel_pixelize_image: bare base64 needs a 'media_type' such as image/png".into(),
            );
        };
        let bytes = match decode::decode_base64(encoded) {
            Ok(b) => b,
            Err(e) => return err(format!("pixel_pixelize_image: {e}")),
        };
        match decode::decode_image(&bytes, media) {
            Ok(v) => v,
            Err(e) => return err(format!("pixel_pixelize_image: {e}")),
        }
    };
    let layer = input
        .get("layer")
        .and_then(|s| s.as_str())
        .unwrap_or(&active.layer);
    let frame = input
        .get("frame")
        .and_then(|s| s.as_str())
        .unwrap_or(&active.frame);
    // options 必须落进同一类型，模型少给字段时用默认值兜住。
    let mut opts = PixelizeOptions::default();
    if let Some(v) = input.get("max_colors").and_then(|v| v.as_u64()) {
        opts.max_colors = (v as usize).clamp(2, 256);
    }
    if let Some(v) = input.get("dither").and_then(|v| v.as_bool()) {
        opts.dither = v;
    }
    if let Some(v) = input.get("snap_tolerance").and_then(|v| v.as_u64()) {
        opts.snap_tolerance = (v as u32).clamp(0, 128);
    }
    if let Some(v) = input.get("expand_palette").and_then(|v| v.as_bool()) {
        opts.expand_palette = v;
    }
    if let Some(v) = input.get("alpha_threshold").and_then(|v| v.as_u64()) {
        opts.alpha_threshold = v as u8;
    }
    if input.get("fit").and_then(|s| s.as_str()) == Some("stretch") {
        opts.fit = FitMode::Stretch;
    }
    match pixelize::pixelize_into_cel(doc, layer, frame, &rgba, src_w, src_h, &opts) {
        Ok(report) => {
            let mut content = format!(
                "pixelized {}x{} source onto {layer}/{frame} ({} fit): {} opaque, {} transparent px, {} color(s) used, +{} palette color(s)\n",
                src_w,
                src_h,
                match report.fit {
                    FitMode::Contain => "contain",
                    FitMode::Stretch => "stretch",
                },
                report.opaque_pixels,
                report.transparent_pixels,
                report.colors_used,
                report.palette_added
            );
            content.push_str(&active_grid(doc, layer, frame));
            ok(content)
        }
        Err(e) => err(format!("pixelize failed: {e}")),
    }
}

/// agent 生图工具的入参。和 pixel_pixelize_image 的分工：位图由工具自己向模型要，
/// 模型只描述「画什么」，不手传 base64；落点与量化选项在这里一次性收口。
#[derive(Debug, Clone)]
pub struct ImageGenToolParams {
    pub prompt: String,
    pub size: Option<String>,
    /// 垫图帧：把文档里这一帧合成一张图交给模型，是「改这一帧」的关键。
    pub reference_frame: Option<String>,
    pub layer: Option<String>,
    pub frame: Option<String>,
    pub spot: LandSpot,
    pub duration_ms: u32,
    pub opts: PixelizeOptions,
}

impl ImageGenToolParams {
    /// 从工具入参解析。空串一律当成没给，避免模型用 "" 占位。
    pub fn parse(input: &Value) -> Result<Self, String> {
        let field = |key: &str| -> Option<String> {
            input
                .get(key)
                .and_then(|v| v.as_str())
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        };
        let Some(prompt) = field("prompt") else {
            return Err(format!(
                "{IMAGE_GEN_TOOL}: needs a non-empty 'prompt' describing what to draw"
            ));
        };
        let mut opts = PixelizeOptions::default();
        if let Some(v) = input.get("max_colors").and_then(|v| v.as_u64()) {
            opts.max_colors = (v as usize).clamp(2, 256);
        }
        if let Some(v) = input.get("dither").and_then(|v| v.as_bool()) {
            opts.dither = v;
        }
        if let Some(v) = input.get("snap_tolerance").and_then(|v| v.as_u64()) {
            opts.snap_tolerance = (v as u32).clamp(0, 128);
        }
        if let Some(v) = input.get("expand_palette").and_then(|v| v.as_bool()) {
            opts.expand_palette = v;
        }
        if let Some(v) = input.get("alpha_threshold").and_then(|v| v.as_u64()) {
            opts.alpha_threshold = v as u8;
        }
        if input.get("fit").and_then(|s| s.as_str()) == Some("stretch") {
            opts.fit = FitMode::Stretch;
        }
        Ok(ImageGenToolParams {
            prompt,
            size: field("size"),
            reference_frame: field("reference_frame"),
            layer: field("layer"),
            frame: field("frame"),
            spot: match input.get("spot").and_then(|s| s.as_str()) {
                Some("new_frame") => LandSpot::NewFrame,
                _ => LandSpot::ActiveCel,
            },
            duration_ms: input
                .get("duration_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(83) as u32,
            opts,
        })
    }
}

/// 生成图落点与量化统计，回给主循环拼摘要、刷新画布。
#[derive(Debug, Clone)]
pub struct GeneratedLand {
    pub layer: String,
    pub frame: String,
    pub report: pixelize::PixelizeReport,
}

/// 把一张生成的位图落到文档上。ActiveCel 直接覆盖目标 cel；
/// NewFrame 先插一帧再落。返回落点与新帧 id，供上层挪激活帧。
pub fn land_generated(
    doc: &mut Document,
    active: &ActiveContext,
    params: &ImageGenToolParams,
    rgba: &[u8],
    width: u32,
    height: u32,
) -> Result<GeneratedLand, String> {
    let layer = params.layer.clone().unwrap_or_else(|| active.layer.clone());
    let frame = match params.spot {
        LandSpot::ActiveCel => params.frame.clone().unwrap_or_else(|| active.frame.clone()),
        LandSpot::NewFrame => {
            let anchor = params.frame.clone().unwrap_or_else(|| active.frame.clone());
            let position = doc
                .frames
                .iter()
                .position(|f| f.id == anchor)
                .ok_or_else(|| format!("unknown frame: {anchor}"))?;
            ops::apply_batch(
                doc,
                &[PixelOperation::CreateFrame {
                    after: Some(anchor),
                    duration_ms: params.duration_ms.clamp(1, 60_000),
                    id: None,
                }],
            )
            .map_err(|e| e.to_string())?;
            // CreateFrame 把新帧插在锚点之后，所以落点就是 position + 1。
            doc.frames
                .get(position + 1)
                .ok_or("the new frame did not land after the anchor")?
                .id
                .clone()
        }
    };
    // ActiveCel 覆盖前先确认 cel 存在，错误里直接点名，让模型改对帧 id。
    if params.spot == LandSpot::ActiveCel && doc.cel(&layer, &frame).is_none() {
        return Err(format!("unknown cel: {layer}/{frame}"));
    }
    let report =
        pixelize::pixelize_into_cel(doc, &layer, &frame, rgba, width, height, &params.opts)?;
    Ok(GeneratedLand {
        layer,
        frame,
        report,
    })
}

/// 激活 cel 的 RLE 网格（附到工具结果里，让模型「读一次」验证）。
pub fn active_grid(doc: &Document, layer: &str, frame: &str) -> String {
    match context::cel_context(doc, layer, frame, TOOL_GRID_CHARS) {
        Ok(s) => s,
        Err(e) => format!("(could not render active grid: {e})"),
    }
}

fn format_view(view: &CanvasView) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "window {}x{} at ({},{}){}\n",
        view.width,
        view.height,
        view.x,
        view.y,
        if view.shrunk {
            " [downsized to fit budget]"
        } else {
            ""
        }
    ));
    out.push_str("legend:\n");
    for line in view.legend.to_lines() {
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str("rows:\n");
    out.push_str(&view.rows.join("\n"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixel_core::ops::PixelCell;
    use pixel_core::png;

    fn active() -> ActiveContext {
        ActiveContext::default()
    }

    /// 两帧不同内容的文档：F0 是左上角一块，F1 是右下角一块。
    fn two_pose_doc() -> Document {
        let mut doc = Document::new("test", 16, 16).expect("16x16");
        let ops = vec![
            PixelOperation::AddPaletteColors {
                colors: vec!["#FF004D".into(), "#29ADFF".into()],
            },
            PixelOperation::SetPixels {
                layer: "L0".into(),
                frame: "F0".into(),
                cells: (0..8)
                    .map(|x| PixelCell {
                        x,
                        y: 0,
                        color: "#FF004D".into(),
                    })
                    .collect(),
            },
            PixelOperation::CreateFrame {
                after: None,
                duration_ms: 83,
                id: None,
            },
            PixelOperation::SetPixels {
                layer: "L0".into(),
                frame: "F1".into(),
                cells: (0..8)
                    .map(|x| PixelCell {
                        x,
                        y: 15,
                        color: "#29ADFF".into(),
                    })
                    .collect(),
            },
        ];
        ops::apply_batch(&mut doc, &ops).expect("setup applies");
        doc
    }

    #[test]
    fn tween_tool_inserts_frames_between_the_two_poses() {
        let mut doc = two_pose_doc();
        let out = execute(
            &mut doc,
            &active(),
            "pixel_tween_frames",
            &json!({"from_frame": "F0", "to_frame": "F1", "count": 3}),
        );
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(doc.frames.len(), 5, "3 tweened between F0 and F1");
        assert!(
            out.content.contains("3 frame(s) inserted"),
            "{}",
            out.content
        );
        // 中间帧必须插在 F0 之后、F1 之前，F1 仍是最后一帧。
        assert_eq!(doc.frames[0].id, "F0");
        assert_eq!(doc.frames[4].id, "F1");
    }

    #[test]
    fn tween_tool_names_the_missing_endpoints() {
        let mut doc = two_pose_doc();
        let out = execute(
            &mut doc,
            &active(),
            "pixel_tween_frames",
            &json!({"to_frame": "F1", "count": 2}),
        );
        assert!(out.is_error);
        assert!(out.content.contains("from_frame"), "{}", out.content);

        let out = execute(
            &mut doc,
            &active(),
            "pixel_tween_frames",
            &json!({"from_frame": "F0", "to_frame": "F9", "count": 2}),
        );
        assert!(out.is_error);
        assert!(
            out.content.contains("unknown layer/frame"),
            "{}",
            out.content
        );
        assert!(out.content.contains("F9"), "{}", out.content);
    }

    #[test]
    fn tween_tool_leaves_the_document_alone_when_the_request_is_bad() {
        let mut doc = two_pose_doc();
        let before = serde_json::to_value(&doc).expect("serializable");
        let out = execute(
            &mut doc,
            &active(),
            "pixel_tween_frames",
            &json!({"from_frame": "F0", "to_frame": "F1", "count": 0}),
        );
        assert!(out.is_error);
        assert_eq!(
            before,
            serde_json::to_value(&doc).expect("serializable"),
            "a rejected tween must not mutate the document"
        );
    }

    /// 造一张真 PNG 当「生图模型产出」，验证 base64 路径能真的落到网格上。
    fn painted_png_base64() -> String {
        let mut src = Document::new("src", 16, 16).expect("16x16");
        let ops = vec![
            PixelOperation::AddPaletteColors {
                colors: vec!["#FF004D".into(), "#29ADFF".into()],
            },
            PixelOperation::SetPixels {
                layer: "L0".into(),
                frame: "F0".into(),
                cells: (0..16)
                    .flat_map(|y| (0..16).map(move |x| (x, y)))
                    .map(|(x, y)| PixelCell {
                        x,
                        y,
                        color: if (x + y) % 2 == 0 {
                            "#FF004D"
                        } else {
                            "#29ADFF"
                        }
                        .into(),
                    })
                    .collect(),
            },
        ];
        ops::apply_batch(&mut src, &ops).expect("setup applies");
        png::base64_encode(&png::document_to_png(&src).expect("encodes"))
    }

    #[test]
    fn pixelize_tool_lands_a_bitmap_on_the_grid() {
        let mut doc = Document::new("test", 16, 16).expect("16x16");
        let out = execute(
            &mut doc,
            &active(),
            "pixel_pixelize_image",
            &json!({
                "image_base64": painted_png_base64(),
                "media_type": "image/png",
                "max_colors": 4,
            }),
        );
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("256 opaque"), "{}", out.content);
        // 16x16 全画：contain 模式下源图与画布同尺寸，dest 覆盖整幅。
        let cel = doc.cel("L0", "F0").expect("cel exists");
        assert!(
            cel.indices.iter().any(|&i| i != 0),
            "some pixels are opaque"
        );
    }

    #[test]
    fn pixelize_tool_accepts_a_data_url_prefix() {
        let mut doc = Document::new("test", 16, 16).expect("16x16");
        let out = execute(
            &mut doc,
            &active(),
            "pixel_pixelize_image",
            &json!({"image_base64": format!("data:image/png;base64,{}", painted_png_base64())}),
        );
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("contain"), "{}", out.content);
    }

    #[test]
    fn pixelize_tool_demands_a_media_type_only_for_bare_base64() {
        let mut doc = Document::new("test", 16, 16).expect("16x16");
        let out = execute(
            &mut doc,
            &active(),
            "pixel_pixelize_image",
            &json!({"image_base64": painted_png_base64()}),
        );
        assert!(out.is_error);
        assert!(out.content.contains("media_type"), "{}", out.content);

        let out = execute(
            &mut doc,
            &active(),
            "pixel_pixelize_image",
            &json!({"image_base64": "   ", "media_type": "image/png"}),
        );
        assert!(out.is_error);
        assert!(out.content.contains("empty image data"), "{}", out.content);
    }

    #[test]
    fn pixelize_tool_reports_a_broken_payload_instead_of_guessing() {
        let mut doc = Document::new("test", 16, 16).expect("16x16");
        let out = execute(
            &mut doc,
            &active(),
            "pixel_pixelize_image",
            &json!({"image_base64": "data:image/png;base64,bm90LWEtcG5n", "media_type": "image/png"}),
        );
        assert!(out.is_error, "{}", out.content);
        assert!(
            out.content.contains("pixel_pixelize_image"),
            "{}",
            out.content
        );
    }

    #[test]
    fn pixelize_tool_names_the_cel_it_cannot_find() {
        let mut doc = Document::new("test", 16, 16).expect("16x16");
        let out = execute(
            &mut doc,
            &active(),
            "pixel_pixelize_image",
            &json!({"image_base64": painted_png_base64(), "frame": "F9"}),
        );
        assert!(out.is_error);
        assert!(out.content.contains("unknown cel"), "{}", out.content);
    }

    #[test]
    fn duplicate_frame_copies_every_layer_cel_and_lands_after_the_source() {
        let mut doc = two_pose_doc();
        let out = execute(
            &mut doc,
            &active(),
            "pixel_apply_operations",
            &json!({"operations": [{"op": "duplicate_frame", "id": "F0"}]}),
        );
        assert!(!out.is_error, "{}", out.content);
        // 新帧紧跟源帧，原有顺序不受影响。
        assert_eq!(doc.frames.len(), 3);
        assert_eq!(doc.frames[1].id, "F2");
        assert_eq!(doc.frames[2].id, "F1");
        assert_eq!(
            doc.frames[1].duration_ms, doc.frames[0].duration_ms,
            "时长跟着源帧"
        );
        let source = &doc.cels["L0"]["F0"];
        let copy = &doc.cels["L0"]["F2"];
        assert_eq!(copy.indices, source.indices, "复制必须连内容一起");
        assert_eq!(copy.indices[0], 1, "F0 左上角是调色板索引 1");
    }

    #[test]
    fn an_unknown_tool_is_reported_back_rather_than_silently_succeeding() {
        let mut doc = Document::new("test", 16, 16).expect("16x16");
        let out = execute(&mut doc, &active(), "pixel_teleport", &json!({}));
        assert!(out.is_error);
        assert!(out.content.contains("unknown tool"), "{}", out.content);
        assert!(out.content.contains("pixel_teleport"), "{}", out.content);
    }

    #[test]
    fn every_spec_names_a_tool_the_dispatcher_actually_handles() {
        // 规格与分发器必须一致，否则模型看到的能力有一样是幽灵。
        for spec in specs() {
            assert_eq!(
                spec.schema.get("type").and_then(|t| t.as_str()),
                Some("object"),
                "{} schema is not an object",
                spec.name
            );
            assert!(
                !spec.description.is_empty(),
                "{} has no description",
                spec.name
            );
            let handled = matches!(
                spec.name.as_ref(),
                "pixel_apply_operations"
                    | "pixel_read_canvas"
                    | "pixel_run_shader"
                    | "pixel_tween_frames"
                    | "pixel_pixelize_image"
                    // 生图由 runner 异步分流，execute 里只有兜底分支，规格仍归这里发。
                    | IMAGE_GEN_TOOL
            );
            assert!(handled, "{} is described but not dispatched", spec.name);
        }
        assert_eq!(specs().len(), 6);
    }

    #[test]
    fn generated_image_params_demand_a_prompt_and_read_options() {
        assert!(ImageGenToolParams::parse(&json!({})).is_err());
        assert!(ImageGenToolParams::parse(&json!({"prompt": "   "})).is_err());
        let p = ImageGenToolParams::parse(&json!({
            "prompt": "a green slime",
            "reference_frame": "F1",
            "spot": "new_frame",
            "max_colors": 16,
            "dither": true,
            "duration_ms": 120,
        }))
        .expect("parses");
        assert_eq!(p.prompt, "a green slime");
        assert_eq!(p.reference_frame.as_deref(), Some("F1"));
        assert_eq!(p.spot, LandSpot::NewFrame);
        assert_eq!(p.opts.max_colors, 16);
        assert!(p.opts.dither);
        assert_eq!(p.duration_ms, 120);
        // 空串一律当成没给，不让模型用 "" 占位。
        let blank = ImageGenToolParams::parse(
            &json!({"prompt": "x", "reference_frame": "", "layer": "  "}),
        )
        .expect("parses");
        assert!(blank.reference_frame.is_none());
        assert!(blank.layer.is_none());
    }

    /// 一张 2x2 四色 RGBA，直接喂 land_generated（绕开 base64）。
    fn quad_rgba() -> Vec<u8> {
        vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
        ]
    }

    #[test]
    fn generated_image_lands_on_the_active_cel_without_adding_a_frame() {
        let mut doc = Document::new("test", 16, 16).expect("16x16");
        let params = ImageGenToolParams::parse(&json!({"prompt": "a red slime"})).expect("parses");
        let land = land_generated(&mut doc, &active(), &params, &quad_rgba(), 2, 2).expect("lands");
        assert_eq!(land.layer, "L0");
        assert_eq!(land.frame, "F0");
        assert_eq!(doc.frames.len(), 1, "active_cel must not spawn a frame");
        let cel = doc.cel("L0", "F0").expect("cel exists");
        assert!(cel.indices.iter().any(|&i| i != 0), "some pixels painted");
    }

    #[test]
    fn generated_image_can_grow_a_new_frame_after_the_anchor() {
        let mut doc = Document::new("test", 16, 16).expect("16x16");
        let params = ImageGenToolParams::parse(&json!({
            "prompt": "one more pose",
            "spot": "new_frame",
            "duration_ms": 120,
        }))
        .expect("parses");
        let land = land_generated(&mut doc, &active(), &params, &quad_rgba(), 2, 2).expect("lands");
        assert_eq!(land.frame, "F1");
        assert_eq!(doc.frames.len(), 2);
        assert_eq!(doc.frames[1].id, "F1");
        assert_eq!(
            doc.frames[1].duration_ms, 120,
            "duration stamped on the new frame"
        );
    }

    #[test]
    fn land_generated_names_a_cel_it_cannot_find() {
        let mut doc = Document::new("test", 16, 16).expect("16x16");
        let params =
            ImageGenToolParams::parse(&json!({"prompt": "x", "frame": "F9"})).expect("parses");
        let e = land_generated(&mut doc, &active(), &params, &quad_rgba(), 1, 1)
            .expect_err("F9 does not exist");
        assert!(e.contains("F9"), "{}", e);
    }
}
