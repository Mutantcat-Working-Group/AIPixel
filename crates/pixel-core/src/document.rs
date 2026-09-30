// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_DIMENSION: u32 = 1024;
pub const MAX_LAYERS: usize = 128;
pub const MAX_FRAMES: usize = 2000;
pub const MAX_PALETTE: usize = 256;
pub const MAX_FRAME_DURATION_MS: u32 = 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const TRANSPARENT: Rgba = Rgba {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };

    pub fn rgb(r: u8, g: u8, b: u8) -> Self {
        Rgba { r, g, b, a: 255 }
    }

    pub fn parse_hex(text: &str) -> Option<Self> {
        let t = text.trim().trim_start_matches('#');
        let parse = |s: &str, shift: u32| u8::from_str_radix(s, 16).ok().map(|v| v << shift);
        match t.len() {
            6 => Some(Rgba {
                r: parse(&t[0..2], 0)?,
                g: parse(&t[2..4], 0)?,
                b: parse(&t[4..6], 0)?,
                a: 255,
            }),
            8 => Some(Rgba {
                r: parse(&t[0..2], 0)?,
                g: parse(&t[2..4], 0)?,
                b: parse(&t[4..6], 0)?,
                a: parse(&t[6..8], 0)?,
            }),
            _ => None,
        }
    }

    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }

    pub fn to_rgba_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Layer {
    pub id: String,
    pub name: String,
    pub visible: bool,
    pub opacity: u8,
    /// 这一图层的配色范围指向文档里的哪一套命名调色板。
    /// 悬空 id 由 `ensure_palette_scope` 兜底，别直接当必然存在。
    #[serde(default = "missing_palette_id")]
    pub palette_id: String,
    /// 锁住 = 只许用 `palette_id` 那套范围里的颜色，越界颜色就近归队；
    /// 解开 = 这一层可以随便扩色，新颜色并入它的范围。
    /// 新建文档默认解开：随便取色、随便画，要收敛的人自己上锁。
    /// 上锁才是「只能用这套范围」的语义，默认锁死会把自由涂色的人全得罪。
    #[serde(default = "unlocked")]
    pub locked: bool,
}

/// 命名调色板：配色范围的一等公民。内置那几套改不得，用户想改就先复制一份。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedPalette {
    pub id: String,
    pub name: String,
    pub colors: Vec<Rgba>,
    #[serde(default)]
    pub builtin: bool,
}

/// 新图层默认落在哪套范围上。Sweetie 16 是知名度最高的一套，先当默认不亏。
pub const DEFAULT_PALETTE_ID: &str = "sweetie16";

fn missing_palette_id() -> String {
    MISSING_PALETTE_ID.to_string()
}

/// 图层没带 palette_id（老文档）时的占位。迁移时一律重指到默认范围。
pub const MISSING_PALETTE_ID: &str = "__unset__";

fn unlocked() -> bool {
    // 默认解锁：文档保持「取色自由」，范围面板里的预设只是建议起点。
    false
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frame {
    pub id: String,
    pub duration_ms: u32,
}

/// 一个 cel 是 layer x frame 交叉点上的调色板索引网格，索引 0 = 透明。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cel {
    pub indices: Vec<u16>,
}

impl Cel {
    pub fn new(width: u32, height: u32) -> Self {
        Cel {
            indices: vec![0u16; (width * height) as usize],
        }
    }

    pub fn get(&self, width: u32, x: u32, y: u32) -> Option<u16> {
        let w = width as usize;
        let y = y as usize;
        let x = x as usize;
        let height = self.indices.len().checked_div(w.max(1))?;
        if x >= w || y >= height {
            return None;
        }
        Some(self.indices[y * w + x])
    }

    pub fn set(&mut self, width: u32, x: u32, y: u32, value: u16) -> bool {
        let w = width as usize;
        let idx = y as usize * w + x as usize;
        if idx >= self.indices.len() {
            return false;
        }
        self.indices[idx] = value;
        true
    }

