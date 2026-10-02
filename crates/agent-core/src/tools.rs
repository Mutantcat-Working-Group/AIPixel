// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 六个工具的封装，把模型入参翻译成 pixel-core 的类型化操作。
//! 「不让模型手写矩阵」的契约就在这一层收口。
//!
//! 六个的分工：结构 ops（文档结构 + 微补丁）、read_canvas（回读）、
//! run_shader（一切美术，沙箱 Lua）、tween_frames（本机插帧）、
//! pixelize_image（垫图量化）。生图（pixel_generate_image）在 specs 里
//! 挂个名供模型发现，执行走 runner 的异步链路，不经 execute。

use super::craft;
use super::imagegen::LandSpot;
use super::models::{ActiveContext, ToolSpec};
use super::plan::{self, PLAN_TOOL};
use pixel_core::context;
use pixel_core::document::Document;
use pixel_core::ops::{self, PixelOperation};
use pixel_core::rle::{self, CanvasView};
use pixel_core::shader::{self, ShaderBudget};
use serde_json::{json, Value};

use pixel_core::decode;
use pixel_core::pixelize::{self, FitMode, PixelizeOptions};
use pixel_core::tween::{self, MigrateOrder, TweenMode, TweenOptions};

/// `pixel_run_shader` 的描述前半：全局量、颜色助手、画布 API、预算。
/// 之所以写这么长：模型拿到的就是这一段，写得太省，
/// 它只能猜 `rect` 是角对角还是 x/y/宽/高、猜 `circle` 的参数顺序，
/// 猜错一次就白烧一整个输出预算。下面每个签名都跟 runtime 对齐。
const RUN_SHADER_DESCRIPTION_BODY: &str = r##"PREFER THIS TOOL for freeform artwork - shapes, characters, scenes, patterns, textures, symmetry, gradients. Draw by running ONE sandboxed Lua script: its size stays small no matter how big the canvas is, because loops, noise and interpolation run in the runtime instead of being spelled out as pixel arrays. Draws on the active cel by default; pass `layer` to target another layer.

