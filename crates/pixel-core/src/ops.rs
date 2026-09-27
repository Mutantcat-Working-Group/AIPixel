//! 类型化像素操作。LLM 与 UI 都通过这套操作改文档，
//! 这是「不让模型手写矩阵」契约的核心。

use super::document::{Cel, Document, DocumentError, Frame, Layer, Rgba, MAX_FRAME_DURATION_MS};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PixelOperation {
    // ---- 帧 ----
    CreateFrame {
        #[serde(default)]
        after: Option<String>,
        #[serde(default = "default_duration")]
        duration_ms: u32,
        #[serde(default)]
        id: Option<String>,
    },
    DeleteFrame {
        id: String,
    },
    MoveFrame {
        id: String,
        to_index: usize,
    },
    SetFrameDuration {
        id: String,
        duration_ms: u32,
    },
    // ---- 图层 ----
    CreateLayer {
        #[serde(default)]
        after: Option<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        id: Option<String>,
    },
    DeleteLayer {
        id: String,
    },
    MoveLayer {
        id: String,
        to_index: usize,
    },
    RenameLayer {
        id: String,
        name: String,
    },
    SetLayerProperties {
        id: String,
        #[serde(default)]
        visible: Option<bool>,
        #[serde(default)]
        opacity: Option<u8>,
    },
    // ---- 调色板 ----
    AddPaletteColors {
        colors: Vec<String>,
    },
    // ---- 像素 ----
    SetPixels {
        layer: String,
        frame: String,
        cells: Vec<PixelCell>,
    },
    BucketFill {
        layer: String,
        frame: String,
        x: u32,
        y: u32,
        color: String,
    },
    DrawShape {
        layer: String,
        frame: String,
        shape: ShapeKind,
        x0: u32,
        y0: u32,
        x1: u32,
        y1: u32,
        color: String,
        #[serde(default)]
        filled: bool,
    },
    ClearRegion {
        layer: String,
        frame: String,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
    },
    StampGrid {
        layer: String,
        frame: String,
        /// ASCII 网格行，`.` 或空格 = 透明
        rows: Vec<String>,
        /// 符号 -> #RRGGBB 映射
        legend: std::collections::HashMap<String, String>,
        x: u32,
        y: u32,
    },
}