    /// 按新宽高重排格子：左上角锚定，装得下的原样搬过来，装不下的丢掉。
    /// 新露出来的区域是透明格（索引 0）。
    pub fn resize(&mut self, old_width: u32, old_height: u32, width: u32, height: u32) {
        if old_width == width && old_height == height {
            return;
        }
        let old_w = old_width as usize;
        let rows = old_height as usize;
        let keep_rows = rows.min(height as usize);
        let keep_cols = old_w.min(width as usize);
        let mut next = vec![0u16; (width * height) as usize];
        for y in 0..keep_rows.min(self.indices.len().div_ceil(old_w.max(1))) {
            let from = y * old_w..y * old_w + keep_cols;
            let to = y * (width as usize)..y * (width as usize) + keep_cols;
            next[to].copy_from_slice(&self.indices[from]);
        }
        self.indices = next;
    }
}

/// 像素文档。所有修改必须经由 `ops::apply`，以维护 revision。
/// cels 按 layer -> frame 嵌套：既能精确寻址，也能序列化成 JSON 对象
/// 跨 Tauri 边界传给前端（`HashMap<(String, String), _>` 的元组键会被 serde_json 拒绝）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub palette: Vec<Rgba>,
    pub layers: Vec<Layer>,
    pub frames: Vec<Frame>,
    pub cels: BTreeMap<String, BTreeMap<String, Cel>>,
    /// 文档里的命名调色板库：内置 + 用户自建。图层按 palette_id 认领范围。
    #[serde(default)]
    pub palettes: Vec<NamedPalette>,
    pub revision: u64,
}

/// 文档级约束违反。
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DocumentError {
    #[error("canvas dimension must be 1..={MAX_DIMENSION}, got {0}")]
    Dimension(u32),
    #[error("would exceed limits: layers={0}, frames={1}, palette={2}")]
    Limits(usize, usize, usize),
    #[error("unknown layer: {0}")]
    UnknownLayer(String),
    #[error("unknown frame: {0}")]
    UnknownFrame(String),
    #[error("palette index out of range: {0}")]
    PaletteIndex(usize),
    #[error("palette is full ({MAX_PALETTE} entries)")]
    PaletteFull,
    #[error("coordinate out of canvas: ({0},{1})")]
    OutOfCanvas(u32, u32),
}

impl Document {
    /// 新建空文档：空调色板 + 1 个图层 + 1 帧。
    /// 调色板只放真实颜色，透明色恒为索引 0 且不占调色板位置。
    pub fn new(name: impl Into<String>, width: u32, height: u32) -> Result<Self, DocumentError> {
        if width < 1 || height < 1 || width > MAX_DIMENSION || height > MAX_DIMENSION {
            return Err(DocumentError::Dimension(width.max(height)));
        }
        let layer = Layer {
            id: "L0".into(),
            name: "Layer 1".into(),
            visible: true,
            opacity: 255,
            palette_id: DEFAULT_PALETTE_ID.into(),
            locked: false,
        };
        let frame = Frame {
            id: "F0".into(),
            duration_ms: 100,
        };
        let mut cels = BTreeMap::new();
        cels.insert(
            layer.id.clone(),
            [(frame.id.clone(), Cel::new(width, height))]
                .into_iter()
                .collect(),
        );
        Ok(Document {
            name: name.into(),
            width,
            height,
            palette: Vec::new(),
            layers: vec![layer],
            frames: vec![frame],
            cels,
            palettes: crate::palettes::builtin_palettes(),
            revision: 0,
        })
    }

