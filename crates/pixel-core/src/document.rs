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
            revision: 0,
        })
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
}