GLOBALS: width, height (canvas size; canvas_w / canvas_h are accepted aliases and canvas.width / canvas.height also work), time (seconds at this frame's start), phase (0..1 across the timeline), frame_index, frame_count, layer. Animate by passing animate=true: the runtime runs the script once per timeline frame with time/phase set for that frame, exactly like a shader time uniform, and writes every frame for you.
CANVAS SIZE HELPERS: canvas.cx / canvas.cy -> the exact canvas centre (width/height over two, never eyeballed), canvas.min / canvas.max -> the shorter and the longer side, canvas.scale(k) -> k times the long-side-over-64 ratio so a constant tuned at 64px keeps its proportions on any canvas (scale(12) is a 12px body at 64 and 24px at 128), canvas.grid(cols, rows) -> integer cell width, height of a cols x rows grid, which is the honest way to centre a subject in a tile-map cell, an isometric cell or a nine-slice panel. Derive every radius and offset from these; a hard-coded size is what leaves a drawing half empty on a canvas it was not tuned for.

COLOR HELPERS: pal(i) -> palette color (1-based, 0 or nil = transparent), hex('#RRGGBB[AA]') -> validated color, mix(c1, c2, t) -> blend (t in 0..1, gradients and glows), hsv(h, s, v[, a]) -> color (h in degrees, s/v/a in 0..1), alpha(c, a) -> color with new alpha, rand() -> float 0..1, rand(a, b) -> integer in a..b inclusive, noise(x, y, scale?) -> float 0..1 (scale spreads the lattice out: 0.3 gives long soft streaks, 1 gives per-pixel grit). Colors are plain Lua values: keep them in locals, pass them around, format with string.format. Standard math.* and string.* are available; math.random is disabled, use rand(). rand and noise are deterministic per script and per frame.

CANVAS API (each of these also works as canvas.pset, canvas.line, ... - both forms run):
  pset(x, y, color)          single pixel; OUT OF BOUNDS IS AN ERROR, so clamp x to 0..width-1 and y to 0..height-1; nil erases
  pget(x, y)                 -> "#RRGGBB" or "transparent"
  line(x0, y0, x1, y1, color)
  rect(x0, y0, x1, y1, color[, filled])        two CORNERS, inclusive (rect(4, 6, 11, 18, c) traces x=4..11, y=6..18; to size by width/height pass x0, y0, x0+w, y0+h)
  rectfill(x0, y0, x1, y1, color)
  ellipse(x0, y0, x1, y1, color[, filled])     BOUNDING-BOX CORNERS, not center+radius (ellipse(10, 10, 30, 40, c) fills exactly x=10..30, y=10..40; if you are thinking in center+radii, write ellipse(cx-rx, cy-ry, cx+rx, cy+ry, c) or use circle)
  ellipfill(x0, y0, x1, y1, color)
  circle(cx, cy, r, color[, filled])           CENTER + RADIUS
  circfill(cx, cy, r, color)
  flood(x, y, color)         fill the contiguous same-color area
  replace(from, to)          swap one color for another across the whole cel
  outline(color)             1px outline around every existing non-transparent pixel; call it LAST, after the shapes
  clear()                     erase the cel; clear(color) fills it with one solid background color
  stamp(rows, legend, x, y)  rows are strings of legend symbols, '.' and space are transparent (they ERASE), legend maps symbol -> color; all rows must share one length. Because '.' erases, stamping over art that is already on the cel punches holes in it - open with clear() when you want a clean slate.
Shapes clip to the canvas, so a circle larger than the canvas is safe. color is a palette index or "#RRGGBB"/"#RRGGBBAA"; alpha 00 or nil erases. pset does not clip, so pset outside the canvas is a hard error.

AA* ANTI-ALIASED AND CURVED - the SAME coordinates as the hard-edged twin of each name, but drawn at partial coverage, so the line blends with whatever is already on the cel instead of stair-stepping. Lay flat base passes with the hard-edged helpers and spend these on the detail layer:
  aaline(x0,y0,x1,y1,c)   alias aaseg                aacurve(x0,y0,x1,y1,cx,cy,c)   alias aaquad (quadratic through the control point)
  aacubic(x0,y0,x1,y1,cx0,cy0,cx1,cy1,c)   alias aabez
  aapoly(points,c[,closed])  alias aapath            aapolyfill(points,c)   alias aafill
      points is a table of {x=,y=} pairs (a flat x,y,x,y list works too); a triangle is just three points
  aacircle(cx,cy,r,c[,filled])  alias aacirc          aaellipse(x0,y0,x1,y1,c[,filled]) (BOUNDING-BOX CORNERS, like ellipse)
  aarect(x0,y0,x1,y1,c[,filled])
  blend(x,y,c,a)    ONE pixel already placed, softened at alpha a     dither(x,y,c,a)    ONE pixel from an ordered pattern at ratio a
On a layer with a locked palette every blended colour snaps back into the range, so a soft edge never widens the palette.

BUDGETS: ~20M Lua instructions, 5 seconds, one changed-pixel cap. When the script has a mistake the error names the exact line - fix that line and resubmit; never resubmit the same script unchanged.

SHADING RECIPE - copy this and scale the numbers to your canvas; do not re-invent it. The two mistakes that flatten a drawing are (1) dropping a second filled shape on top of a finished body, which erases the first one's detail, and (2) mixing the shadow toward '#000000' and the highlight toward '#FFFFFF', which reads as dirt instead of shade. The recipe does neither: build ONE hue-shifted ramp, lay a single flat base pass in its MIDDLE step, then re-shade the body pixel by pixel from its own form normal with a pget test. Scale the ramp length with the canvas (3 steps under 32px, 5 above) and scale the band thresholds to taste; the pattern is what matters.
"##;

/// 描述后半：两个示例。手写行那份保留原样——它讲的是另一条路径（小图手写），
/// 跟上色配方各管一头，互不替代。
const RUN_SHADER_EXAMPLES: &str = r##"EXAMPLE - hand-written rows for ONE small sprite, for when there is no loop to write: shorter than the equivalent script and every pixel placed deliberately. All rows must share one length, so pad the right of each row with '.'.
  stamp({
      '.hd.xx..',
      'hhxxxxx.',
      'xxxxxxxx',
      'xxxxxxxx',
      '.xxxxxx.',
      '..xxxx..',
      '...xx...',
      '........',
  }, {h='#FF6B6B', d='#B32D2D', x='#E23B3B'}, 0, 0)

EXAMPLE - animate=true bob (phase drives everything; create the frames with pixel_apply_operations first):
  local ink = pal(1)
  local bob = math.floor(1 + math.sin(phase * math.pi * 2))
  canvas.clear(nil)
  circfill(16, 30 + bob, 5, ink)
  circle(16, 30 + bob, 8, pal(2))

Write ONE script per transaction: draw, then let the tool result show you the updated grid."##;

/// 拼好的 `pixel_run_shader` 描述：主体 + 老配方 + 新配方说明 + 新配方 + 示例。
/// 分段拼而不是一段常量，是因为 `format!` 的 `{}` 替代位和 Lua 示例里满地的
/// 花括号打架；各段各自原样拼起来就不用转义。两段配方都来自 pixel-core，由
/// pixel-core 自己的测试保证它们在真实沙箱里跑得通，那边改了 API 这边会跟着知道。
static RUN_SHADER_DESCRIPTION: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        "{body}\n{recipe}\n{note}\n{form}\n\n{examples}",
        body = RUN_SHADER_DESCRIPTION_BODY,
        recipe = pixel_core::SHADING_RECIPE,
        note = FORM_SHADING_NOTE,
        form = pixel_core::FORM_SHADING_RECIPE,
        examples = RUN_SHADER_EXAMPLES,
    )
});

