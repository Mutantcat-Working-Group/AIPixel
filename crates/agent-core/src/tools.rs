//! 三个落地工具的封装，把模型入参翻译成 pixel-core 的类型化操作。
//! 「不让模型手写矩阵」的契约就在这一层收口。

use super::models::{ActiveContext, ToolSpec};
use pixel_core::context;
use pixel_core::document::Document;
use pixel_core::ops::{self, PixelOperation};
use pixel_core::rle::{self, CanvasView};
use pixel_core::shader::{self, ShaderBudget};
use serde_json::{json, Value};

/// 工具回填给模型的 RLE 网格字符预算。
const TOOL_GRID_CHARS: usize = 3000;

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
            name: "pixel_apply_operations",
            description: "Apply ONE transaction of typed pixel/layer/frame/palette operations. Structure ops (create/move/delete/rename layers and frames, set duration, add palette colors) and tiny precise pixel patches (set_pixels, stamp_grid, draw_shape, bucket_fill, clear_region). Fails atomically if any operation is invalid; the error names the failing operation index.",
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
                                    "enum": ["create_frame","delete_frame","move_frame","set_frame_duration","create_layer","delete_layer","move_layer","rename_layer","set_layer_properties","add_palette_colors","set_pixels","bucket_fill","draw_shape","clear_region","stamp_grid"]
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
            name: "pixel_read_canvas",
            description: "Read the current canvas as an RLE grid. Pass {\"overview\": true} for a downsampled whole-canvas map on large canvases, or {\"region\": {x,y,width,height}} (up to 128x128) for an exact window. With no arguments it returns the active-layer window.",
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
            name: "pixel_run_shader",
            description: "Draw by running ONE sandboxed Lua script whose size is independent of the canvas. Supports pset/line/rect/ellipse/circle/flood/stamp/replace/outline/clear and pal/mix/hsv/alpha/hex/noise/rand. Pass animate=true to render every existing frame driven by phase/time. Use this for all artwork.",
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

/// 激活 cel 的 RLE 网格（附到工具结果里，让模型「读一次」验证）。
fn active_grid(doc: &Document, layer: &str, frame: &str) -> String {
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
