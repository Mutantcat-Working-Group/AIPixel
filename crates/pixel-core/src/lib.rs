//! pixel-core: 像素文档模型、类型化操作、RLE 上下文编码、
//! .aip v2 文本格式（参照 PixTXT 设计，图层/帧一等公民）、Lua 沙箱着色器与 PNG 导出。
//!
//! 设计约束（按本项目自己的取舍）：
//! - 文档是唯一权威状态，文本网格优先于任何位图渲染
//! - 所有变更经过类型化操作并推进 revision（乐观锁基础）
//! - 索引 0 恒为透明，调色板从 0 开始编号

pub mod aip;
pub mod context;
pub mod document;
pub mod ops;
pub mod png;
pub mod rle;
pub mod shader;

pub use document::{Cel, Document, Frame, Layer, Rgba};
pub use ops::{OperationError, PixelOperation};
