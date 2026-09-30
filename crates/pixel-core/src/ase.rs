// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! Aseprite .ase / .aseprite 导出。字段布局逐字节对齐官方规范
//! (`ase-file-specs.md`)：RGBA 位深、唯一图层、每帧一张 zlib 压缩 cel。
//! 逐帧工具如果导不出 .ase，图层/帧语义就断在门外，所以这里和 GIF /
//! spritesheet 同级。
//!
//! 结构：
//! - 128 字节文件头（magic 0xA5E0）
//! - 每帧：16 字节帧头（magic 0xF1FA）+ chunk
//! - chunk：4 字节长度 + 2 字节类型 + 负载
//!   - 0x2004 layer（仅首帧，定义唯一图层）
//!   - 0x2005 cel（zlib 压缩位图，cel type 2）
//!
//! 帧时长写在帧头 WORD 里；单帧文档也合法（Aseprite 打开即一张静态图）。

use std::io::Write;

use flate2::{write::ZlibEncoder, Compression};

use super::document::Document;
use super::png::composite_frame;

const ASE_MAGIC: u16 = 0xA5E0;
const FRAME_MAGIC: u16 = 0xF1FA;
const CHUNK_LAYER: u16 = 0x2004;
const CHUNK_CEL: u16 = 0x2005;
const HEADER_LEN: usize = 128;
/// 帧头：4 size + 2 magic + 2 chunks（旧）+ 2 duration + 2 保留 + 4 chunks（新）。
const FRAME_HEADER_LEN: usize = 16;
const LAYER_NAME: &[u8] = b"Layer 1";

/// 文档 -> .ase 字节。空文档也能导出一个合法的空壳，不 panic。
pub fn encode_ase(doc: &Document) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    // 文件大小最后回填：先把位置占住。
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&ASE_MAGIC.to_le_bytes());
    out.extend_from_slice(&(doc.frames.len() as u16).to_le_bytes());
    out.extend_from_slice(&(doc.width as u16).to_le_bytes());
    out.extend_from_slice(&(doc.height as u16).to_le_bytes());
    // 32 = RGBA。不选 8 位索引：调色板可能给不出全部用到的颜色。
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // flags
    out.extend_from_slice(&0u16.to_le_bytes()); // speed（已废弃，时长在帧头）
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.push(0); // palette entry：透明色索引（仅索引模式有效）
    out.extend_from_slice(&[0u8; 3]); // 规范：忽略这 3 字节
    out.extend_from_slice(&0u16.to_le_bytes()); // 颜色数（0 = 256，仅旧索引精灵）
    out.push(1); // 像素宽高比 1:1
    out.push(1);
    out.extend_from_slice(&0i16.to_le_bytes()); // grid x/y
    out.extend_from_slice(&0i16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // grid 宽高
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&[0u8; 84]); // 保留给未来字段
    debug_assert_eq!(out.len(), HEADER_LEN);

    for (fi, frame) in doc.frames.iter().enumerate() {
        let img = composite_frame(doc, fi as u32);
        let pixels = img.into_raw(); // Rgba<u8> 的原始字节就是无预乘 RGBA
        let mut chunks = Vec::new();
        if fi == 0 {
            chunks.extend_from_slice(&layer_chunk());
        }
        chunks.extend_from_slice(&cel_chunk(doc.width, doc.height, &pixels)?);
        let chunk_count: u16 = if fi == 0 { 2 } else { 1 };
        out.extend_from_slice(&((FRAME_HEADER_LEN + chunks.len()) as u32).to_le_bytes());
        out.extend_from_slice(&FRAME_MAGIC.to_le_bytes());
        out.extend_from_slice(&chunk_count.to_le_bytes()); // 旧字段
                                                           // 帧头只有 u16，超过 65535ms 的停留夹住（文档上限 60s 也到不了）。
        out.extend_from_slice(&(frame.duration_ms.min(u16::MAX as u32) as u16).to_le_bytes());
        out.extend_from_slice(&[0u8; 2]); // 保留
        out.extend_from_slice(&0u32.to_le_bytes()); // 新字段 0 = 用旧字段
        out.extend_from_slice(&chunks);
    }

    let size = out.len() as u32;
    out[0..4].copy_from_slice(&size.to_le_bytes());
    Ok(out)
}

