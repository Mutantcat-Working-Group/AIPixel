// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! PNG 导出：文档拼接后的 RGBA 位图（帧顺序横向铺开多帧时纵向堆叠）。

use super::document::Document;
use image::{ImageBuffer, Rgba};

/// 拼接后位图：所有帧沿横向排列，每个帧是该帧全部可见图层的合成。
pub fn flatten(doc: &Document) -> ImageBuffer<Rgba<u8>, Vec<u8>> {
    let frame_w = doc.width;
    let frame_h = doc.height;
    let cols = doc.frames.len() as u32;
    let mut img = ImageBuffer::new(frame_w * cols.max(1), frame_h);
    for (fi, _frame) in doc.frames.iter().enumerate() {
        let ox = fi as u32 * frame_w;
        for y in 0..frame_h {
            for x in 0..frame_w {
                let color = composite_pixel(doc, fi as u32, x, y);
                img.put_pixel(ox + x, y, color);
            }
        }
    }
    img
}

/// 单帧合成（按图层顺序，跳过隐藏层，按 opacity 混合）。
pub fn composite_frame(doc: &Document, frame_index: u32) -> ImageBuffer<Rgba<u8>, Vec<u8>> {
    let mut img = ImageBuffer::new(doc.width, doc.height);
    for y in 0..doc.height {
        for x in 0..doc.width {
            img.put_pixel(x, y, composite_pixel(doc, frame_index, x, y));
        }
    }
    img
}

/// 按图层顺序把一格的可见部分合成成最终 RGBA。
///
/// 循环里三个 continue 各挡一种情况：索引 0 是透明槽而不是一个颜色，
/// 拿它当颜色画会把整张画布糊成 palette[0]；图层缺席的 cel 也跳过
/// （图层可以先有 frame、后补 cel，缺席等于那里没有东西）；越界帧返回
/// 全透明而不是 panic——导出时少一帧不该让整个软件崩掉。
///
/// 累加走 f32 预乘：opacity 和 alpha 都是分数，按整数除会把半透明边缘
/// 提前压暗，网页那侧看到的图就和画布里对不上了。
fn composite_pixel(doc: &Document, frame_index: u32, x: u32, y: u32) -> Rgba<u8> {
    let frame = match doc.frames.get(frame_index as usize) {
        Some(f) => &f.id,
        None => return Rgba([0, 0, 0, 0]),
    };
    let mut dst = [0f32, 0f32, 0f32, 0f32];
    for layer in &doc.layers {
        if !layer.visible {
            continue;
        }
        let cel = match doc.cel(&layer.id, frame) {
            Some(c) => c,
            None => continue,
        };
        let idx = cel.get(doc.width, x, y).unwrap_or(0);
        if idx == 0 {
            continue;
        }
        let color = match doc.color_of(idx) {
            Some(c) => c,
            None => continue,
        };
        let alpha = (color.a as f32 / 255.0) * (layer.opacity as f32 / 255.0);
        blend(
            &mut dst,
            [color.r as f32, color.g as f32, color.b as f32],
            alpha,
        );
    }
    Rgba([
        dst[0] as u8,
        dst[1] as u8,
        dst[2] as u8,
        (dst[3] * 255.0) as u8,
    ])
}

/// 源上架预乘：dst 已经是「底 + 已blend」的预乘状态，a 进来按标准
/// over 公式叠上去。预乘后再除回来，半透明边缘才不会按背景提前变暗。
fn blend(dst: &mut [f32; 4], src: [f32; 3], a: f32) {
    let out_a = a + dst[3] * (1.0 - a);
    if out_a <= 0.0 {
        return;
    }
    for i in 0..3 {
        dst[i] = (src[i] * a + dst[i] * dst[3] * (1.0 - a)) / out_a;
    }
    dst[3] = out_a;
}

/// PNG 字节。
pub fn encode_png(img: &ImageBuffer<Rgba<u8>, Vec<u8>>) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut buf);
    img.write_to(&mut cursor, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(buf)
}

/// 整份文档的 PNG：多帧横向铺开。「保存这张图」想要的是能一眼看全所有帧的
/// 那一张，所以默认走 flatten，而不是只导第一帧。
pub fn document_to_png(doc: &Document) -> Result<Vec<u8>, String> {
    encode_png(&flatten(doc))
}

/// 文档 -> data URL（前端 <img> 直接用）。
pub fn document_to_data_url(doc: &Document) -> Result<String, String> {
    let bytes = document_to_png(doc)?;
    Ok(format!("data:image/png;base64,{}", base64_encode(&bytes)))
}

// 自带 base64：data URL 只在这里用一次，不值得为一个函数引一个 crate。
// 尾块不足 3 字节时按 RFC 4648 补 '='，缺的字节当 0 参与移位、不参与输出。
const B64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// base64 编码。`data:` URL 只需要这一处，为它引一个 crate 不划算。
pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = |i: usize| *chunk.get(i).unwrap_or(&0) as usize;
        out.push(B64[b(0) >> 2] as char);
        out.push(B64[((b(0) & 0x03) << 4) | (b(1) >> 4)] as char);
        out.push(if chunk.len() > 1 {
            B64[((b(1) & 0x0f) << 2) | (b(2) >> 6)] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[b(2) & 0x3f] as char
        } else {
            '='
        });
    }
    out
}
