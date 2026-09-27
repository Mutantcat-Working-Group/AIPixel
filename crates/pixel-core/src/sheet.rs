//! 动画导出：GIF 与 spritesheet，和 PNG 导出同级。
//!
//! 逐帧工具如果交不出动画文件，画出来的帧就永远出不了这个软件，所以这里不是附属功能。
//! 帧延时取文档自己的 `duration_ms`，但会兜底抬高：GIF 的延时单位是 1 厘秒（10ms），
//! 而不少查看器把 0 当成「立刻切下一帧」，20ms 以下的设置会被直接闪过去。

use std::time::Duration;

use super::document::{Document, MAX_FRAME_DURATION_MS};
use super::png::composite_frame;
use image::{Delay, Frame, ImageBuffer, Rgba};

/// 单帧最短延时。低于它的设置会被抬到这里，20ms = 2 厘秒，所有查看器都认得。
pub const MIN_FRAME_MS: u32 = 20;

/// spritesheet：帧从左到右排，排满 `columns` 列换行。
///
/// `columns` 为 0 或超过总帧数时排成一行 —— 一行横排的序列向下滚动看最省事，
/// 也是大多数素材库的默认约定。
pub fn spritesheet(doc: &Document, columns: u32) -> ImageBuffer<Rgba<u8>, Vec<u8>> {
    let count = doc.frames.len() as u32;
    if count == 0 {
        return ImageBuffer::new(doc.width, doc.height);
    }
    let cols = if columns == 0 || columns > count {
        count
    } else {
        columns
    };
    let rows = count.div_ceil(cols);
    let mut img = ImageBuffer::new(doc.width * cols, doc.height * rows);
    for fi in 0..count {
        let frame = composite_frame(doc, fi);
        let ox = (fi % cols) * doc.width;
        let oy = (fi / cols) * doc.height;
        for y in 0..frame.height() {
            for x in 0..frame.width() {
                img.put_pixel(ox + x, oy + y, *frame.get_pixel(x, y));
            }
        }
    }
    img
}

/// 文档 -> GIF89a 字节，无限循环。空文档只会得到一个空的 GIF，不会 panic。
pub fn encode_gif(doc: &Document) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        // encoder 借着 out，必须先 drop 再 return，否则所有权还压在缓冲区上。
        let mut encoder = image::codecs::gif::GifEncoder::new_with_speed(&mut out, 10);
        encoder
            .set_repeat(image::codecs::gif::Repeat::Infinite)
            .map_err(|e| e.to_string())?;
        let frames = doc.frames.iter().enumerate().map(|(fi, frame)| {
            let ms = u64::from(frame.duration_ms.clamp(MIN_FRAME_MS, MAX_FRAME_DURATION_MS));
            Frame::from_parts(
                composite_frame(doc, fi as u32),
                0,
                0,
                Delay::from_saturating_duration(Duration::from_millis(ms)),
            )
        });
        encoder.encode_frames(frames).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Rgba;
    use crate::ops::{self, PixelOperation};
    use image::AnimationDecoder;
    use std::io::Cursor;

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

    fn paint(doc: &mut Document, layer: &str, frame: &str, indices: &[u16]) {
        let cel = doc.cel_mut(layer, frame).unwrap();
        for (i, v) in indices.iter().enumerate() {
            cel.indices[i] = *v;
        }
    }

    #[test]
    fn spritesheet_lays_frames_left_to_right_then_wraps() {
        let mut d = doc();
        d.intern_color(Rgba::rgb(255, 0, 0)).unwrap();
        d.intern_color(Rgba::rgb(0, 0, 255)).unwrap();
        paint(&mut d, "L0", "F0", &[1, 1, 1, 1, 1, 1, 1, 1]);
        paint(&mut d, "L0", "F1", &[2, 2, 2, 2, 2, 2, 2, 2]);

        // 两帧、指定 1 列 -> 纵向两行
        let tall = spritesheet(&d, 1);
        assert_eq!(tall.dimensions(), (4, 4));
        assert_eq!(tall.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(tall.get_pixel(0, 2).0, [0, 0, 255, 255]);

        // 0 列 = 排成一行
        let wide = spritesheet(&d, 0);
        assert_eq!(wide.dimensions(), (8, 2));
        assert_eq!(wide.get_pixel(4, 0).0, [0, 0, 255, 255]);

        // columns 超过总帧数也排成一行
        let clamped = spritesheet(&d, 9);
        assert_eq!(clamped.dimensions(), (8, 2));
    }

    #[test]
    fn gif_keeps_every_frame_and_loops_forever() {
        let d = doc();
        let bytes = encode_gif(&d).unwrap();
        assert_eq!(&bytes[..6], b"GIF89a");

        // NETSCAPE2.0 应用扩展就是「无限循环」这个决定本身
        assert!(
            bytes.windows(11).any(|w| w == b"NETSCAPE2.0"),
            "gif should ask viewers to loop forever"
        );

        let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).unwrap();
        let frames = decoder.into_frames().collect_frames().unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].buffer().dimensions(), (4, 2));
    }

    #[test]
    fn short_frame_durations_are_lifted_to_a_visible_delay() {
        let mut d = doc();
        d.frames[0].duration_ms = 5;
        let bytes = encode_gif(&d).unwrap();
        // 回读出来的延时是厘秒，5ms 必须被抬到至少 2 厘秒，不能被记成 0
        let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).unwrap();
        let frames = decoder.into_frames().collect_frames().unwrap();
        let (numer, denom) = frames[0].delay().numer_denom_ms();
        assert!(numer / denom >= MIN_FRAME_MS);
    }
}
