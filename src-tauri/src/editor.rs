//! 工作台编辑器命令层：画笔、油漆桶、帧与图层的直接操作。
//!
//! 和主循环共用 `AgentSession::with_document_mut` 的同一把锁，不会出现
//! 「agent 正在跑，用户的笔触插进去改坏画布」的交织。改完统一发
//! `AgentEvent::DocumentUpdated`，前端只有一条刷新路径。
//!
//! 编辑器为什么不直接复用 `pixel_apply_operations`：模型侧契约不收透明色
//! （索引 0 只能靠 clear_region 逐格表达），而画笔的橡皮、油漆桶的透明填充
//! 是编辑器的一等公民。这里用自己的窄接口，模型契约保持干净。

use pixel_core::document::{Document, Rgba};
use pixel_core::ops::{self, PixelOperation};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::state::AppState;
use crate::workflow::emit_document;

/// 一笔笔画里的一个格子。颜色按整笔给：一笔一色，换色必然是下一笔。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrokeCell {
    pub x: u32,
    pub y: u32,
}

/// 一次落笔：layer/frame 指明落在哪个 cel，`color` 为 None 表示擦回透明。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrokeRequest {
    pub layer: String,
    pub frame: String,
    pub cells: Vec<StrokeCell>,
    pub color: Option<String>,
}

/// 一次油漆桶：从 (x,y) 浸出去。`color` 为 None 表示把整片区域浸回透明。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FillRequest {
    pub layer: String,
    pub frame: String,
    pub x: u32,
    pub y: u32,
    pub color: Option<String>,
}

/// 颜色字面量 -> 调色板索引；None = 透明（索引 0）。
/// 规则和 ops::intern_color_str 一致，只多一条 None 分支给编辑器。
fn color_index(doc: &mut Document, hex: Option<&str>) -> Result<u16, String> {
    match hex {
        None => Ok(0),
        Some(hex) => {
            let color =
                Rgba::parse_hex(hex).ok_or_else(|| format!("invalid color literal: {hex}"))?;
            doc.intern_color(color).map_err(|e| e.to_string())
        }
    }
}

/// 把一笔笔画落进文档（纯函数，可单测）。
/// 整笔先校验再落笔：出界或 cel 不存在时一笔作废，不会留下半截笔画，
/// 也不会为了一个错坐标把新颜色提前塞进调色板。
pub fn apply_stroke(doc: &mut Document, stroke: &StrokeRequest) -> Result<u64, String> {
    let (w, h) = (doc.width, doc.height);
    {
        let _cel = doc
            .cel(&stroke.layer, &stroke.frame)
            .ok_or_else(|| format!("unknown cel {}/{}", stroke.layer, stroke.frame))?;
        for cell in &stroke.cells {
            if cell.x >= w || cell.y >= h {
                return Err(format!(
                    "cell ({},{}) is outside the {w}x{h} canvas",
                    cell.x, cell.y
                ));
            }
        }
    }
    let idx = color_index(doc, stroke.color.as_deref())?;
    let cel = doc
        .cel_mut(&stroke.layer, &stroke.frame)
        .ok_or_else(|| format!("unknown cel {}/{}", stroke.layer, stroke.frame))?;
    for cell in &stroke.cells {
        // 前面整笔校验过，这里再失败只可能是越界——宁折不弯，返回错误。
        if !cel.set(w, cell.x, cell.y, idx) {
            return Err(format!(
                "cell ({},{}) is outside the {w}x{h} canvas",
                cell.x, cell.y
            ));
        }
    }
    doc.check_limits().map_err(|e| e.to_string())?;
    doc.bump();
    Ok(doc.revision)
}

