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
        let cel = doc.cel(layer, frame).ok_or_else(|| format!("unknown cel {layer}/{frame}"))?;
        if cel.get(w, x, y).is_none() {
            return Err(format!("fill origin ({x},{y}) is outside the {w}x{h} canvas"));
        }
    }
    // 校验过了才 intern：错坐标不会把新颜色提前塞进调色板。
    let idx = color_index(doc, color)?;
    let cel = doc
        .cel_mut(layer, frame)
        .ok_or_else(|| format!("unknown cel {layer}/{frame}"))?;
    let Some(target) = cel.get(w, x, y) else {
        return Err(format!("fill origin ({x},{y}) is outside the {w}x{h} canvas"));
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
    emit_document(&app, &session);
    Ok(revision)
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
}