/// 唯一图层：可见、可编辑、普通类型、完全不透明。仅写进首帧，
/// 后续帧的 cel 靠 layer index 0 引用它。
fn layer_chunk() -> Vec<u8> {
    // 负载：flags 到 blend 共 12 字节 + opacity + 3 保留 + 2 namelen + 名字
    let data_len = 18 + LAYER_NAME.len();
    let mut c = Vec::with_capacity(6 + data_len);
    c.extend_from_slice(&(data_len as u32).to_le_bytes());
    c.extend_from_slice(&CHUNK_LAYER.to_le_bytes());
    c.extend_from_slice(&3u16.to_le_bytes()); // flags: 可见 + 可编辑
    c.extend_from_slice(&0u16.to_le_bytes()); // type: 普通图层
    c.extend_from_slice(&0u16.to_le_bytes()); // child level
    c.extend_from_slice(&0u16.to_le_bytes()); // 默认宽（忽略）
    c.extend_from_slice(&0u16.to_le_bytes()); // 默认高（忽略）
    c.extend_from_slice(&0u16.to_le_bytes()); // blend: normal
    c.push(255); // opacity
    c.extend_from_slice(&[0u8; 3]); // 保留
    c.extend_from_slice(&(LAYER_NAME.len() as u16).to_le_bytes()); // STRING: 2 字节长度 + 字节
    c.extend_from_slice(LAYER_NAME);
    c
}