fn default_duration() -> u32 {
    100
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShapeKind {
    Line,
    Rect,
    Ellipse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PixelCell {
    pub x: u32,
    pub y: u32,
    pub color: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OperationError {
    #[error(transparent)]
    Document(#[from] DocumentError),
    #[error("invalid color literal: {0}")]
    BadColor(String),
    #[error("cell coordinate or size outside canvas")]
    OutOfCanvas,
    #[error("cannot delete the last layer or frame")]
    LastOfKind,
    #[error("stamp area exceeds canvas at ({x},{y})")]
    StampOverflow { x: u32, y: u32 },
}

/// 应用一组操作；任一失败则整批回滚（幂等失败，无半成品状态）。
pub fn apply_batch(doc: &mut Document, ops: &[PixelOperation]) -> Result<u64, OperationError> {
    let snapshot = doc.clone();
    for op in ops {
        if let Err(e) = apply_one(doc, op) {
            // 任一失败整批回滚，绝不留下半成品状态。
            *doc = snapshot;
            return Err(e);
        }
    }
    doc.check_limits().map_err(OperationError::Document)?;
    doc.bump();
    Ok(doc.revision)
}

pub fn apply_one(doc: &mut Document, op: &PixelOperation) -> Result<(), OperationError> {
    match op {
        PixelOperation::CreateFrame {
            after,
            duration_ms,
            id,
        } => {
            if *duration_ms < 1 || *duration_ms > MAX_FRAME_DURATION_MS {
                return Err(OperationError::Document(DocumentError::Dimension(
                    *duration_ms,
                )));
            }
            let new_id = id.clone().unwrap_or_else(|| {
                next_id(
                    "F",
                    &doc.frames.iter().map(|f| f.id.clone()).collect::<Vec<_>>(),
                )
            });
            let frame = Frame {
                id: new_id.clone(),
                duration_ms: *duration_ms,
            };
            let pos = match after {
                Some(after_id) => {
                    let p = doc
                        .frames
                        .iter()
                        .position(|f| &f.id == after_id)
                        .ok_or_else(|| {
                            OperationError::Document(DocumentError::UnknownFrame(after_id.clone()))
                        })?
                        + 1;
                    p
                }
                None => doc.frames.len(),
            };
            doc.frames.insert(pos.min(doc.frames.len()), frame);
            for layer in &doc.layers {
                doc.cels
                    .entry(layer.id.clone())
                    .or_default()
                    .insert(new_id.clone(), Cel::new(doc.width, doc.height));
            }
        }
        PixelOperation::DeleteFrame { id } => {
            if doc.frames.len() <= 1 {
                return Err(OperationError::LastOfKind);
            }
            let pos =
                doc.frames.iter().position(|f| &f.id == id).ok_or_else(|| {
                    OperationError::Document(DocumentError::UnknownFrame(id.clone()))
                })?;
            doc.frames.remove(pos);
            for frames in doc.cels.values_mut() {
                frames.remove(id);
            }
        }
        PixelOperation::MoveFrame { id, to_index } => {
            let pos =
                doc.frames.iter().position(|f| &f.id == id).ok_or_else(|| {
                    OperationError::Document(DocumentError::UnknownFrame(id.clone()))
                })?;
            let frame = doc.frames.remove(pos);
            doc.frames.insert((*to_index).min(doc.frames.len()), frame);
        }
        PixelOperation::SetFrameDuration { id, duration_ms } => {
            if *duration_ms < 1 || *duration_ms > MAX_FRAME_DURATION_MS {
                return Err(OperationError::Document(DocumentError::Dimension(
                    *duration_ms,
                )));
            }
            let frame =
                doc.frames.iter_mut().find(|f| &f.id == id).ok_or_else(|| {
                    OperationError::Document(DocumentError::UnknownFrame(id.clone()))
                })?;
            frame.duration_ms = *duration_ms;
        }
        PixelOperation::CreateLayer { after, name, id } => {
            let new_id = id.clone().unwrap_or_else(|| {
                next_id(
                    "L",
                    &doc.layers.iter().map(|l| l.id.clone()).collect::<Vec<_>>(),
                )
            });
            let layer = Layer {
                id: new_id.clone(),
                name: name
                    .clone()
                    .unwrap_or_else(|| format!("Layer {}", doc.layers.len() + 1)),
                visible: true,
                opacity: 255,
            };
            let pos = match after {
                Some(after_id) => {
                    let p = doc
                        .layers
                        .iter()
                        .position(|l| &l.id == after_id)
                        .ok_or_else(|| {
                            OperationError::Document(DocumentError::UnknownLayer(after_id.clone()))
                        })?
                        + 1;
                    p
                }
                None => doc.layers.len(),
            };
            doc.layers.insert(pos.min(doc.layers.len()), layer);
            let mut frames = BTreeMap::new();
            for frame in &doc.frames {
                frames.insert(frame.id.clone(), Cel::new(doc.width, doc.height));
            }
            doc.cels.insert(new_id.clone(), frames);
        }
        PixelOperation::DeleteLayer { id } => {
            if doc.layers.len() <= 1 {
                return Err(OperationError::LastOfKind);
            }
            let pos =
                doc.layers.iter().position(|l| &l.id == id).ok_or_else(|| {
                    OperationError::Document(DocumentError::UnknownLayer(id.clone()))
                })?;
            doc.layers.remove(pos);
            doc.cels.remove(id);
        }
        PixelOperation::MoveLayer { id, to_index } => {
            let pos =
                doc.layers.iter().position(|l| &l.id == id).ok_or_else(|| {
                    OperationError::Document(DocumentError::UnknownLayer(id.clone()))
                })?;
            let layer = doc.layers.remove(pos);
            doc.layers.insert((*to_index).min(doc.layers.len()), layer);
        }
        PixelOperation::RenameLayer { id, name } => {
            let layer =
                doc.layers.iter_mut().find(|l| &l.id == id).ok_or_else(|| {
                    OperationError::Document(DocumentError::UnknownLayer(id.clone()))
                })?;
            layer.name = name.clone();
        }
        PixelOperation::SetLayerProperties {
            id,
            visible,
            opacity,
        } => {
            let layer =
                doc.layers.iter_mut().find(|l| &l.id == id).ok_or_else(|| {
                    OperationError::Document(DocumentError::UnknownLayer(id.clone()))
                })?;
            if let Some(v) = visible {
                layer.visible = *v;
            }
            if let Some(o) = opacity {
                layer.opacity = *o;
            }
        }
        PixelOperation::AddPaletteColors { colors } => {
            for hex in colors {
                let color =
                    Rgba::parse_hex(hex).ok_or_else(|| OperationError::BadColor(hex.clone()))?;
                doc.intern_color(color).map_err(OperationError::Document)?;
            }
        }
        PixelOperation::SetPixels {
            layer,
            frame,
            cells,
        } => {
            // 先把颜色全部 intern 并做边界检查，再拿 cel 的可变借用，
            // 避免 intern（&mut doc）与 cel（&mut doc）同时活着。
            let mut painted: Vec<(u32, u32, u16)> = Vec::with_capacity(cells.len());
            for cell in cells {
                let idx = intern_color_str(doc, &cell.color)?;
                if cell.x >= doc.width || cell.y >= doc.height {
                    return Err(OperationError::OutOfCanvas);
                }
                painted.push((cell.x, cell.y, idx));
            }
            let (w, _h) = (doc.width, doc.height);
            let cel = cel_mut(doc, layer, frame)?;
            for (x, y, idx) in painted {
                cel.set(w, x, y, idx);
            }
        }
        PixelOperation::BucketFill {
            layer,
            frame,
            x,
            y,
            color,
        } => {
            if *x >= doc.width || *y >= doc.height {
                return Err(OperationError::OutOfCanvas);
            }
            let target = doc
                .cel(layer, frame)
                .ok_or_else(|| {
                    OperationError::Document(DocumentError::UnknownLayer(layer.clone()))
                })?
                .get(doc.width, *x, *y)
                .ok_or(OperationError::OutOfCanvas)?;
            let fill = intern_color_str(doc, color)?;
            let (w, h) = (doc.width, doc.height);
            let cel = cel_mut(doc, layer, frame)?;
            if target != fill {
                flood_fill(cel, w, h, *x, *y, target, fill);
            }
        }
        PixelOperation::DrawShape {
            layer,
            frame,
            shape,
            x0,
            y0,
            x1,
            y1,
            color,
            filled,
        } => {
            let idx = intern_color_str(doc, color)?;
            let (x0, y0, x1, y1) = (*x0, *y0, *x1, *y1);
            if x0.max(x1) >= doc.width || y0.max(y1) >= doc.height {
                return Err(OperationError::OutOfCanvas);
            }
            let (w, h) = (doc.width, doc.height);
            let cel = cel_mut(doc, layer, frame)?;
            match shape {
                ShapeKind::Line => draw_line(cel, w, h, x0, y0, x1, y1, idx),
                ShapeKind::Rect => draw_rect(cel, w, h, x0, y0, x1, y1, idx, *filled),
                ShapeKind::Ellipse => draw_ellipse(cel, w, h, x0, y0, x1, y1, idx, *filled),
            }
        }
        PixelOperation::ClearRegion {
            layer,
            frame,
            x,
            y,
            w,
            h,
        } => {
            let (cw, ch) = (doc.width, doc.height);
            let cel = cel_mut(doc, layer, frame)?;
            let x_end = (*x + *w).min(cw);
            let y_end = (*y + *h).min(ch);
            for cy in *y..y_end {
                for cx in *x..x_end {
                    cel.set(cw, cx, cy, 0);
                }
            }
        }
        PixelOperation::StampGrid {
            layer,
            frame,
            rows,
            legend,
            x,
            y,
        } => {
            let mut legend_idx = std::collections::HashMap::new();
            for (sym, hex) in legend {
                let idx = intern_color_str(doc, hex)?;
                legend_idx.insert(
                    sym.chars()
                        .next()
                        .ok_or_else(|| OperationError::BadColor(sym.clone()))?,
                    idx,
                );
            }
            legend_idx.entry('.').or_insert(0);
            legend_idx.entry(' ').or_insert(0);
            let (w, h) = (doc.width, doc.height);
            let cel = cel_mut(doc, layer, frame)?;
            for (dy, row) in rows.iter().enumerate() {
                let py = y.saturating_add(dy as u32);
                if py >= h {
                    return Err(OperationError::StampOverflow { x: *x, y: py });
                }
                for (dx, ch) in row.chars().enumerate() {
                    let px = x.saturating_add(dx as u32);
                    if px >= w {
                        return Err(OperationError::StampOverflow { x: px, y: py });
                    }
                    let idx = legend_idx.get(&ch).copied().unwrap_or(0);
                    cel.set(w, px, py, idx);
                }
            }
        }
    }
    Ok(())
}

fn cel_mut<'a>(
    doc: &'a mut Document,
    layer: &str,
    frame: &str,
) -> Result<&'a mut Cel, OperationError> {
    doc.cel_mut(layer, frame).ok_or_else(|| {
        OperationError::Document(DocumentError::UnknownLayer(format!("{layer}/{frame}")))
    })
}

fn intern_color_str(doc: &mut Document, hex: &str) -> Result<u16, OperationError> {
    let color = Rgba::parse_hex(hex).ok_or_else(|| OperationError::BadColor(hex.to_string()))?;
    doc.intern_color(color).map_err(OperationError::Document)
}

pub(crate) fn next_id(prefix: &str, existing: &[String]) -> String {
    for i in 0..existing.len() + 2 {
        let candidate = format!("{prefix}{i}");
        if !existing.iter().any(|e| e == &candidate) {
            return candidate;
        }
    }
    format!("{prefix}{}", existing.len())
}

pub fn flood_fill(
    cel: &mut Cel,
    width: u32,
    height: u32,
    sx: u32,
    sy: u32,
    target: u16,
    fill: u16,
) {
    let mut stack = vec![(sx, sy)];
    while let Some((x, y)) = stack.pop() {
        if x >= width || y >= height {
            continue;
        }
        if cel.get(width, x, y) != Some(target) {
            continue;
        }
        cel.set(width, x, y, fill);
        if x > 0 {
            stack.push((x - 1, y));
        }
        stack.push((x + 1, y));
        if y > 0 {
            stack.push((x, y - 1));
        }
        stack.push((x, y + 1));
    }
}

/// 端点 + 画布裁剪区作为平铺参数传入，是几何原语的惯用形态；拆成结构体反而遮住算法本体。
#[allow(clippy::too_many_arguments)]
pub fn draw_line(cel: &mut Cel, w: u32, h: u32, x0: u32, y0: u32, x1: u32, y1: u32, idx: u16) {
    let dx = (x1 as i64 - x0 as i64).abs();
    let dy = (y1 as i64 - y0 as i64).abs();
    let sx = if x0 < x1 { 1i64 } else { -1 };
    let sy = if y0 < y1 { 1i64 } else { -1 };
    let mut err = dx - dy;
    let (mut cx, mut cy) = (x0 as i64, y0 as i64);
    loop {
        if cx >= 0 && cy >= 0 && (cx as u32) < w && (cy as u32) < h {
            cel.set(w, cx as u32, cy as u32, idx);
        }
        if cx == x1 as i64 && cy == y1 as i64 {
            break;
        }
        let e2 = 2 * err;
        if e2 > -dy {
            err -= dy;
            cx += sx;
        }
        if e2 < dx {
            err += dx;
            cy += sy;
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn draw_rect(
    cel: &mut Cel,
    w: u32,
    _h: u32,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    idx: u16,
    filled: bool,
) {
    let (xa, xb) = (x0.min(x1), x0.max(x1));
    let (ya, yb) = (y0.min(y1), y0.max(y1));
    for y in ya..=yb {
        for x in xa..=xb {
            let edge = x == xa || x == xb || y == ya || y == yb;
            if filled || edge {
                cel.set(w, x, y, idx);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn draw_ellipse(
    cel: &mut Cel,
    w: u32,
    h: u32,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    idx: u16,
    filled: bool,
) {
    let (xa, xb) = (x0.min(x1) as f64, x0.max(x1) as f64);
    let (ya, yb) = (y0.min(y1) as f64, y0.max(y1) as f64);
    let cx = (xa + xb) / 2.0;
    let cy = (ya + yb) / 2.0;
    let rx = (xb - xa) / 2.0;
    let ry = (yb - ya) / 2.0;
    for y in ya as u32..=yb as u32 {
        for x in xa as u32..=xb as u32 {
            let d =
                ((x as f64 - cx) / rx.max(0.5)).powi(2) + ((y as f64 - cy) / ry.max(0.5)).powi(2);
            if ((filled && d <= 1.02) || (!filled && (d - 1.0).abs() < 0.35)) && x < w && y < h {
                cel.set(w, x, y, idx);
            }
        }
    }
}