/// 油漆桶：从 (x,y) 向外浸染同一索引的区域，遇到不同索引就停。
/// `color` 为 None 时浸成透明——橡皮桶，删一大块比逐格擦快得多。
pub fn apply_fill(
    doc: &mut Document,
    layer: &str,
    frame: &str,
    x: u32,
    y: u32,
    color: Option<&str>,
) -> Result<u64, String> {
    let (w, h) = (doc.width, doc.height);
    {
        // 先只读校验：坐标与 cel 都站得住才动文档。
        let cel = doc
            .cel(layer, frame)
            .ok_or_else(|| format!("unknown cel {layer}/{frame}"))?;
        if cel.get(w, x, y).is_none() {
            return Err(format!(
                "fill origin ({x},{y}) is outside the {w}x{h} canvas"
            ));
        }
    }
    // 校验过了才 intern：错坐标不会把新颜色提前塞进调色板。
    let idx = color_index(doc, color)?;
    let cel = doc
        .cel_mut(layer, frame)
        .ok_or_else(|| format!("unknown cel {layer}/{frame}"))?;
    let Some(target) = cel.get(w, x, y) else {
        return Err(format!(
            "fill origin ({x},{y}) is outside the {w}x{h} canvas"
        ));
    };
    // 起点已经是目标色就别浸了：白跑一趟 flood fill 没有意义。
    if target != idx {
        ops::flood_fill(cel, w, h, x, y, target, idx);
    }
    doc.check_limits().map_err(|e| e.to_string())?;
    doc.bump();
    Ok(doc.revision)
}

/// 一笔笔画落文档并广播新文档。命令只做装配，逻辑都在 `apply_stroke`。
#[tauri::command]
pub fn editor_paint_stroke(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    stroke: StrokeRequest,
) -> Result<u64, String> {
    let session = state.session(&id)?;
    let revision = session.with_document_mut(|doc| apply_stroke(doc, &stroke))?;
    session.note_edit(stroke_note(&stroke, &session.document()));
    emit_document(&app, &session);
    Ok(revision)
}

/// 油漆桶落地。
#[tauri::command]
pub fn editor_fill(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    fill: FillRequest,
) -> Result<u64, String> {
    let session = state.session(&id)?;
    let revision = session.with_document_mut(|doc| {
        apply_fill(
            doc,
            &fill.layer,
            &fill.frame,
            fill.x,
            fill.y,
            fill.color.as_deref(),
        )
    })?;
    session.note_edit(fill_note(&fill));
    emit_document(&app, &session);
    Ok(revision)
}

/// 改画布大小（纯函数，可单测）：左上角锚定，装得下的像素原样保留。
/// 改完自查一遍限额——上限以内的宽高也可能把格子总量顶到很高，check_limits 是最后一道关。
pub fn apply_resize(doc: &mut Document, width: u32, height: u32) -> Result<u64, String> {
    doc.resize(width, height).map_err(|e| e.to_string())?;
    doc.check_limits().map_err(|e| e.to_string())?;
    doc.bump();
    Ok(doc.revision)
}

/// 改画布宽高并广播新文档。左上角 WxH 点开的那个弹窗落到这儿。
#[tauri::command]
pub fn editor_resize_canvas(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    width: u32,
    height: u32,
) -> Result<u64, String> {
    let session = state.session(&id)?;
    let before = session.document();
    let (old_width, old_height) = (before.width, before.height);
    if old_width == width && old_height == height {
        // 尺寸没动就别白刷一次 revision：前端靠 revision 判断要不要重画。
        return Ok(before.revision);
    }
    let revision = session.with_document_mut(|doc| apply_resize(doc, width, height))?;
    session.note_edit(format!(
        "canvas resized from {old_width}x{old_height} to {width}x{height}"
    ));
    emit_document(&app, &session);
    Ok(revision)
}

/// 结构与帧操作（含 duplicate_frame）直接复用 pixel-core 的 ops：
/// 同一套原子事务，模型和编辑器走的是一条代码路径。
#[tauri::command]
pub fn editor_apply_ops(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    ops: Vec<PixelOperation>,
) -> Result<u64, String> {
    let session = state.session(&id)?;
    let revision =
        session.with_document_mut(|doc| ops::apply_batch(doc, &ops).map_err(|e| e.to_string()))?;
    session.note_edit(ops_note(&ops));
    emit_document(&app, &session);
    Ok(revision)
}

