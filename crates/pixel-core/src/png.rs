//! PNG 导出：文档拼接后的 RGBA 位图（帧顺序横向铺开多帧时纵向堆叠）。

use super::document::{Cel, Document};
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

pub fn document_to_png(doc: &Document) -> Result<Vec<u8>, String> {
    encode_png(&flatten(doc))
}

/// 文档 -> data URL（前端 <img> 直接用）。
pub fn document_to_data_url(doc: &Document) -> Result<String, String> {
    let bytes = document_to_png(doc)?;
    Ok(format!("data:image/png;base64,{}", base64_encode(&bytes)))
}

const B64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

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

/// 单帧 cels 的 PNG（测试/调试用）。
pub fn cel_to_png(cel: &Cel, width: u32, height: u32) -> Vec<u8> {
    let mut img = ImageBuffer::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let v = cel.get(width, x, y).unwrap_or(0);
            let _ = v;
            img.put_pixel(x, y, Rgba([0, 0, 0, 0]));
        }
    }
    encode_png(&img).unwrap_or_default()
}
