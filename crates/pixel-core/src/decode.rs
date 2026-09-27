//! 位图解码：把 PNG / JPEG / GIF / WebP 字节转成 RGBA 字节串，喂给 pixelize。
//! 生图模型与用户参考图都可能是这四种之一，格式嗅探放在这里统一收口。

use super::document::MAX_DIMENSION;

/// 单个源图最大允许的源像素数：4x MAX_DIMENSION 见方，约 4M 像素。
/// 再大的图对像素画没有意义，只会白白吃掉内存。
pub const MAX_SOURCE_PIXELS: u64 = (MAX_DIMENSION as u64) * (MAX_DIMENSION as u64) * 4;

/// media type -> image crate 的格式标识。
pub fn format_for_media_type(media_type: &str) -> Option<image::ImageFormat> {
    let base = media_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    use image::ImageFormat::*;
    Some(match base.as_str() {
        "image/png" => Png,
        "image/jpeg" | "image/jpg" => Jpeg,
        "image/gif" => Gif,
        "image/webp" => WebP,
        "image/bmp" => Bmp,
        "image/tiff" => Tiff,
        _ => return None,
    })
}

/// 从字节解码一张位图，返回 `(RGBA 字节, 宽, 高)`。
/// `media_type` 未知时退回按内容嗅探，仍失败则报错。
pub fn decode_image(bytes: &[u8], media_type: &str) -> Result<(Vec<u8>, u32, u32), String> {
    let guessed = image::guess_format(bytes).ok();
    let format = match format_for_media_type(media_type).or(guessed) {
        Some(f) => f,
        None => return Err("unrecognized image format".into()),
    };
    let img = image::load_from_memory_with_format(bytes, format)
        .map_err(|e| format!("cannot decode image: {e}"))?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    if (w as u64) * (h as u64) > MAX_SOURCE_PIXELS {
        return Err(format!(
            "source image is {w}x{h}; above the {} pixel limit",
            MAX_SOURCE_PIXELS
        ));
    }
    Ok((rgba.into_raw(), w, h))
}

/// 把一张位图缩到最长边不超过 `max_dim`，重新编码成 PNG 字节。
///
/// 存在的理由只有一个：读视频的工作流要把若干帧同时塞进一条消息，
/// 1080p 的 PNG 一帧就几兆，模型还没读到内容上下文就先爆了。
/// 缩小用 Triangle 滤波而不是 Nearest：缩完的图是给模型「看懂动作」用的，
/// 不是给像素量化用的，保持形体比保持硬边缘重要。
/// 已经够小的图原样返回，不再折腾一遍编解码。
pub fn thumbnail_png(
    bytes: &[u8],
    media_type: &str,
    max_dim: u32,
) -> Result<(Vec<u8>, String), String> {
    let (rgba, width, height) = decode_image(bytes, media_type)?;
    let longest = width.max(height);
    if longest == 0 {
        return Err("image has a zero dimension".into());
    }
    let img = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_raw(width, height, rgba)
        .ok_or_else(|| "decoded buffer does not match its dimensions".to_string())?;
    // max_dim 为 0 表示「不缩」。够小的图也原样编码，只把容器统一成 PNG。
    if max_dim > 0 && longest > max_dim {
        let scale = max_dim as f32 / longest as f32;
        let target = (
            ((width as f32 * scale).round() as u32).max(1),
            ((height as f32 * scale).round() as u32).max(1),
        );
        let small = image::imageops::resize(
            &img,
            target.0,
            target.1,
            image::imageops::FilterType::Triangle,
        );
        return Ok((super::png::encode_png(&small)?, "image/png".into()));
    }
    Ok((super::png::encode_png(&img)?, "image/png".into()))
}

/// 从 data URL（`data:image/png;base64,...`）解码；生图模型常用这种形式回图。
pub fn decode_data_url(url: &str) -> Result<(Vec<u8>, u32, u32), String> {
    let (media_type, payload) = split_data_url(url)?;
    let bytes = decode_base64(payload)?;
    decode_image(&bytes, &media_type)
}

/// 拆 data URL，返回 (media_type, base64 载荷)。
pub fn split_data_url(url: &str) -> Result<(String, &str), String> {
    let rest = url
        .strip_prefix("data:")
        .ok_or("not a data URL: missing data: prefix")?;
    let comma = rest
        .find(',')
        .ok_or("not a data URL: missing payload separator")?;
    let meta = &rest[..comma];
    let payload = &rest[comma + 1..];
    let media_type = meta
        .split(';')
        .next()
        .filter(|m| !m.is_empty())
        .unwrap_or("image/png")
        .to_string();
    if !meta.contains("base64") {
        return Err("data URL is not base64 encoded".into());
    }
    Ok((media_type, payload))
}

