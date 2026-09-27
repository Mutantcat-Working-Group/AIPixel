//! pixel-core: 像素文档模型、类型化操作、RLE 上下文编码、
//! .aip v2 文本格式（参照 PixTXT 设计，图层/帧一等公民）、Lua 沙箱着色器，
//! 以及 PNG 序列 / spritesheet / GIF 导出。
//!
//! 设计约束（来自前作逆向 + 三份参照）：
//! - 文档是唯一权威状态，文本网格优先于任何位图渲染
//! - 所有变更经过类型化操作并推进 revision（乐观锁基础）
//! - 索引 0 恒为透明，调色板从 0 开始编号

pub mod aip;
pub mod context;
pub mod decode;
pub mod document;
pub mod ops;
pub mod pixelize;
pub mod png;
pub mod rle;
pub mod shader;
pub mod sheet;
pub mod tween;

pub use document::{Cel, Document, Frame, Layer, Rgba};
pub use ops::{OperationError, PixelOperation};
pub use pixelize::{pixelize_rgba, PixelizeOptions, PixelizeReport};