/// 第二份上色配方的说明段，夹在老配方和新配方代码之间。它和 `pixel-core` 的
/// `FORM_SHADING_RECIPE` 一起拼进描述里，那边由沙箱测试保证跑得通、球体上
/// 与老配方逐像素一致。写清楚「什么时候换用它」比多写一条禁令有用：躯干、
/// 树干、瓦片、矩形尾巴这些正是老配方那句「报出圆心和半径」办不到的形状。
const FORM_SHADING_NOTE: &str = r#"FORM SHADING RECIPE (any contour) - use this instead of SHADING RECIPE above for ANY form that is not a round body - a cat torso, a tree trunk, a tile, a rectangular tail - because the normal is read off the pixels instead of a guessed centre and radius. The discipline is the same (one hue-shifted ramp, ONE flat base pass in the middle step, then band the body pixel by pixel with a pget test), but pass one scans the rows for the base colour and records each row's left/right edge plus the body's top/bottom, and pass two bands every base-coloured pixel by that normal against the same top-left light. On a sphere it gives the same steps as SHADING RECIPE above; on a flat or composite form it still gets layers, which a guessed centre cannot. Pixels drawn in any OTHER colour are outside the scan, so stripes, eyes and whiskers layered on top of the base pass survive. Shade ONE flat base region per run, then run it again with the next base colour, and scale the ramp length to the canvas the same way (3 steps under 32px, 5 above).
"#;

/// `pixel_apply_operations` 的工具描述：一条事务，加上每个 op 的字段说明。
/// 名字列在 enum 里，字段却要猜，是这个工具最容易翻车的地方。
const OPS_DESCRIPTION: &str = r##"Apply ONE transaction of typed pixel/layer/frame/palette operations. Use it for document STRUCTURE and for tiny precise patches; use pixel_run_shader for everything that is artwork. Batch every structural change of a turn into this ONE call. Fails atomically if any operation is invalid and the error names the failing operation index.

STRUCTURE:
  create_layer {name, after?, palette_id?, locked?}        append (or insert after `after`) a layer
  delete_layer {layer}                                     move_layer {layer, to_index}
  rename_layer {layer, name}                               set_layer_properties {layer, visible?, opacity?}
  create_frame {after?, duration_ms?}                     duplicate_frame {frame, after?}
  delete_frame {frame}                                     move_frame {frame, to_index}
  set_frame_duration {frame, duration_ms}                  the default pace is ~83ms for a 12 FPS loop

COLOR RANGES: every layer points at exactly one named palette.
  create_palette {name, from?, colors[], layer?}           `from` copies an existing range as the starting point
  rename_palette {id, name}    delete_palette {id, fallback?}
  while a layer still points at a range, deleting it needs `fallback` (another range id): those layers move over first, then the range goes. Omitting it keeps the refusal.
  add_palette_color {id, color}  remove_palette_color {id, index, replacement?}
  when painted pixels use the color being removed, name a `replacement` color and those pixels are repainted with it before the color goes.
  set_layer_palette {layer, palette_id}  set_layer_locked {layer, locked}
  add_palette_colors {colors[]} extends the document palette itself; set_palette {colors[]} replaces it and remaps already-painted pixels to the nearest color.
  Builtin ranges are read-only: to change one, create_palette with from=<builtin id> to fork it, then set_layer_palette.
  locked = true restricts that layer to its range and snaps out-of-range colors to the nearest color inside it; locked = false lets the layer use any color.