/// 一句话簿记：在哪儿画了几个格子、用的什么颜色。
/// 颜色给 hex 原样，不换算色名——读这条的是模型，它要的是能直接写回笔尖的值。
fn stroke_note(stroke: &StrokeRequest, doc: &Document) -> String {
    let color = match stroke.color.as_deref() {
        Some(hex) => hex.to_string(),
        None => "transparent (erased)".to_string(),
    };
    let spot = describe_cel(doc, &stroke.layer, &stroke.frame);
    format!(
        "brush stroke of {} cell(s) on {} using {}",
        stroke.cells.len(),
        spot,
        color
    )
}

/// 一句话簿记：从哪一格里浸染出去、浸成了什么颜色。
fn fill_note(fill: &FillRequest) -> String {
    let color = fill
        .color
        .as_deref()
        .unwrap_or("transparent (erased)")
        .to_string();
    format!(
        "paint bucket from ({},{}) on {}/{} flooded to {}",
        fill.x, fill.y, fill.layer, fill.frame, color
    )
}

/// cel 的一句话名片：图层名 / 帧号，再加一句「这是第几帧」。
/// 模型读到的 layer/frame 是 id（L0/F2），而用户脑子里是名字，两个都给最稳。
fn describe_cel(doc: &Document, layer: &str, frame: &str) -> String {
    let layer_name = doc
        .layers
        .iter()
        .find(|l| l.id == layer)
        .map(|l| l.name.clone())
        .unwrap_or_else(|| layer.to_string());
    let (frame_no, frame_name) = doc
        .frames
        .iter()
        .enumerate()
        .find(|(_, f)| f.id == frame)
        .map(|(i, f)| (i + 1, f.id.clone()))
        .unwrap_or((0, frame.to_string()));
    let _ = frame_name;
    format!("layer {layer} \"{layer_name}\", frame {frame} (#{frame_no})")
}

/// 结构与帧操作的簿记：把这一批 op 说成人话，一条一批。
/// 模型下一轮要判断「用户已经把这些结构动过了」，不需要每个字段都背下来。
fn ops_note(ops: &[PixelOperation]) -> String {
    if ops.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = ops.iter().map(op_summary).collect();
    format!("structure edits applied: {}", parts.join("; "))
}