    pub fn layer(&self, id: &str) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id == id)
    }

    /// 某一图层的配色范围。id 悬空时回默认那一套，界面不至于开天窗。
    pub fn layer_palette(&self, layer_id: &str) -> Option<&NamedPalette> {
        let id = self.layer(layer_id).map(|l| l.palette_id.as_str());
        self.palette_by_id(id.unwrap_or(DEFAULT_PALETTE_ID))
    }

    pub fn palette_by_id(&self, id: &str) -> Option<&NamedPalette> {
        self.palettes.iter().find(|p| p.id == id)
    }

    /// 范围仲裁：锁着的层只认范围内的颜色，外来颜色就近归队；没锁就原样放行。
    /// 透明色不参与归队——擦除永远是合法动作。
    pub fn color_for_layer(&self, layer_id: &str, color: Rgba) -> Rgba {
        if color == Rgba::TRANSPARENT {
            return color;
        }
        let Some(layer) = self.layer(layer_id) else {
            return color;
        };
        if !layer.locked {
            return color;
        }
        let Some(range) = self.palette_by_id(&layer.palette_id) else {
            return color;
        };
        if range.colors.contains(&color) {
            return color;
        }
        nearest_color(&range.colors, color).unwrap_or(color)
    }

    /// 兜底清一遍配置：palettes 空了就把内置那套灌回来，图层指向不存在的
    /// 范围就改指默认。读老文件、手改 JSON 之后都得靠这个函数收拾。
    pub fn ensure_palette_scope(&mut self) {
        if self.palettes.is_empty() {
            self.palettes = crate::palettes::builtin_palettes();
        }
        // 默认那套还在就指它，被用户删了就退到库里第一套，总比开天窗强。
        let fallback = if self.palettes.iter().any(|p| p.id == DEFAULT_PALETTE_ID) {
            DEFAULT_PALETTE_ID.to_string()
        } else {
            match self.palettes.first() {
                Some(first) => first.id.clone(),
                None => return,
            }
        };
        // id 清单先取出来：循环里不能再借 self，layers 正被改着呢。
        let ids: Vec<String> = self.palettes.iter().map(|p| p.id.clone()).collect();
        for layer in self.layers.iter_mut() {
            if !ids.contains(&layer.palette_id) {
                layer.palette_id = fallback.clone();
            }
        }
    }

    pub fn palette_index_of(&self, color: Rgba) -> Option<u16> {
        self.palette
            .iter()
            .position(|c| *c == color)
            .map(|i| i as u16 + 1)
    }

    /// 取色，不存在则追加；满则报错。透明色永远映射 0。
    pub fn intern_color(&mut self, color: Rgba) -> Result<u16, DocumentError> {
        if color == Rgba::TRANSPARENT {
            return Ok(0);
        }
        if let Some(i) = self.palette_index_of(color) {
            return Ok(i);
        }
        if self.palette.len() >= MAX_PALETTE {
            return Err(DocumentError::PaletteFull);
        }
        self.palette.push(color);
        Ok(self.palette.len() as u16)
    }

    /// 按 cel 索引取色：0 = 透明，1..=n 对应 palette[0..n-1]。
    pub fn color_of(&self, index: u16) -> Option<Rgba> {
        if index == 0 {
            return Some(Rgba::TRANSPARENT);
        }
        self.palette.get(index as usize - 1).copied()
    }

    pub fn cel(&self, layer: &str, frame: &str) -> Option<&Cel> {
        self.cels.get(layer).and_then(|frames| frames.get(frame))
    }

    pub fn cel_mut(&mut self, layer: &str, frame: &str) -> Option<&mut Cel> {
        self.cels
            .get_mut(layer)
            .and_then(|frames| frames.get_mut(frame))
    }

    /// 原地改过文档之后必须调一次，否则前端的 revision 比对会把新内容当过期结果丢掉。
    pub fn bump(&mut self) {
        self.revision += 1;
    }

    /// 编辑器（src-tauri）直接改文档后也要自查限额，故开放。
    pub fn check_limits(&self) -> Result<(), DocumentError> {
        if self.layers.len() > MAX_LAYERS
            || self.frames.len() > MAX_FRAMES
            || self.palette.len() > MAX_PALETTE
        {
            return Err(DocumentError::Limits(
                self.layers.len(),
                self.frames.len(),
                self.palette.len(),
            ));
        }
        Ok(())
    }

    /// 就近归队到这一层范围里的某个颜色：色不在范围内时用，锁着那一层全靠它兜底。
    pub fn nearest_in_range(&self, layer_id: &str, color: Rgba) -> Option<Rgba> {
        let range = self.layer_palette(layer_id)?;
        nearest_color(&range.colors, color)
    }

    /// 改画布宽高。左上角锚定：已有像素按原位保留，越界部分自然裁掉，
    /// 放大时新区域是透明格。尺寸不合法时整幅不动，报 `DocumentError::Dimension`。
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), DocumentError> {
        if width < 1 || height < 1 || width > MAX_DIMENSION || height > MAX_DIMENSION {
            return Err(DocumentError::Dimension(width.max(height)));
        }
        let (old_width, old_height) = (self.width, self.height);
        if old_width == width && old_height == height {
            return Ok(());
        }
        for layer in self.cels.values_mut() {
            for cel in layer.values_mut() {
                cel.resize(old_width, old_height, width, height);
            }
        }
        self.width = width;
        self.height = height;
        Ok(())
    }
}