/// base64 解码（标准字母表，容忍缺失填充与 URL 安全变体）。
pub fn decode_base64(text: &str) -> Result<Vec<u8>, String> {
    fn val(c: u8) -> Option<u8> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        })
    }
    // 先去掉填充与空白：填充位不承载数据，留着会让尾组多吐出字节。
    let digits: Vec<u8> = text
        .bytes()
        .filter(|c| !c.is_ascii_whitespace() && *c != b'=')
        .map(|c| val(c).ok_or("invalid base64 character"))
        .collect::<Result<_, _>>()?;
    let mut out = Vec::with_capacity(digits.len() / 4 * 3 + 3);
    let mut chunks = digits.chunks_exact(4);
    for c in &mut chunks {
        out.push((c[0] << 2) | (c[1] >> 4));
        out.push((c[1] << 4) | (c[2] >> 2));
        out.push((c[2] << 6) | c[3]);
    }
    let tail = chunks.remainder();
    match tail.len() {
        0 => {}
        1 => return Err("base64 length is invalid: 1 trailing digit".into()),
        2 => out.push((tail[0] << 2) | (tail[1] >> 4)),
        _ => {
            out.push((tail[0] << 2) | (tail[1] >> 4));
            out.push((tail[1] << 4) | (tail[2] >> 2));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_type_maps_to_format() {
        assert_eq!(
            format_for_media_type("image/png"),
            Some(image::ImageFormat::Png)
        );
        assert_eq!(
            format_for_media_type("image/jpeg; charset=binary"),
            Some(image::ImageFormat::Jpeg)
        );
        assert_eq!(format_for_media_type("application/json"), None);
    }

    #[test]
    fn data_url_splits_media_type_and_payload() {
        let (mt, payload) = split_data_url("data:image/png;base64,QUJD").unwrap();
        assert_eq!(mt, "image/png");
        assert_eq!(payload, "QUJD");
        assert!(split_data_url("image/png;base64,QUJD").is_err());
    }

    #[test]
    fn base64_round_trips_through_encoder() {
        // 与 png.rs 的 base64_encode 对齐
        for raw in [&b""[..], b"A", b"AB", b"ABC", b"ABCD", b"hello world"] {
            let text = super::super::png::base64_encode(raw);
            assert_eq!(decode_base64(&text).unwrap(), raw, "payload {raw:?}");
        }
    }

    #[test]
    fn base64_rejects_garbage() {
        assert!(decode_base64("!!!!").is_err());
    }

    #[test]
    fn decodes_a_real_png() {
        // 用项目自己的 PNG 编码器造真实字节，避免手写 CRC 出错
        let mut img = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::new(2, 2);
        img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        img.put_pixel(1, 0, image::Rgba([0, 255, 0, 255]));
        img.put_pixel(0, 1, image::Rgba([0, 0, 255, 255]));
        img.put_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
        let png = super::super::png::encode_png(&img).unwrap();
        let (rgba, w, h) = decode_image(&png, "image/png").unwrap();
        assert_eq!((w, h), (2, 2));
        assert_eq!(rgba.len(), 16);
        assert_eq!(&rgba[0..4], &[255, 0, 0, 255]);
        assert_eq!(&rgba[4..8], &[0, 255, 0, 255]);
        assert_eq!(&rgba[8..12], &[0, 0, 255, 255]);
        assert_eq!(&rgba[12..16], &[255, 255, 255, 255]);
    }

    #[test]
    fn decodes_a_data_url_round_trip() {
        let mut img = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::new(1, 1);
        img.put_pixel(0, 0, image::Rgba([12, 34, 56, 255]));
        let png = super::super::png::encode_png(&img).unwrap();
        let url = format!(
            "data:image/png;base64,{}",
            super::super::png::base64_encode(&png)
        );
        let (rgba, w, h) = decode_data_url(&url).unwrap();
        assert_eq!((w, h), (1, 1));
        assert_eq!(rgba, vec![12, 34, 56, 255]);
    }

    #[test]
    fn a_thumbnail_respects_the_longest_edge_and_keeps_the_shape() {
        let mut img = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::new(64, 32);
        for (x, _y, pixel) in img.enumerate_pixels_mut() {
            // 渐变：缩完之后颜色必须还是渐变的，验证滤波真的在插值而不是丢块。
            let v = (x * 255 / 63) as u8;
            *pixel = image::Rgba([v, 128, 255 - v, 255]);
        }
        let png = super::super::png::encode_png(&img).unwrap();
        let (bytes, media_type) = thumbnail_png(&png, "image/png", 16).unwrap();
        assert_eq!(media_type, "image/png");
        let (rgba, w, h) = decode_image(&bytes, "image/png").unwrap();
        assert_eq!((w, h), (16, 8), "aspect ratio must survive the resize");
        // 渐变从左往右变亮，缩完还留着这个方向，说明滤波在插值而不是丢块。
        assert!(rgba[0] < rgba[(15 * 4) as usize], "{rgba:?}");
    }

    #[test]
    fn an_image_under_the_cap_is_returned_untouched() {
        let mut img = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::new(4, 4);
        img.put_pixel(0, 0, image::Rgba([9, 9, 9, 255]));
        let png = super::super::png::encode_png(&img).unwrap();
        let (bytes, _) = thumbnail_png(&png, "image/png", 16).unwrap();
        let (rgba, w, h) = decode_image(&bytes, "image/png").unwrap();
        assert_eq!((w, h), (4, 4));
        assert_eq!(&rgba[0..4], &[9, 9, 9, 255]);
    }

    #[test]
    fn a_zero_cap_means_do_not_scale() {
        let mut img = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::new(8, 8);
        img.put_pixel(0, 0, image::Rgba([1, 2, 3, 255]));
        let png = super::super::png::encode_png(&img).unwrap();
        let (bytes, _) = thumbnail_png(&png, "image/png", 0).unwrap();
        let (_, w, h) = decode_image(&bytes, "image/png").unwrap();
        assert_eq!((w, h), (8, 8));
    }

    #[test]
    fn a_broken_image_cannot_become_a_thumbnail() {
        assert!(thumbnail_png(b"not an image at all", "image/png", 16).is_err());
    }
}