fn op_summary(op: &PixelOperation) -> String {
    match op {
        PixelOperation::CreateFrame { after, .. } => {
            format!("created a frame after {after:?}")
        }
        PixelOperation::DeleteFrame { id } => format!("deleted frame {id}"),
        PixelOperation::DuplicateFrame { id, .. } => {
            format!("duplicated frame {id}")
        }
        PixelOperation::MoveFrame { id, to_index } => {
            format!("moved frame {id} to slot {to_index}")
        }
        PixelOperation::SetFrameDuration { id, duration_ms } => {
            format!("frame {id} now lasts {duration_ms}ms")
        }
        PixelOperation::CreateLayer { after, name, .. } => {
            format!("created layer {name:?} after {after:?}")
        }
        PixelOperation::DeleteLayer { id } => format!("deleted layer {id}"),
        PixelOperation::MoveLayer { id, to_index } => {
            format!("moved layer {id} to slot {to_index}")
        }
        PixelOperation::RenameLayer { id, name } => {
            format!("renamed layer {id} to \"{name}\"")
        }
        PixelOperation::SetLayerProperties {
            id,
            visible,
            opacity,
        } => {
            format!("layer {id} properties now visible={visible:?} opacity={opacity:?}")
        }
        PixelOperation::AddPaletteColors { colors } => {
            format!(
                "added {} palette color(s): {}",
                colors.len(),
                colors.join(" ")
            )
        }
        PixelOperation::SetPalette { colors } => {
            format!(
                "palette replaced with {} color(s): {}",
                colors.len(),
                colors.join(" ")
            )
        }
        PixelOperation::SetPixels { layer, frame, .. } => {
            format!("set pixels directly on {layer}/{frame}")
        }
        PixelOperation::BucketFill {
            layer, frame, x, y, ..
        } => {
            format!("bucket fill on {layer}/{frame} from ({x},{y})")
        }
        PixelOperation::DrawShape {
            layer,
            frame,
            shape,
            ..
        } => {
            format!("drew a {shape:?} on {layer}/{frame}")
        }
        PixelOperation::StampGrid { layer, frame, .. } => {
            format!("stamped a grid on {layer}/{frame}")
        }
        PixelOperation::ClearRegion { layer, frame, .. } => {
            format!("cleared a region on {layer}/{frame}")
        }
        PixelOperation::CreatePalette { name, .. } => {
            format!("created color range \"{name}\"")
        }
        PixelOperation::DeletePalette { id } => format!("deleted color range {id}"),
        PixelOperation::RenamePalette { id, name } => {
            format!("renamed color range {id} to \"{name}\"")
        }
        PixelOperation::AddPaletteColor { id, color } => {
            format!("added {color} to color range {id}")
        }
        PixelOperation::RemovePaletteColor { id, index } => {
            format!("removed color #{index} from color range {id}")
        }
        PixelOperation::SetLayerPalette { layer, palette_id } => {
            format!("layer {layer} now uses color range {palette_id}")
        }
        PixelOperation::SetLayerLocked { layer, locked } => {
            format!("layer {layer} color lock = {locked}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Document {
        let mut d = Document::new("editor", 8, 8).expect("8x8 stays in limits");
        let red = Rgba::parse_hex("#FF004D").expect("hex parses");
        d.intern_color(red).expect("first color interns");
        d
    }

    fn cell(x: u32, y: u32) -> StrokeCell {
        StrokeCell { x, y }
    }

    #[test]
    fn a_stroke_paints_the_cells_it_names_and_bumps_the_revision() {
        let mut d = doc();
        let before = d.revision;
        let stroke = StrokeRequest {
            layer: "L0".into(),
            frame: "F0".into(),
            cells: vec![cell(1, 1), cell(2, 1)],
            color: Some("#FF004D".into()),
        };
        let revision = apply_stroke(&mut d, &stroke).expect("stroke applies");
        assert_eq!(revision, before + 1);
        assert_eq!(d.cels["L0"]["F0"].indices[9], 1);
        assert_eq!(d.cels["L0"]["F0"].indices[10], 1);
        assert_eq!(d.cels["L0"]["F0"].indices[0], 0, "没点过的格子不动");
    }

    #[test]
    fn a_stroke_without_color_erases_back_to_transparent() {
        let mut d = doc();
        let painted = StrokeRequest {
            layer: "L0".into(),
            frame: "F0".into(),
            cells: vec![cell(3, 3)],
            color: Some("#FF004D".into()),
        };
        apply_stroke(&mut d, &painted).expect("paint applies");
        assert_eq!(d.cels["L0"]["F0"].indices[27], 1);

        let erased = StrokeRequest {
            layer: "L0".into(),
            frame: "F0".into(),
            cells: vec![cell(3, 3)],
            color: None,
        };
        apply_stroke(&mut d, &erased).expect("erase applies");
        assert_eq!(d.cels["L0"]["F0"].indices[27], 0, "None = 擦回透明");
    }

    #[test]
    fn fill_spreads_over_the_same_index_and_stops_at_the_border() {
        let mut d = doc();
        // 墙必须贯通整列：只画一半高，浸染会从墙底绕过去，就测不到「停在边界」。
        let wall = StrokeRequest {
            layer: "L0".into(),
            frame: "F0".into(),
            cells: (0..8).map(|y| cell(1, y)).collect(),
            color: Some("#FF004D".into()),
        };
        apply_stroke(&mut d, &wall).expect("wall applies");

        apply_fill(&mut d, "L0", "F0", 0, 0, Some("#29ADFF")).expect("fill applies");
        // 只浸染左侧通道：墙和墙右边的格子原封不动。
        assert_eq!(d.cels["L0"]["F0"].indices[0], 2, "起点被浸成新色");
        assert_eq!(d.cels["L0"]["F0"].indices[8], 2, "通道内连续浸染");
        assert_eq!(d.cels["L0"]["F0"].indices[9], 1, "墙本身不动");
        assert_eq!(d.cels["L0"]["F0"].indices[18], 0, "墙右侧不动");
        assert_eq!(d.cels["L0"]["F0"].indices[63], 0, "右下角也不动");
    }

    #[test]
    fn fill_without_color_washes_a_region_transparent() {
        let mut d = doc();
        let blob = StrokeRequest {
            layer: "L0".into(),
            frame: "F0".into(),
            cells: vec![cell(0, 0), cell(0, 1), cell(0, 2)],
            color: Some("#FF004D".into()),
        };
        apply_stroke(&mut d, &blob).expect("blob applies");
        apply_fill(&mut d, "L0", "F0", 0, 0, None).expect("erase fill applies");
        assert_eq!(d.cels["L0"]["F0"].indices[0], 0);
        assert_eq!(d.cels["L0"]["F0"].indices[16], 0);
    }

    #[test]
    fn a_stroke_outside_the_canvas_is_rejected_without_leaving_half_of_it() {
        let mut d = doc();
        let stroke = StrokeRequest {
            layer: "L0".into(),
            frame: "F0".into(),
            cells: vec![cell(0, 0), cell(99, 0)],
            color: Some("#FF004D".into()),
        };
        assert!(apply_stroke(&mut d, &stroke).is_err());
        // 出界即整笔作废，不能落下一半。
        assert_eq!(d.cels["L0"]["F0"].indices[0], 0);
    }

    #[test]
    fn an_unknown_cel_is_named_in_the_error() {
        let mut d = doc();
        let stroke = StrokeRequest {
            layer: "L0".into(),
            frame: "F9".into(),
            cells: vec![cell(0, 0)],
            color: Some("#FF004D".into()),
        };
        let error = apply_stroke(&mut d, &stroke).expect_err("unknown frame");
        assert!(error.contains("F9"), "{error}");
    }

    #[test]
    fn a_fill_that_starts_on_its_own_color_changes_nothing() {
        let mut d = doc();
        apply_fill(&mut d, "L0", "F0", 0, 0, Some("#FF004D")).expect("fill applies");
        let painted = d.cels["L0"]["F0"].clone();
        apply_fill(&mut d, "L0", "F0", 0, 0, Some("#FF004D")).expect("same-color fill applies");
        assert_eq!(d.cels["L0"]["F0"].indices, painted.indices);
    }

    /// 放大画布：原像素留住、版本号往前动一格，前端才会跟着重画。
    #[test]
    fn resizing_the_canvas_keeps_existing_pixels_and_bumps_the_revision() {
        let mut d = doc();
        apply_stroke(
            &mut d,
            &StrokeRequest {
                layer: "L0".into(),
                frame: "F0".into(),
                cells: vec![cell(1, 1)],
                color: Some("#FF004D".into()),
            },
        )
        .expect("stroke applies");
        let before = d.revision;

        let revision = apply_resize(&mut d, 12, 12).expect("grow applies");

        assert_eq!(revision, before + 1);
        assert_eq!((d.width, d.height), (12, 12));
        assert_eq!(d.cels["L0"]["F0"].indices.len(), 144);
        // 改动之后宽度是 12，那一笔落在 y=1、x=1，所以下标是 stride + 1。
        let stride = d.width as usize;
        assert_eq!(
            d.cels["L0"]["F0"].indices[stride + 1],
            1,
            "画过的格子还在原位"
        );
    }

    #[test]
    fn resizing_to_the_same_dimensions_still_delivers_a_revision() {
        let mut d = doc();
        let before = d.revision;
        // 前端点了确认但尺寸没动：也照常回一个新 revision，界面不会卡在旧读数上。
        let revision = apply_resize(&mut d, 8, 8).expect("same size applies");
        assert_eq!(revision, before + 1);
    }

    #[test]
    fn resizing_outside_the_limits_is_refused_wholesale() {
        let mut d = doc();
        let before = (d.width, d.height);
        assert!(apply_resize(&mut d, 0, 8).is_err(), "0 不是合法宽高");
        assert_eq!((d.width, d.height), before, "被拒的尺寸不许改动文档");
    }
}