/// redmean 加权距离：人眼对绿差敏感、对暗部红差迟钝。
/// 和前端 palette.ts 用同一把尺子，两侧「就近归队」的结果才对得上。
pub fn color_distance(a: Rgba, b: Rgba) -> f64 {
    let r_mean = (a.r as f64 + b.r as f64) / 2.0;
    let dr = a.r as f64 - b.r as f64;
    let dg = a.g as f64 - b.g as f64;
    let db = a.b as f64 - b.b as f64;
    let weight_r = 2.0 + r_mean / 256.0;
    let weight_g = 4.0;
    let weight_b = 2.0 + (255.0 - r_mean) / 256.0;
    dr * dr * weight_r + dg * dg * weight_g + db * db * weight_b
}

/// 候选里最近的那个。同距留在前面，结果才稳定可复现。
pub fn nearest_color(candidates: &[Rgba], target: Rgba) -> Option<Rgba> {
    let mut best: Option<Rgba> = None;
    let mut best_distance = f64::INFINITY;
    for candidate in candidates {
        let distance = color_distance(*candidate, target);
        if distance < best_distance {
            best_distance = distance;
            best = Some(*candidate);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Document {
        let mut d = Document::new("resize", 4, 3).expect("4x3 stays in limits");
        d.intern_color(Rgba::rgb(255, 0, 0)).expect("red interns");
        d
    }

    /// 左上角 (0,0) 附近各画一格，改完宽高要还能找得回来。
    #[test]
    fn resize_keeps_pixels_anchored_at_the_top_left() {
        let mut d = doc();
        d.cel_mut("L0", "F0").expect("cel").set(4, 0, 0, 1);
        d.cel_mut("L0", "F0").expect("cel").set(4, 3, 2, 1);

        d.resize(6, 4).expect("grow is legal");

        assert_eq!((d.width, d.height), (6, 4));
        assert_eq!(d.cels["L0"]["F0"].indices.len(), 24);
        assert_eq!(d.cels["L0"]["F0"].indices[0], 1, "原点那颗跟着搬家");
        assert_eq!(d.cels["L0"]["F0"].indices[2 * 6 + 3], 1, "角落那颗也要留住");
        assert_eq!(
            d.cels["L0"]["F0"].indices[3 * 6 + 5],
            0,
            "新露出来的格子是透明"
        );
    }

    /// 缩小时右下角被裁掉：那不是 bug，是用户把画布改小了。
    #[test]
    fn shrinking_drops_what_falls_outside() {
        let mut d = doc();
        d.cel_mut("L0", "F0").expect("cel").set(4, 3, 2, 1);

        d.resize(2, 2).expect("shrink is legal");

        assert_eq!(d.cels["L0"]["F0"].indices.len(), 4);
        assert_eq!(
            d.cels["L0"]["F0"].indices.iter().sum::<u16>(),
            0,
            "右下角那颗连同它的行一起被裁掉"
        );
    }

    #[test]
    fn resize_rejects_dimensions_outside_the_document_limits() {
        let mut d = doc();
        let before = (d.width, d.height);
        assert!(d.resize(0, 8).is_err(), "0 不是合法宽高");
        assert!(d.resize(MAX_DIMENSION + 1, 8).is_err(), "超过上限也不行");
        assert_eq!((d.width, d.height), before, "被拒的尺寸不许改动文档");
    }
}