PIXEL PATCHES (document structure, plus the placement half of a hand-written sprite):
  set_pixels {layer?, frame?, cells: [{x, y, color}]}      sparse pixel list
  stamp_grid {layer?, frame?, rows: [string], legend: {sym: color}, x, y}   every row must share one length, '.' is transparent
  draw_shape {layer?, frame?, shape: line|rect|ellipse, x0, y0, x1, y1, color, filled?}
  bucket_fill {layer?, frame?, x, y, color}           clear_region {layer?, frame?, x, y, w, h}
Coordinates are 0-based from the top-left; colors are "#RRGGBB" or "#RRGGBBAA" (null clears).

Use pixel_run_shader to create or change artwork. Its body is paths and math when the artwork has symmetry, repetition, a cycle or a canvas at 48px and above, and hand-written rows through stamp() when it is one small sprite at 47px and below with no loop to write; stamp_grid here is the same hand-written-row route for a single static frame. To animate, create the frames here first, then run the shader with animate=true."##;

/// 工具回填给模型的 RLE 网格字符预算。
const TOOL_GRID_CHARS: usize = 3000;

/// agent 生图工具名。同步工具走 `tools::execute`；这一个要等模型回图，
/// 由 runner 分流到异步路径，所以名字单独抽出来给规格、execute 兜底、runner 共用。
pub const IMAGE_GEN_TOOL: &str = "pixel_generate_image";

/// 着色器工具名。抽出来是为了 runner 的重放护栏：同一份 script 原样重跑
/// 要认得这个名字，字面量散在各处迟早对不上。
pub const SHADER_TOOL: &str = "pixel_run_shader";

/// 事务绘制工具名。往画布上落 pixels 的主路径：建帧、建图层、调色板增删这些
/// 搭台动作和真正的画像素全走这一个工具，所以抽出来给提示词硬闸认名字。
pub const APPLY_TOOL: &str = "pixel_apply_operations";

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

/// 脚本错时塞给模型的处置要求。不写这句，模型常把脚本错当成「画布坏了」，
/// 干脆认输回头跟用户说画不了，白白浪费一轮。
const SHADER_SCRIPT_HINT: &str = "\nFix that exact spot and call pixel_run_shader again with the corrected script; never resubmit the same broken script unchanged. Do not tell the user the drawing failed until you have retried at least once with a different script. When the message names a coordinate, clamp it instead of guessing: math.max(0, math.min(width - 1, x)) keeps every x inside the canvas, and an empty loop body (a lower bound below the upper bound) is a coordinate bug, not a harmless no-op.";

/// 预算烧穿：脚本跑不完，不是画不出来。把活儿拆小才是正路。
const SHADER_BUDGET_HINT: &str = "\nThe script ran out of time or instructions, so nothing was kept. Shrink the work: fewer per-pixel iterations, drop the anti-alias pass, or split the drawing into two scripts (background first, then the subject), then resubmit.";

