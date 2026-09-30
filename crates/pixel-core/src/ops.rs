//! 类型化像素操作。LLM 与 UI 都通过这套操作改文档，
//! 这是「不让模型手写矩阵」契约的核心。

use super::document::{
    Cel, Document, DocumentError, Frame, Layer, NamedPalette, Rgba, MAX_FRAME_DURATION_MS,
    MAX_PALETTE,
};
use super::palettes::MAX_PALETTES;
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
        /// 帧号。`frame` 是文档与模型口中的写法，两种都收。
        #[serde(alias = "frame")]
        id: String,
    },
    /// 整帧复制（所有图层的 cel 一起），插在源帧后面。
    /// 「照着这一帧改」是编辑器与模型都高频的动作，值得一个原子操作。
    DuplicateFrame {
        #[serde(alias = "frame")]
        id: String,
        /// 指定落点：插在这一帧号之后；缺省就是源帧后面。
        #[serde(default)]
        after: Option<String>,
    },
    MoveFrame {
        #[serde(alias = "frame")]
        id: String,
        to_index: usize,
    },
    SetFrameDuration {
        #[serde(alias = "frame")]
        id: String,
        duration_ms: u32,
    },
    // ---- 图层 ----
    CreateLayer {
        #[serde(default)]
        after: Option<String>,
        #[serde(default)]
        name: Option<String>,
        /// 建层就指一套配色范围；缺省继承邻居那层（见 apply 里的取值）。
        #[serde(default)]
        palette_id: Option<String>,
        /// 建层就上锁；缺省跟着邻居。
        #[serde(default)]
        locked: Option<bool>,
        #[serde(default)]
        id: Option<String>,
    },
    DeleteLayer {
        #[serde(alias = "layer")]
        id: String,
    },
    MoveLayer {
        #[serde(alias = "layer")]
        id: String,
        to_index: usize,
    },
    RenameLayer {
        #[serde(alias = "layer")]
        id: String,
        name: String,
    },
    SetLayerProperties {
        #[serde(alias = "layer")]
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
    /// 整幅换调色板：已有像素按颜色就近映射到新配色。
    /// 「换配色范围」对用户就是把图画进另一套色板，语义上必须保留画面而不是留透明。
    SetPalette {
        colors: Vec<String>,
    },
    // ---- 命名配色范围 ----
    /// 新建一套配色范围。`from` 是复制某套现成的当起点——改内置预设就走这条，
    /// 落点永远是副本；`colors` 是直接从零列色。建完可以把某一层指过去。
    CreatePalette {
        name: String,
        #[serde(default)]
        from: Option<String>,
        #[serde(default)]
        colors: Vec<String>,
        #[serde(default)]
        layer: Option<String>,
        #[serde(default)]
        id: Option<String>,
    },
    /// 删掉一套。内置删不得，且正在被某层引用的删掉之前必须先让引用方改指别的。
    DeletePalette {
        id: String,
    },
    RenamePalette {
        id: String,
        name: String,
    },
    AddPaletteColor {
        id: String,
        color: String,
    },
    RemovePaletteColor {
        id: String,
        index: usize,
    },
    /// 把某一层指到另一套范围上。换层等于把这一层的像素就地收进新范围。
    SetLayerPalette {
        layer: String,
        palette_id: String,
    },
    SetLayerLocked {
        layer: String,
        locked: bool,
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
    #[error("builtin palette {0} cannot be modified")]
    BuiltinPalette(String),
    #[error("unknown palette: {0}")]
    UnknownPalette(String),
    #[error("would exceed palette limits: {0} palettes in one document")]
    TooManyPalettes(usize),
    #[error("cannot remove the last color of palette {0}")]
    LastPaletteColor(String),
    #[error("palette {0} is still used by layer {1}")]
    PaletteInUse(String, String),
    #[error("palette color index out of range: {0}")]
    PaletteColorIndex(usize),
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
        PixelOperation::DuplicateFrame { id, after } => {
            let src =
                doc.frames.iter().position(|f| &f.id == id).ok_or_else(|| {
                    OperationError::Document(DocumentError::UnknownFrame(id.clone()))
                })?;
            // 落点：显式指定的那帧之后；没指定就到源帧后面。
            let pos = match after {
                Some(after_id) => {
                    doc.frames
                        .iter()
                        .position(|f| &f.id == after_id)
                        .ok_or_else(|| {
                            OperationError::Document(DocumentError::UnknownFrame(after_id.clone()))
                        })?
                        + 1
                }
                None => src + 1,
            };
            let existing: Vec<String> = doc.frames.iter().map(|f| f.id.clone()).collect();
            let new_id = next_id("F", &existing);
            let duration_ms = doc.frames[src].duration_ms;
            // 先把各图层的 cel 副本取出来再动 frames，避免同时借 doc.cels 和 doc.frames。
            let mut copies: Vec<(String, Cel)> = Vec::with_capacity(doc.layers.len());
            for layer in &doc.layers {
                let cel = doc
                    .cels
                    .get(&layer.id)
                    .and_then(|frames| frames.get(id))
                    .cloned()
                    .unwrap_or_else(|| Cel::new(doc.width, doc.height));
                copies.push((layer.id.clone(), cel));
            }
            doc.frames.insert(
                pos.min(doc.frames.len()),
                Frame {
                    id: new_id.clone(),
                    duration_ms,
                },
            );
            for (layer_id, cel) in copies {
                doc.cels
                    .entry(layer_id)
                    .or_default()
                    .insert(new_id.clone(), cel);
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
        PixelOperation::CreateLayer {
            after,
            name,
            palette_id,
            locked,
            id,
        } => {
            // 插入位置决定了新图层跟谁做邻居：配色范围和锁不锁都跟着邻居走。
            // 往一叠「都锁在同一套色板」的图层中间插一层，不该突然冒出一个
            // 自由散色班子；插在最顶上就继承栈顶那层。
            let pos = match after {
                Some(after_id) => {
                    doc.layers
                        .iter()
                        .position(|l| &l.id == after_id)
                        .ok_or_else(|| {
                            OperationError::Document(DocumentError::UnknownLayer(after_id.clone()))
                        })?
                        + 1
                }
                None => doc.layers.len(),
            };
            let inherited = doc
                .layers
                .get(pos.saturating_sub(1))
                .map(|l| (l.palette_id.clone(), l.locked))
                .filter(|(id, _)| doc.palette_by_id(id).is_some())
                .unwrap_or_else(|| (crate::document::DEFAULT_PALETTE_ID.to_string(), false));
            let new_id = id.clone().unwrap_or_else(|| {
                next_id(
                    "L",
                    &doc.layers.iter().map(|l| l.id.clone()).collect::<Vec<_>>(),
                )
            });
            // 显式指定的配色范围必须真的存在，否则模型会拿一个拼错的 id
            // 建出一层「指向不存在色板」的层，错误直到后面才炸。
            let palette = match palette_id {
                Some(pid) => {
                    if doc.palette_by_id(pid).is_none() {
                        return Err(OperationError::UnknownPalette(pid.clone()));
                    }
                    pid.clone()
                }
                None => inherited.0,
            };
            let layer = Layer {
                id: new_id.clone(),
                name: name
                    .clone()
                    .unwrap_or_else(|| format!("Layer {}", doc.layers.len() + 1)),
                visible: true,
                opacity: 255,
                palette_id: palette,
                locked: locked.unwrap_or(inherited.1),
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
        PixelOperation::SetPalette { colors } => {
            let mut next: Vec<Rgba> = Vec::with_capacity(colors.len());
            for hex in colors {
                let color =
                    Rgba::parse_hex(hex).ok_or_else(|| OperationError::BadColor(hex.clone()))?;
                next.push(color);
            }
            // 就近映射表：旧索引 -> 新索引。透明（0）与调色板外的东西都归 0。
            let mut remap = vec![0u16; doc.palette.len() + 1];
            for (old, color) in doc.palette.iter().enumerate() {
                remap[old + 1] = nearest_index(&next, *color)
                    .map(|i| i as u16 + 1)
                    .unwrap_or(0);
            }
            for cel in doc.cels.values_mut().flat_map(|m| m.values_mut()) {
                for idx in cel.indices.iter_mut() {
                    let old = *idx as usize;
                    *idx = remap.get(old).copied().unwrap_or(0);
                }
            }
            doc.palette = next;
        }
        PixelOperation::CreatePalette {
            name,
            from,
            colors,
            layer,
            id,
        } => {
            doc.ensure_palette_scope();
            if doc.palettes.len() >= MAX_PALETTES {
                return Err(OperationError::TooManyPalettes(doc.palettes.len()));
            }
            // 复制现成的那一套当起点：内置的走 fork_builtin（原套一个色都不动），
            // 用户自己那套走 shallow 复制，改的是新的一份。
            let mut palette = match from {
                Some(src_id) => {
                    let src = doc
                        .palette_by_id(src_id)
                        .cloned()
                        .ok_or_else(|| OperationError::UnknownPalette(src_id.clone()))?;
                    if src.builtin {
                        crate::palettes::fork_builtin(&src, &doc.palettes)
                    } else {
                        NamedPalette {
                            id: crate::palettes::unique_palette_id(
                                &doc.palettes,
                                &crate::palettes::slugify(name),
                            ),
                            name: name.clone(),
                            colors: src.colors.clone(),
                            builtin: false,
                        }
                    }
                }
                None => NamedPalette {
                    id: id.clone().unwrap_or_else(|| {
                        crate::palettes::unique_palette_id(
                            &doc.palettes,
                            &crate::palettes::slugify(name),
                        )
                    }),
                    name: name.clone(),
                    colors: Vec::new(),
                    builtin: false,
                },
            };
            if let Some(explicit) = id {
                palette.id = explicit.clone();
                if doc.palette_by_id(explicit).is_some() {
                    return Err(OperationError::UnknownPalette(format!("{explicit} exists")));
                }
            }
            for hex in colors {
                let color =
                    Rgba::parse_hex(hex).ok_or_else(|| OperationError::BadColor(hex.clone()))?;
                if palette.colors.contains(&color) {
                    continue;
                }
                if palette.colors.len() >= MAX_PALETTE {
                    return Err(OperationError::Document(DocumentError::PaletteFull));
                }
                palette.colors.push(color);
            }
            let new_id = palette.id.clone();
            if doc.palette_by_id(&new_id).is_some() {
                return Err(OperationError::UnknownPalette(format!("{new_id} exists")));
            }
            doc.palettes.push(palette);
            // 调用方指定的那一层跟着换过去；换范围要把这一层的像素就地收进新范围。
            if let Some(layer_id) = layer {
                apply_one(
                    doc,
                    &PixelOperation::SetLayerPalette {
                        layer: layer_id.clone(),
                        palette_id: new_id,
                    },
                )?;
            }
        }
        PixelOperation::DeletePalette { id } => {
            if crate::palettes::is_builtin_id(id) {
                return Err(OperationError::BuiltinPalette(id.clone()));
            }
            let pos = doc
                .palettes
                .iter()
                .position(|p| &p.id == id)
                .ok_or_else(|| OperationError::UnknownPalette(id.clone()))?;
            // 还被图层引用着就先别删：静默改指会让别的层莫名其妙换色板。
            if let Some(layer) = doc.layers.iter().find(|l| &l.palette_id == id) {
                return Err(OperationError::PaletteInUse(id.clone(), layer.id.clone()));
            }
            doc.palettes.remove(pos);
        }
        PixelOperation::RenamePalette { id, name } => {
            if crate::palettes::is_builtin_id(id) {
                return Err(OperationError::BuiltinPalette(id.clone()));
            }
            let trimmed = name.trim();
            if trimmed.is_empty() {
                return Err(OperationError::UnknownPalette("empty name".into()));
            }
            let palette = doc
                .palettes
                .iter_mut()
                .find(|p| &p.id == id)
                .ok_or_else(|| OperationError::UnknownPalette(id.clone()))?;
            palette.name = trimmed.to_string();
        }
        PixelOperation::AddPaletteColor { id, color } => {
            if crate::palettes::is_builtin_id(id) {
                return Err(OperationError::BuiltinPalette(id.clone()));
            }
            let color =
                Rgba::parse_hex(color).ok_or_else(|| OperationError::BadColor(color.clone()))?;
            let palette = doc
                .palettes
                .iter_mut()
                .find(|p| &p.id == id)
                .ok_or_else(|| OperationError::UnknownPalette(id.clone()))?;
            if palette.colors.contains(&color) {
                return Ok(());
            }
            if palette.colors.len() >= MAX_PALETTE {
                return Err(OperationError::Document(DocumentError::PaletteFull));
            }
            palette.colors.push(color);
        }
        PixelOperation::RemovePaletteColor { id, index } => {
            if crate::palettes::is_builtin_id(id) {
                return Err(OperationError::BuiltinPalette(id.clone()));
            }
            let palette = doc
                .palettes
                .iter_mut()
                .find(|p| &p.id == id)
                .ok_or_else(|| OperationError::UnknownPalette(id.clone()))?;
            if *index >= palette.colors.len() {
                return Err(OperationError::PaletteColorIndex(*index));
            }
            if palette.colors.len() <= 1 {
                return Err(OperationError::LastPaletteColor(id.clone()));
            }
            // 删的是范围不是存储：cel 里的索引照旧指向文档调色板，一个像素都不动。
            // 真要在画面上消掉这个色，用户自己用橡皮擦，或者换一套范围触发重归队。
            palette.colors.remove(*index);
        }
        PixelOperation::SetLayerPalette { layer, palette_id } => {
            let layer_exists = doc.layers.iter().any(|l| &l.id == layer);
            if !layer_exists {
                return Err(OperationError::Document(DocumentError::UnknownLayer(
                    layer.clone(),
                )));
            }
            let target = doc
                .palette_by_id(palette_id)
                .ok_or_else(|| OperationError::UnknownPalette(palette_id.clone()))?
                .colors
                .clone();
            let found = doc
                .layers
                .iter_mut()
                .find(|l| &l.id == layer)
                .expect("layer existence checked above");
            found.palette_id = palette_id.clone();
            requantize_layer(doc, layer, &target)?;
        }
        PixelOperation::SetLayerLocked { layer, locked } => {
            let found = doc
                .layers
                .iter_mut()
                .find(|l| &l.id == layer)
                .ok_or_else(|| {
                    OperationError::Document(DocumentError::UnknownLayer(layer.clone()))
                })?;
            found.locked = *locked;
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
                let idx = intern_layer_color_str(doc, layer, &cell.color)?;
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
            let fill = intern_layer_color_str(doc, layer, color)?;
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
            let idx = intern_layer_color_str(doc, layer, color)?;
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
                let idx = intern_layer_color_str(doc, layer, hex)?;
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

/// 写像素用的颜色落地：先按图层的配色范围仲裁，再 intern 进文档调色板。
/// 锁着的层越界颜色就近归队，没锁的层原样放行——「锁」的力气全在这一步。
fn intern_layer_color_str(
    doc: &mut Document,
    layer: &str,
    hex: &str,
) -> Result<u16, OperationError> {
    let color = Rgba::parse_hex(hex).ok_or_else(|| OperationError::BadColor(hex.to_string()))?;
    let color = doc.color_for_layer(layer, color);
    doc.intern_color(color).map_err(OperationError::Document)
}

/// 把某一层的像素就地收进 `range` 这套新范围。
///
/// 文档调色板是存储、命名范围是约束，两件事分开：所以这里要先把
/// `doc.palette` 里每个颜色折算成新范围里的那个颜色，再 intern 成
/// 新下标重指 cel。透明永远留在 0，换范围绝不把像素擦掉。
fn requantize_layer(doc: &mut Document, layer: &str, range: &[Rgba]) -> Result<(), OperationError> {
    if range.is_empty() {
        return Err(OperationError::UnknownPalette("empty range".into()));
    }
    // cel 下标比 palette 下标多 1（下标 0 是透明），所以 targets 前面补一个透明。
    let mut targets: Vec<Rgba> = Vec::with_capacity(doc.palette.len() + 1);
    targets.push(Rgba::TRANSPARENT);
    for color in &doc.palette {
        let nearest = super::document::nearest_color(range, *color).unwrap_or(*color);
        targets.push(nearest);
    }
    // range 里的颜色要先进文档调色板才拿得到下标，这一步是纯写 palette。
    let mut remap: Vec<u16> = Vec::with_capacity(targets.len());
    for target in &targets {
        remap.push(
            doc.intern_color(*target)
                .map_err(OperationError::Document)?,
        );
    }
    let Some(frames) = doc.cels.get_mut(layer) else {
        return Ok(());
    };
    for cel in frames.values_mut() {
        for idx in cel.indices.iter_mut() {
            if let Some(&next) = remap.get(*idx as usize) {
                *idx = next;
            }
        }
    }
    Ok(())
}

/// 新调色板里和 `color` 最接近的下标。RGB 欧氏距离，不比较 alpha：
/// 换色板是肉眼决策，半透明的那点差别交给新色板自己说话。
fn nearest_index(palette: &[Rgba], color: Rgba) -> Option<usize> {
    palette
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            perceptual_distance(**a, color)
                .partial_cmp(&perceptual_distance(**b, color))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
}

/// CIELAB（D65）下的欧氏距离平方。换调色板时按感知距离而不是裸 RGB
/// 找最接近的颜色：红色对照 {黑, 白} 时裸 RGB 会判成黑，人眼明显更像白，
/// 换成 Lab 距离就落到白。像素画换预设色板是高频操作，值得这一步精确。
fn perceptual_distance(a: Rgba, b: Rgba) -> f64 {
    let (l1, a1, b1) = lab(a);
    let (l2, a2, b2) = lab(b);
    let dl = l1 - l2;
    let da = a1 - a2;
    let db = b1 - b2;
    dl * dl + da * da + db * db
}

/// sRGB -> CIELAB，x/y/z 白点取 D65。
fn lab(c: Rgba) -> (f64, f64, f64) {
    let (x, y, z) = xyz(c);
    // D65 白点归一化，f(t) 是 CIE 标准化分段函数。
    let fx = pivot(x / 0.95047);
    let fy = pivot(y);
    let fz = pivot(z / 1.08883);
    (116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz))
}

fn pivot(t: f64) -> f64 {
    if t > 0.008856 {
        t.cbrt()
    } else {
        7.787 * t + 16.0 / 116.0
    }
}

fn xyz(c: Rgba) -> (f64, f64, f64) {
    let r = linearize(c.r);
    let g = linearize(c.g);
    let b = linearize(c.b);
    (
        0.4124564 * r + 0.3575761 * g + 0.1804375 * b,
        0.2126729 * r + 0.7151522 * g + 0.0721750 * b,
        0.0193339 * r + 0.1191920 * g + 0.9503041 * b,
    )
}

fn linearize(v: u8) -> f64 {
    let c = v as f64 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
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
    h: u32,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    idx: u16,
    filled: bool,
) {
    // 盒子先夹到画布内再进内层循环。
    let (xa, xb) = clamp_span(x0, x1, w);
    let (ya, yb) = clamp_span(y0, y1, h);
    for y in ya..=yb {
        for x in xa..=xb {
            let edge = x == xa || x == xb || y == ya || y == yb;
            if filled || edge {
                cel.set(w, x, y, idx);
            }
        }
    }
}

/// 把一维区间夹到 `0..=extent-1`，并保证 `lo <= hi`。
/// Lua 侧 circle 的上下界是 `cx-r`/`cx+r`，cx<r 时负数强转 u32 会翻成
/// 40 亿，不夹界的话下面那双层循环就没了尽头。
fn clamp_span(a: u32, b: u32, extent: u32) -> (u32, u32) {
    let (lo, hi) = (a.min(b), a.max(b));
    let top = extent.max(1) - 1;
    (lo.min(top), hi.min(top))
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
    let (xa, xb) = (
        xa.min(w.saturating_sub(1) as f64),
        xb.min(w.saturating_sub(1) as f64),
    );
    let (ya, yb) = (
        ya.min(h.saturating_sub(1) as f64),
        yb.min(h.saturating_sub(1) as f64),
    );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_match_uses_perceived_distance() {
        let palette = vec![Rgba::rgb(0, 0, 0), Rgba::rgb(255, 255, 255)];
        // 裸 RGB 会把红色判成黑，感知距离判成白。
        assert_eq!(nearest_index(&palette, Rgba::rgb(255, 0, 0)), Some(1));
        assert_eq!(nearest_index(&palette, Rgba::rgb(0, 0, 255)), Some(0));
    }

    #[test]
    fn lab_matches_published_reference_points() {
        // sRGB 红/绿/蓝的 CIELAB 基准值（D65）。
        let (l, a, b) = lab(Rgba::rgb(255, 0, 0));
        assert!((l - 53.24).abs() < 0.2 && (a - 80.09).abs() < 0.2 && (b - 67.20).abs() < 0.3);
        let (l, a, b) = lab(Rgba::rgb(255, 255, 255));
        assert!((l - 100.0).abs() < 0.01 && a.abs() < 0.01 && b.abs() < 0.01);
        let (l, a, b) = lab(Rgba::rgb(0, 0, 0));
        assert!(l.abs() < 0.01 && a.abs() < 0.01 && b.abs() < 0.01);
    }
}