/// 压缩 cel（type 2）：zlib 包裹的 RGBA 位图。raw cel（type 0）规范标为
/// 「unused」，现代 Aseprite 写的就是压缩 cel，这里跟着写。
fn cel_chunk(width: u32, height: u32, pixels: &[u8]) -> Result<Vec<u8>, String> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(pixels)
        .map_err(|e| format!("zlib cel compress failed: {e}"))?;
    let compressed = encoder
        .finish()
        .map_err(|e| format!("zlib cel compress failed: {e}"))?;
    // 负载：2+2+2 layer/xy + 1 opacity + 2 type + 2 z-index + 5 保留
    //       + 2+2 宽高 + 压缩数据
    let data_len = 20 + compressed.len();
    let mut c = Vec::with_capacity(6 + data_len);
    c.extend_from_slice(&(data_len as u32).to_le_bytes());
    c.extend_from_slice(&CHUNK_CEL.to_le_bytes());
    c.extend_from_slice(&0u16.to_le_bytes()); // layer index：唯一的 0 号图层
    c.extend_from_slice(&0i16.to_le_bytes()); // x
    c.extend_from_slice(&0i16.to_le_bytes()); // y
    c.push(255); // opacity
    c.extend_from_slice(&2u16.to_le_bytes()); // cel type: compressed image
    c.extend_from_slice(&0i16.to_le_bytes()); // z-index: 默认图层顺序
    c.extend_from_slice(&[0u8; 5]); // 保留
    c.extend_from_slice(&(width as u16).to_le_bytes());
    c.extend_from_slice(&(height as u16).to_le_bytes());
    c.extend_from_slice(&compressed);
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Rgba;
    use crate::ops::{self, PixelOperation};
    use flate2::read::ZlibDecoder;
    use std::io::Read;

    fn doc() -> Document {
        let mut d = Document::new("t", 4, 2).unwrap();
        ops::apply_one(
            &mut d,
            &PixelOperation::CreateFrame {
                after: Some("F0".into()),
                duration_ms: 100,
                id: None,
            },
        )
        .unwrap();
        d
    }

    fn paint(document: &mut Document, layer: &str, frame: &str, indices: &[u16]) {
        let cel = document.cel_mut(layer, frame).unwrap();
        for (i, v) in indices.iter().enumerate() {
            cel.indices[i] = *v;
        }
    }

    /// 按规范把文件拆回结构，校验长度字段对得上、像素就是合成结果。
    struct Parsed {
        frames: usize,
        width: u16,
        height: u16,
        durations: Vec<u16>,
        cels: Vec<Vec<u8>>,
    }

    fn parse(bytes: &[u8]) -> Parsed {
        let u16_at = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]);
        let u32_at =
            |o: usize| u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
        assert_eq!(u16_at(4), ASE_MAGIC);
        let frames = u16_at(6) as usize;
        assert_eq!(u32_at(0) as usize, bytes.len(), "file size must be exact");
        assert_eq!(u16_at(12), 32, "RGBA depth");
        let width = u16_at(8);
        let height = u16_at(10);
        let mut out = Parsed {
            frames,
            width,
            height,
            durations: Vec::new(),
            cels: Vec::new(),
        };
        let mut o = HEADER_LEN;
        for _ in 0..frames {
            let frame_size = u32_at(o) as usize;
            assert_eq!(u16_at(o + 4), FRAME_MAGIC);
            out.durations.push(u16_at(o + 8));
            // 帧头 10-11 是废弃的 future 字段，保持 0。
            assert_eq!(u16_at(o + 10), 0, "future field must stay zero");
            // 帧头 16 字节后还剩 4 字节「新 chunk 数」字段，必须为 0。
            assert_eq!(u32_at(o + 12), 0, "new chunk count 0 = use the old field");
            let mut p = o + FRAME_HEADER_LEN;
            let frame_end = o + frame_size;
            while p < frame_end {
                let chunk_size = u32_at(p) as usize;
                let chunk_type = u16_at(p + 4);
                if chunk_type == CHUNK_CEL {
                    // cel 固定前缀 16 字节：layer/x/y/opacity/type/z-index/保留
                    // layer/x/y 各 2 字节、opacity 1 字节，type 从第 7 字节起
                    let cel_type = u16_at(p + 6 + 7);
                    assert_eq!(cel_type, 2, "compressed cel");
                    let z_index = u16_at(p + 6 + 9);
                    assert_eq!(z_index as i16, 0, "default z-index");
                    let w = u16_at(p + 6 + 16) as usize;
                    let h = u16_at(p + 6 + 18) as usize;
                    // chunk_size 只数负载，还要越过 6 字节 chunk 头
                    let payload = &bytes[p + 6 + 20..p + 6 + chunk_size];
                    let mut data = Vec::with_capacity(w * h * 4);
                    ZlibDecoder::new(payload)
                        .read_to_end(&mut data)
                        .expect("zlib cel decodes");
                    assert_eq!(data.len(), w * h * 4);
                    out.cels.push(data);
                }
                p += 6 + chunk_size;
            }
            assert_eq!(p, frame_end, "chunks must fill the frame exactly");
            o = frame_end;
        }
        assert_eq!(o, bytes.len(), "frames must fill the file exactly");
        out
    }

    #[test]
    fn header_and_cel_bytes_follow_the_spec() {
        let mut d = doc();
        d.intern_color(Rgba::rgb(255, 0, 0)).unwrap();
        d.intern_color(Rgba::rgb(0, 0, 255)).unwrap();
        paint(&mut d, "L0", "F0", &[1, 1, 0, 1, 1, 0, 1, 1]);
        paint(&mut d, "L0", "F1", &[2, 2, 0, 2, 2, 0, 2, 2]);
        d.frames[1].duration_ms = 250;

        let bytes = encode_ase(&d).unwrap();
        let parsed = parse(&bytes);
        assert_eq!(parsed.frames, 2);
        assert_eq!(parsed.width, 4);
        assert_eq!(parsed.height, 2);
        assert_eq!(parsed.durations, vec![100, 250]);
        assert_eq!(parsed.cels.len(), 2);

        // cel 就是合成帧：透明格 (0,0,0,0)，红色/蓝色按帧不同
        assert_eq!(&parsed.cels[0][..4], &[255, 0, 0, 255]);
        assert_eq!(&parsed.cels[0][8..12], &[0, 0, 0, 0]);
        assert_eq!(&parsed.cels[1][..4], &[0, 0, 255, 255]);
    }

    #[test]
    fn empty_document_still_exports_a_valid_shell() {
        // 1x1 空文档：没有任何颜色，cel 是一格全透明。
        let d = Document::new("empty", 1, 1).unwrap();
        let bytes = encode_ase(&d).unwrap();
        let parsed = parse(&bytes);
        assert_eq!(parsed.frames, 1);
        assert_eq!(parsed.width, 1);
        assert_eq!(parsed.height, 1);
    }
}