/// 六个工具的对外规格（协议无关，schema 为 JSON Schema）。
/// 顺序即推荐度：模型挑花眼时，靠前的就是引擎更希望它用的那条。
pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "pixel_apply_operations".into(),
            description: OPS_DESCRIPTION.into(),
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
                                    "enum": ["create_frame","delete_frame","duplicate_frame","move_frame","set_frame_duration","create_layer","delete_layer","move_layer","rename_layer","set_layer_properties","add_palette_colors","set_palette","create_palette","delete_palette","rename_palette","add_palette_color","remove_palette_color","set_layer_palette","set_layer_locked","set_pixels","bucket_fill","draw_shape","clear_region","stamp_grid"]
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
                                "palette_id": {"type": "string", "description": "named color range id, e.g. sweetie16"},
                                "from": {"type": "string", "description": "create_palette: copy this range as the starting point"},
                                "index": {"type": "integer"},
                                "locked": {"type": "boolean", "description": "true = this layer may only use colors inside its range"},
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
            description: RUN_SHADER_DESCRIPTION.clone().into(),
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
        plan::spec(),
        craft::spec(),
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
        // 同理：本轮分流归 runner，它手里握着这一轮的附件清单和分流表，
        // 工具层看不见。落到这里只说明有人绕过了 runner 直接调 execute。
        PLAN_TOOL => err(format!(
            "{PLAN_TOOL} only runs in the agent loop, where the turn's attachments and routing live"
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
        let mut patched = v.clone();
        fill_pixel_target(&mut patched, active);
        // 错误里必须带上 op 名：一整批操作里第几条、是哪一种操作翻的车，
        // 少了名字模型只能靠猜，猜三遍还是同一处（见配色范围那组）。
        let op_name = patched
            .get("op")
            .and_then(|o| o.as_str())
            .unwrap_or("?")
            .to_string();
        match serde_json::from_value::<PixelOperation>(patched) {
            Ok(op) => ops.push(op),
            Err(e) => {
                return err(format!(
                    "operation[{i}] {op_name} invalid: {e}{}",
                    op_field_hint(&op_name)
                ))
            }
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

/// 反序列化失败时补的一句字段提示。配色范围那一组的 id 字段在 schema 里
/// 别处叫 palette_id，是模型最常写错的一处，单独说清楚。
fn op_field_hint(op: &str) -> &'static str {
    match op {
        "delete_palette" | "rename_palette" | "add_palette_color" | "remove_palette_color" => {
            " (the color-range id field is `id`, and `palette_id` is accepted too)"
        }
        "delete_frame" | "duplicate_frame" | "move_frame" | "set_frame_duration" => {
            " (the frame field is `id`, and `frame` is accepted too)"
        }
        _ => "",
    }
}

/// 像素五件套允许省掉 `layer`/`frame`：省了就落到当前活跃层帧上。
/// 模型少发两个字段比发错两个字段便宜——一次缺省能省掉一整轮红字来回。
fn fill_pixel_target(v: &mut Value, active: &ActiveContext) {
    let Some(obj) = v.as_object_mut() else { return };
    let is_pixel = matches!(
        obj.get("op").and_then(|o| o.as_str()),
        Some("set_pixels")
            | Some("bucket_fill")
            | Some("draw_shape")
            | Some("clear_region")
            | Some("stamp_grid")
    );
    if !is_pixel {
        return;
    }
    for key in ["layer", "frame"] {
        let slot = obj.entry(key).or_insert(Value::Null);
        // 空串、纯空白和 null 一律当没发（和参数解析那边的口径一致），
        // 真发了值就让位给模型自己指定的那个。
        let blank = slot.as_str().map(str::trim).unwrap_or_default().is_empty();
        if blank {
            *slot = Value::String(if key == "layer" {
                active.layer.clone()
            } else {
                active.frame.clone()
            });
        }
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
        // 模型手滑给个大数会从 u64 截断回绕成小坐标，画出来的窗口就错位了。
        // 先饱和到 u32，越界部分交给 render_window 去夹回画布。
        let sat = |v: u64| v.min(u32::MAX as u64) as u32;
        let x = region
            .get("x")
            .and_then(|v| v.as_u64())
            .map(sat)
            .unwrap_or(0);
        let y = region
            .get("y")
            .and_then(|v| v.as_u64())
            .map(sat)
            .unwrap_or(0);
        let w = region
            .get("width")
            .and_then(|v| v.as_u64())
            .map(sat)
            .unwrap_or(doc.width);
        let h = region
            .get("height")
            .and_then(|v| v.as_u64())
            .map(sat)
            .unwrap_or(doc.height);
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
        Err(e) => {
            let mut content = format!("shader error: {e}");
            match &e {
                // 行号点名到第几行，再把「改脚本再跑一次」说死：不补这两句，
                // 模型很可能把脚本错当成画布有问题，直接回头跟用户认输。
                shader::ShaderError::Lua { line, .. } => {
                    if let Some(line) = line {
                        content.push_str(&format!(
                            "\nfault in your script at line {line}; the canvas was rolled back, nothing partial was kept."
                        ));
                    } else {
                        content.push_str("\nfault in your script; the canvas was rolled back, nothing partial was kept.");
                    }
                    content.push_str(SHADER_SCRIPT_HINT);
                }
                shader::ShaderError::Budget(_) => content.push_str(SHADER_BUDGET_HINT),
                _ => content.push_str(SHADER_SCRIPT_HINT),
            }
            err(content)
        }
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
            .map(|v| v.clamp(1, 60_000) as u32)
            .unwrap_or(83),
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
        opts.alpha_threshold = v.clamp(0, 255) as u8;
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
            opts.alpha_threshold = v.clamp(0, 255) as u8;
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
                .map(|v| v.clamp(1, 60_000) as u32)
                .unwrap_or(83),
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

    /// 三段拼接出来的 `pixel_run_shader` 描述，任何一段掉了都要能发现。
    /// 这里守的不只是「拼上了」：示例一旦回到「第二个填充盖在身体上」和
    /// 「阴影往黑里掺」那套写法，模型会照着抄，成品就永远是平的。
    #[test]
    fn the_shader_description_carries_the_shading_recipe() {
        let spec = specs()
            .into_iter()
            .find(|s| s.name == "pixel_run_shader")
            .expect("pixel_run_shader 在规格表里");
        let text = spec.description.into_owned();
        assert!(text.contains("SHADING RECIPE"), "配方段没拼上");
        assert!(
            text.contains("circfill(32, 30, 11, R[3])"),
            "配方本体没拼上"
        );
        // 任意轮廓版配方：一类形状一份。少了它，模型在躯干、树干、矩形尾巴上
        // 只能回头猜圆心和半径，抄完还是平涂。
        assert!(text.contains("FORM SHADING RECIPE"), "任意轮廓配方段没拼上");
        assert!(
            text.contains("local top, bot, rows = nil, nil, {}"),
            "任意轮廓配方的扫描段没拼上"
        );
        // 反面示例不能回来：第二个填充盖上身体、阴影往纯黑里掺。
        assert!(
            !text.contains("mix(skin, '#000000'"),
            "阴影往纯黑里掺的示例又回来了"
        );
        assert!(
            !text.contains("circfill(34, 32, 8, shadow)"),
            "用第二个填充盖上身体的示例又回来了"
        );
        // 手写行路径是另一条腿，和配方各管一头，不能因为加配方被删掉。
        assert!(
            text.contains("EXAMPLE - hand-written rows"),
            "手写行示例没了"
        );
        assert!(text.contains("EXAMPLE - animate=true bob"), "动画示例没了");
    }

    /// 柔边这组接口必须在工具描述里点名。少一个名字，模型就少一条路，
    /// 只能继续用 line() 画台阶——用户看到的就是「三角形很突兀」。
    #[test]
    fn the_shader_description_documents_the_aa_family() {
        let spec = specs()
            .into_iter()
            .find(|s| s.name == "pixel_run_shader")
            .expect("pixel_run_shader 在规格表里");
        let text = spec.description.into_owned();
        for name in [
            "aaline",
            "aaseg",
            "aacurve",
            "aaquad",
            "aacubic",
            "aabez",
            "aapoly",
            "aapolyfill",
            "aacircle",
            "aaellipse",
            "aarect",
            "blend(",
            "dither(",
        ] {
            assert!(text.contains(name), "工具描述里没有 {name}");
        }
        // 配色锁那一句得在：模型要知道柔边不会把调色板撑宽。
        assert!(text.contains("locked palette"), "没说柔边在配色锁下的行为");
    }

    /// 尺寸适配助手在工具描述里也得逐个点到。
    /// 系统提示词和工具描述是两个入口，模型读哪一份都可能；只改一处，
    /// 它就还是会按 64x64 写死常量，换画布回来一半是空的。
    #[test]
    fn the_shader_description_documents_the_size_helpers() {
        let spec = specs()
            .into_iter()
            .find(|s| s.name == "pixel_run_shader")
            .expect("pixel_run_shader 在规格表里");
        let text = spec.description.into_owned();
        assert!(
            text.contains("CANVAS SIZE HELPERS"),
            "工具描述里缺尺寸适配这一节"
        );
        for helper in [
            "canvas.cx",
            "canvas.cy",
            "canvas.min",
            "canvas.max",
            "canvas.scale(",
            "canvas.grid(",
        ] {
            assert!(text.contains(helper), "工具描述里没有 {helper}");
        }
        // 比例按长边折算这点不能丢：写成「按比例」它会拿短边算，宽画布主体偏小。
        assert!(text.contains("long-side-over-64"), "没说 scale 按长边折算");
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
    fn tween_tool_clamps_runaway_frame_durations() {
        let mut doc = two_pose_doc();
        let out = execute(
            &mut doc,
            &active(),
            "pixel_tween_frames",
            &json!({
                "from_frame": "F0",
                "to_frame": "F1",
                "count": 1,
                "duration_ms": 9_999_999_999_u64,
            }),
        );
        assert!(!out.is_error, "{}", out.content);
        // 超大 u64 若直接 as u32 会回绕成小值，必须夹回 60000。
        assert_eq!(doc.frames[1].duration_ms, 60_000);
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

    /// 脚本报错回给模型的话里要带行号，还要带「改完再跑一次」的要求：
    /// 少了这两样，模型容易把脚本错当成画布坏了，直接认输不画。
    #[test]
    fn a_broken_shader_script_comes_back_with_its_line_and_repair_orders() {
        let mut doc = Document::new("test", 16, 16).expect("16x16");
        let out = execute(
            &mut doc,
            &active(),
            "pixel_run_shader",
            &json!({"script": "local a = 1\nnope(2)\n"}),
        );
        assert!(out.is_error, "{}", out.content);
        assert!(
            out.content.contains("line 2"),
            "行号要点名：{}",
            out.content
        );
        assert!(
            out.content.contains("call pixel_run_shader again"),
            "要说清改完再跑：{}",
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

    /// 像素五件套省掉 layer/frame：落到当前活跃层帧上，而不是报错。
    /// 模型少发两个字段不该换来一整轮红字来回。
    #[test]
    fn pixel_ops_fall_back_to_the_active_layer_and_frame() {
        let mut doc = two_pose_doc();
        let active = ActiveContext {
            layer: "L0".into(),
            frame: "F1".into(),
            color: None,
        };
        let untouched = doc.cel("L0", "F0").unwrap().indices.clone();
        let out = execute(
            &mut doc,
            &active,
            "pixel_apply_operations",
            &json!({"operations": [
                {"op": "set_pixels", "cells": [{"x": 3, "y": 3, "color": "#00E436"}]},
                {"op": "stamp_grid",
                 "rows": ["..x..", "..x.."],
                 "legend": {"x": "#FFA300"},
                 "x": 0, "y": 0}
            ]}),
        );
        assert!(!out.is_error, "{}", out.content);
        // 落点只能是活跃帧：F1 被改了两处，F0 一个像素都不许动。
        let landed = doc.cel("L0", "F1").unwrap();
        let green = doc
            .palette_index_of(pixel_core::document::Rgba::rgb(0x00, 0xE4, 0x36))
            .expect("颜色在档");
        assert_eq!(
            landed.indices[3 * doc.width as usize + 3],
            green,
            "省掉 layer/frame 的 set_pixels 落在活跃帧上"
        );
        let amber = doc
            .palette_index_of(pixel_core::document::Rgba::rgb(0xFF, 0xA3, 0x00))
            .expect("stamp 颜色在档");
        assert_eq!(landed.indices[2], amber, "stamp_grid 同样落在活跃帧");
        assert_eq!(
            doc.cel("L0", "F0").unwrap().indices,
            untouched,
            "别的帧不许被顺手改掉"
        );
    }

    /// 显式写出 layer/frame 时缺省逻辑必须让位：指错层还得报错，
    /// 不能默默改到活跃层上去。
    #[test]
    fn explicit_pixel_targets_are_not_overridden() {
        let mut doc = two_pose_doc();
        let out = execute(
            &mut doc,
            &active(),
            "pixel_apply_operations",
            &json!({"operations": [{"op": "set_pixels",
                "layer": "L9", "frame": "F0",
                "cells": [{"x": 0, "y": 0, "color": "#00E436"}]}]}),
        );
        assert!(out.is_error, "{}", out.content);
        assert!(out.content.contains("L9"), "{}", out.content);
    }

    /// 非像素操作不受缺省影响：改名少给目标照样报错，不蒙一个活跃层上去改。
    #[test]
    fn structural_ops_keep_requiring_their_target() {
        let mut doc = two_pose_doc();
        let out = execute(
            &mut doc,
            &active(),
            "pixel_apply_operations",
            &json!({"operations": [{"op": "rename_layer", "name": "改名"}]}),
        );
        assert!(out.is_error, "{}", out.content);
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
    fn read_canvas_region_clamps_huge_numbers_without_wrapping() {
        let mut doc = Document::new("test", 8, 8).expect("8x8");
        // 4294967297 = u32::MAX + 2。直接截断会变成 2，模型就会去画布左上角
        // 读一个错的窗口；饱和成 u32::MAX 后由 render_window 夹到右下角最后一格。
        let out = execute(
            &mut doc,
            &active(),
            "pixel_read_canvas",
            &json!({
                "region": {
                    "x": 4_294_967_297_u64,
                    "y": 4_294_967_297_u64,
                    "width": 4_294_967_297_u64,
                    "height": 4_294_967_297_u64,
                }
            }),
        );
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content.contains("window 1x1 at (7,7)"),
            "{}",
            out.content
        );
        // 正常范围内的窗口照旧按请求返回，不受饱和逻辑影响。
        let sane = execute(
            &mut doc,
            &active(),
            "pixel_read_canvas",
            &json!({"region": {"x": 6, "y": 6, "width": 5, "height": 5}}),
        );
        assert!(!sane.is_error, "{}", sane.content);
        assert!(
            sane.content.contains("window 2x2 at (6,6)"),
            "{}",
            sane.content
        );
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
                    // 本轮分流归 runner：它手里有这一轮的附件清单和分流表。
                    | PLAN_TOOL
                    // 提示词清单同样归 runner：它要把它绑到这一轮的每一次生图上。
                    | craft::PROMPT_TOOL
            );
            assert!(handled, "{} is described but not dispatched", spec.name);
        }
        assert_eq!(specs().len(), 8);
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
            "alpha_threshold": 300,
        }))
        .expect("parses");
        assert_eq!(p.prompt, "a green slime");
        assert_eq!(p.reference_frame.as_deref(), Some("F1"));
        assert_eq!(p.spot, LandSpot::NewFrame);
        assert_eq!(p.opts.max_colors, 16);
        assert!(p.opts.dither);
        assert_eq!(p.duration_ms, 120);
        // 越界 alpha_threshold 必须夹回 0..255，不能走 u8 回绕变成 44。
        assert_eq!(p.opts.alpha_threshold, 255);
        // 帧时长同理：u64 夹到 1..60000 后再转 u32，不能回绕。
        let big = ImageGenToolParams::parse(&json!({
            "prompt": "x",
            "duration_ms": 9_999_999_999_u64,
        }))
        .expect("parses");
        assert_eq!(big.duration_ms, 60_000);
        let tiny = ImageGenToolParams::parse(&json!({
            "prompt": "x",
            "duration_ms": 0,
        }))
        .expect("parses");
        assert_eq!(tiny.duration_ms, 1);
        // 空串一律当成没给，不让模型用 "" 占位。
        let blank = ImageGenToolParams::parse(
            &json!({"prompt": "x", "reference_frame": "", "layer": "  "}),
        )
        .expect("parses");
        assert!(blank.reference_frame.is_none());
        assert!(blank.layer.is_none());
    }

    /// 一张半透明混色的 PNG：alpha=100 的像素夹在阈值 44 和 255 之间，
    /// 回绕成 44 会把它们算成不透明，夹回 255 才会正确地算成透明。
    fn translucent_png_base64() -> String {
        let mut src = Document::new("src", 16, 16).expect("16x16");
        let colors = vec!["#c8502864".to_string(), "#29adff".to_string()];
        let ops = vec![
            PixelOperation::AddPaletteColors {
                colors: colors.clone(),
            },
            PixelOperation::SetPixels {
                layer: "L0".into(),
                frame: "F0".into(),
                cells: (0..16)
                    .flat_map(|y| (0..16).map(move |x| (x, y)))
                    .map(|(x, y)| PixelCell {
                        x,
                        y,
                        color: colors[((x + y) % 2) as usize].clone(),
                    })
                    .collect(),
            },
        ];
        ops::apply_batch(&mut src, &ops).expect("setup applies");
        png::base64_encode(&png::document_to_png(&src).expect("encodes"))
    }

    #[test]
    fn pixelize_tool_clamps_alpha_threshold_instead_of_wrapping() {
        let image = translucent_png_base64();
        let run = |threshold: u64| {
            let mut doc = Document::new("test", 16, 16).expect("16x16");
            execute(
                &mut doc,
                &active(),
                "pixel_pixelize_image",
                &json!({
                    "image_base64": image,
                    "media_type": "image/png",
                    "alpha_threshold": threshold,
                }),
            )
        };
        let clamped = run(300);
        let sane = run(255);
        assert!(!clamped.is_error, "{}", clamped.content);
        assert!(!sane.is_error, "{}", sane.content);
        assert_eq!(
            clamped.content, sane.content,
            "alpha=100 must be treated as transparent at threshold 300, not wrapped to 44"
        );
        assert!(
            clamped.content.contains("128 opaque"),
            "{}",
            clamped.content
        );
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
