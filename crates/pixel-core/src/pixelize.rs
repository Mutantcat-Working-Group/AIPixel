//! 位图 -> 像素文档：把模型生成的位图降采样、量化成 .aip 的调色板索引网格。
//! 这是「生图模型」工作流的落点：位图只是原料，权威状态永远是文本网格。
//!
//! 三步：按比例面积平均降采样到画布尺寸 -> 中位切分提取主色 -> 感知加权最近色匹配。
//! 全程不依赖 LLM：模型出图后由这里负责「翻译」成像素文档，模型不手写矩阵。

use super::document::{Document, Rgba, MAX_PALETTE};

/// 源图与画布尺寸不一致时如何处理。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FitMode {
    /// 保比例放进画布，剩余区域透明（默认，避免拉伸变形）。
    #[default]
    Contain,
    /// 拉伸铺满画布，忽略源图宽高比。
    Stretch,
}

/// 量化选项。默认值面向「别把调色板撑爆」的常见诉求。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PixelizeOptions {
    /// 从位图里最多提取多少种主色（中位切分上限）。
    pub max_colors: usize,
    /// 命中画布已有调色板的容差：距离小于它的主色直接复用旧色，不新增。
    pub snap_tolerance: u32,
    /// 允许为找不到近似色的主色新增调色板项。
    pub expand_palette: bool,
    /// 在两个最近调色板档位之间做有序抖动（Bayer 4x4）。
    pub dither: bool,
    /// alpha 低于该值的输出像素视为透明（索引 0）。
    pub alpha_threshold: u8,
    /// 宽高比处理方式。
    pub fit: FitMode,
}

impl Default for PixelizeOptions {
    fn default() -> Self {
        PixelizeOptions {
            max_colors: 32,
            snap_tolerance: 12,
            expand_palette: true,
            dither: false,
            alpha_threshold: 128,
            fit: FitMode::Contain,
        }
    }
}

/// 一次量化的结果统计，回给 UI 与模型看。
#[derive(Debug, Clone, PartialEq)]
pub struct PixelizeReport {
    pub fit: FitMode,
    /// 覆盖到的源图区域（x, y, w, h）。
    pub source_rect: (u32, u32, u32, u32),
    /// 覆盖到的画布区域（x, y, w, h）。
    pub dest_rect: (u32, u32, u32, u32),
    pub opaque_pixels: u32,
    pub transparent_pixels: u32,
    /// 新增到文档调色板的颜色数。
    pub palette_added: usize,
    /// 实际写入网格的去重颜色数。
    pub colors_used: usize,
    /// 量化所用的目标调色板：前 `doc.palette.len()` 项是文档原有色，
    /// 其余是本次新增色。索引空间就是这张表的 1 起编号。
    pub palette: Vec<Rgba>,
    /// 输出索引网格（与画布同尺寸，索引已是文档调色板空间），可直接写进 cel。
    pub indices: Vec<u16>,
}

/// 源图区域 -> 画布区域的映射，加上每格对应的源图取样矩形。
struct FitPlan {
    source: (u32, u32, u32, u32),
    dest: (u32, u32, u32, u32),
}

fn resolve_fit(src_w: u32, src_h: u32, dst_w: u32, dst_h: u32, fit: FitMode) -> FitPlan {
    match fit {
        FitMode::Stretch => FitPlan {
            source: (0, 0, src_w, src_h),
            dest: (0, 0, dst_w, dst_h),
        },
        FitMode::Contain => {
            let scale = (dst_w as f64 / src_w as f64).min(dst_h as f64 / src_h as f64);
            // 至少占 1 像素，否则源图会被整个丢掉。
            let dw = ((src_w as f64 * scale).round() as u32).clamp(1, dst_w);
            let dh = ((src_h as f64 * scale).round() as u32).clamp(1, dst_h);
            FitPlan {
                source: (0, 0, src_w, src_h),
                dest: ((dst_w - dw) / 2, (dst_h - dh) / 2, dw, dh),
            }
        }
    }
}

/// 目标像素 (tx, ty) 在源图里的取样矩形；宽高至少 1，放大时靠复刻补足。
fn sample_rect(plan: &FitPlan, src_w: u32, src_h: u32, tx: u32, ty: u32) -> (u32, u32, u32, u32) {
    let (sx, sy, sw, sh) = plan.source;
    let x0 = sx + (tx as u64 * sw as u64 / plan.dest.2 as u64) as u32;
    let x1 = sx + ((tx + 1) as u64 * sw as u64 / plan.dest.2 as u64) as u32;
    let y0 = sy + (ty as u64 * sh as u64 / plan.dest.3 as u64) as u32;
    let y1 = sy + ((ty + 1) as u64 * sh as u64 / plan.dest.3 as u64) as u32;
    let px = x0.min(src_w.saturating_sub(1));
    let py = y0.min(src_h.saturating_sub(1));
    let pw = x1.saturating_sub(x0).max(1).min(src_w - px);
    let ph = y1.saturating_sub(y0).max(1).min(src_h - py);
    (px, py, pw, ph)
}

/// 把 RGBA 位图量化成与 `doc` 同尺寸的调色板索引网格。
/// 返回的网格不含任何文档副作用；写入 cel 由调用方决定，便于事务式使用。
pub fn pixelize_rgba(
    rgba: &[u8],
    src_w: u32,
    src_h: u32,
    doc: &Document,
    opts: &PixelizeOptions,
) -> Result<PixelizeReport, String> {
    if src_w == 0 || src_h == 0 {
        return Err("source image has a zero dimension".into());
    }
    let expected = (src_w as usize)
        .checked_mul(src_h as usize)
        .and_then(|px| px.checked_mul(4))
        .ok_or("source image is too large")?;
    if rgba.len() != expected {
        return Err(format!(
            "expected {expected} bytes for {src_w}x{src_h} RGBA, got {}",
            rgba.len()
        ));
    }

    let plan = resolve_fit(src_w, src_h, doc.width, doc.height, opts.fit);
    let (dx0, dy0, dw, dh) = plan.dest;
    let (sx0, sy0, sw, sh) = plan.source;

    // 面积平均降采样：一个目标像素 = 它覆盖的源图矩形的均值。
    let total = (dw * dh) as usize;
    let mut rgb: Vec<[u8; 3]> = Vec::with_capacity(total);
    let mut alpha: Vec<u8> = Vec::with_capacity(total);
    for ty in 0..dh {
        for tx in 0..dw {
            let (px, py, pw, ph) = sample_rect(&plan, src_w, src_h, tx, ty);
            let mut acc = [0u64; 4];
            let mut n = 0u64;
            for yy in py..(py + ph).min(src_h) {
                for xx in px..(px + pw).min(src_w) {
                    let i = ((yy * src_w + xx) * 4) as usize;
                    acc[0] += rgba[i] as u64;
                    acc[1] += rgba[i + 1] as u64;
                    acc[2] += rgba[i + 2] as u64;
                    acc[3] += rgba[i + 3] as u64;
                    n += 1;
                }
            }
            // n 为 0 只可能是取样矩形空了一片；此时累加器全 0，除以 1 仍是全 0。
            let n = n.max(1);
            rgb.push([(acc[0] / n) as u8, (acc[1] / n) as u8, (acc[2] / n) as u8]);
            alpha.push((acc[3] / n) as u8);
        }
    }

    let palette = build_palette(doc, &rgb, &alpha, opts);
    let palette_added = palette.len().saturating_sub(doc.palette.len());

    // 相同取样色只算一次最近色：1024x1024 画布也不会退化成 画布 x 调色板 的平方级扫描。
    let mut cache: std::collections::HashMap<u32, Nearest> = std::collections::HashMap::new();
    let mut indices = vec![0u16; (doc.width * doc.height) as usize];
    let mut opaque = 0u32;
    let mut transparent = 0u32;
    for ty in 0..dh {
        for tx in 0..dw {
            let s = (ty * dw + tx) as usize;
            let gx = dx0 + tx;
            let gy = dy0 + ty;
            if gx >= doc.width || gy >= doc.height {
                continue;
            }
            let gi = (gy * doc.width + gx) as usize;
            if alpha[s] < opts.alpha_threshold {
                indices[gi] = 0;
                transparent += 1;
                continue;
            }
            let key = rgb_pack(rgb[s]);
            let choice = *cache
                .entry(key)
                .or_insert_with(|| two_nearest(&palette, rgb[s]));
            indices[gi] = choose(choice, gx, gy, opts.dither) as u16;
            opaque += 1;
        }
    }

    let colors_used = indices
        .iter()
        .filter(|i| **i != 0)
        .collect::<std::collections::BTreeSet<_>>()
        .len();

    Ok(PixelizeReport {
        fit: opts.fit,
        source_rect: (sx0, sy0, sw, sh),
        dest_rect: (dx0, dy0, dw, dh),
        opaque_pixels: opaque,
        transparent_pixels: transparent,
        palette_added,
        colors_used,
        palette,
        indices,
    })
}

/// 量化并写入 cel。索引已处于文档调色板空间，直接落盘即可。
pub fn pixelize_into_cel(
    doc: &mut Document,
    layer: &str,
    frame: &str,
    rgba: &[u8],
    src_w: u32,
    src_h: u32,
    opts: &PixelizeOptions,
) -> Result<PixelizeReport, String> {
    let report = pixelize_rgba(rgba, src_w, src_h, doc, opts)?;
    // 新增主色必须真正落进文档调色板，否则网格索引会指向不存在的颜色。
    // report.palette 的前 doc.palette.len() 项与文档一致，只追加尾部增量。
    for color in report.palette.iter().skip(doc.palette.len()) {
        doc.intern_color(*color)
            .map_err(|e| format!("cannot extend palette: {e}"))?;
    }
    let cel = doc
        .cel_mut(layer, frame)
        .ok_or_else(|| format!("unknown cel: {layer}/{frame}"))?;
    if cel.indices.len() != report.indices.len() {
        return Err(format!(
            "cel {layer}/{frame} holds {} index/indices but the canvas needs {}",
            cel.indices.len(),
            report.indices.len()
        ));
    }
    cel.indices.copy_from_slice(&report.indices);
    doc.bump();
    Ok(report)
}

/// 生成目标调色板：文档已有调色板打底，按 snap 容差决定是否追加主色。
/// 调色板为空时必须扩张——没有颜色就画不出任何像素。
pub fn build_palette(
    doc: &Document,
    rgb: &[[u8; 3]],
    alpha: &[u8],
    opts: &PixelizeOptions,
) -> Vec<Rgba> {
    let mut palette = doc.palette.clone();
    let main_colors = median_cut(rgb, alpha, opts.alpha_threshold, opts.max_colors.max(1));
    for color in main_colors {
        let existing = nearest_existing(&palette, color, opts.snap_tolerance);
        // 已经足够接近，或不准扩张：都不新增，让匹配阶段落到旧色上。
        if existing.is_some() || !opts.expand_palette {
            continue;
        }
        if palette.len() >= MAX_PALETTE {
            break;
        }
        palette.push(Rgba::rgb(color[0], color[1], color[2]));
    }
    palette
}

/// 已有调色板里是否存在容差内的近似色。
/// `snap_tolerance` 是 redmean 距离阈值；0 表示只接受完全同色。
fn nearest_existing(palette: &[Rgba], color: [u8; 3], snap_tolerance: u32) -> Option<usize> {
    let target = Rgba::rgb(color[0], color[1], color[2]);
    palette
        .iter()
        .position(|c| color_distance(*c, target) <= snap_tolerance)
}

/// 一个取样色的最近两档调色板索引与距离。索引是文档空间（1 起）。
#[derive(Debug, Clone, Copy)]
struct Nearest {
    best: usize,
    best_d: u32,
    second: usize,
    second_d: u32,
}

fn two_nearest(palette: &[Rgba], color: [u8; 3]) -> Nearest {
    let target = Rgba::rgb(color[0], color[1], color[2]);
    let mut found = Nearest {
        best: 1,
        best_d: u32::MAX,
        second: 1,
        second_d: u32::MAX,
    };
    for (i, c) in palette.iter().enumerate() {
        let d = color_distance(*c, target);
        if d < found.best_d {
            found.second = found.best;
            found.second_d = found.best_d;
            found.best = i + 1;
            found.best_d = d;
        } else if d < found.second_d {
            found.second = i + 1;
            found.second_d = d;
        }
    }
    if palette.is_empty() {
        // 调用方需保证 build_palette 至少产出 1 色；空调色板意味着没有可画颜色。
        found.best = 1;
    }
    found
}

/// 按 Bayer 阈值在最近两档之间取一档。dither 关闭时恒定取最近档。
fn choose(n: Nearest, x: u32, y: u32, dither: bool) -> usize {
    if !dither || n.second_d == u32::MAX || n.second == n.best {
        return n.best;
    }
    // 真实颜色按 best_d/(best_d+second_d) 的比例落在第二档；阈值小于该比例才取第二档。
    let weight = n.best_d as f32 / (n.best_d + n.second_d).max(1) as f32;
    let gate = bayer4((x % 4) as usize, (y % 4) as usize);
    if gate < weight {
        n.second
    } else {
        n.best
    }
}

fn rgb_pack(c: [u8; 3]) -> u32 {
    ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32
}

/// redmean 加权欧氏距离：比朴素 RGB 距离更接近人眼判断。
fn color_distance(a: Rgba, b: Rgba) -> u32 {
    let rmean = (a.r as i32 + b.r as i32) / 2;
    let dr = a.r as i32 - b.r as i32;
    let dg = a.g as i32 - b.g as i32;
    let db = a.b as i32 - b.b as i32;
    let d = (((512 + rmean) * dr * dr) >> 8) + 4 * dg * dg + (((767 - rmean) * db * db) >> 8);
    d.max(0) as u32
}

/// Bayer 4x4 阈值矩阵，归一到 0..1。
fn bayer4(x: usize, y: usize) -> f32 {
    const M: [[u32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    (M[y % 4][x % 4] as f32 + 0.5) / 16.0
}

/// 中位切分：把不透明取样色装进盒子，反复沿最长轴从中位切开，直到够 k 个或切不动。
fn median_cut(rgb: &[[u8; 3]], alpha: &[u8], threshold: u8, k: usize) -> Vec<[u8; 3]> {
    let opaque: Vec<[u8; 3]> = rgb
        .iter()
        .zip(alpha)
        .filter(|(_, a)| **a >= threshold)
        .map(|(c, _)| *c)
        .collect();
    if opaque.is_empty() {
        return Vec::new();
    }
    let mut boxes: Vec<Vec<[u8; 3]>> = vec![opaque];
    while boxes.len() < k {
        let Some(idx) = boxes
            .iter()
            .enumerate()
            .filter(|(_, b)| b.len() >= 2)
            .max_by_key(|(_, b)| weighted_range(b))
        else {
            break;
        };
        let mut split = boxes.swap_remove(idx.0);
        let axis = longest_axis(&split);
        split.sort_by_key(|c| c[axis]);
        let mid = split.len() / 2;
        let right = split.split_off(mid);
        boxes.push(split);
        boxes.push(right);
    }
    boxes.iter().map(|b| average(b)).collect()
}

/// 盒子的加权极差：用彩色盒体积近似，优先切信息量大的盒子。
fn weighted_range(b: &[[u8; 3]]) -> u32 {
    let mut lo = [255u16; 3];
    let mut hi = [0u16; 3];
    for c in b {
        for i in 0..3 {
            lo[i] = lo[i].min(c[i] as u16);
            hi[i] = hi[i].max(c[i] as u16);
        }
    }
    let dr = (hi[0] - lo[0]) as u32;
    let dg = (hi[1] - lo[1]) as u32;
    let db = (hi[2] - lo[2]) as u32;
    dr * dg * db + (dr + dg + db) * 4
}

fn longest_axis(b: &[[u8; 3]]) -> usize {
    let mut lo = [255u16; 3];
    let mut hi = [0u16; 3];
    for c in b {
        for i in 0..3 {
            lo[i] = lo[i].min(c[i] as u16);
            hi[i] = hi[i].max(c[i] as u16);
        }
    }
    let spans = [
        (hi[0] - lo[0]) as u32,
        (hi[1] - lo[1]) as u32,
        (hi[2] - lo[2]) as u32,
    ];
    spans
        .iter()
        .enumerate()
        .max_by_key(|(_, s)| **s)
        .map(|(i, _)| i)
        .unwrap_or(0)
}

fn average(b: &[[u8; 3]]) -> [u8; 3] {
    let n = b.len() as u64;
    if n == 0 {
        return [0, 0, 0];
    }
    let mut acc = [0u64; 3];
    for c in b {
        for i in 0..3 {
            acc[i] += c[i] as u64;
        }
    }
    [(acc[0] / n) as u8, (acc[1] / n) as u8, (acc[2] / n) as u8]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_with(palette: &[[u8; 3]]) -> Document {
        let mut doc = Document::new("t", 8, 8).unwrap();
        for c in palette {
            doc.palette.push(Rgba::rgb(c[0], c[1], c[2]));
        }
        doc
    }

    fn solid(w: u32, h: u32, c: [u8; 3], a: u8) -> (Vec<u8>, u32, u32) {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..(w * h) {
            v.extend_from_slice(&[c[0], c[1], c[2], a]);
        }
        (v, w, h)
    }

    #[test]
    fn pixelize_maps_onto_existing_palette_without_growth() {
        // 源图是纯红，调色板已有近似红：不应新增调色板项。
        let doc = doc_with(&[[200, 0, 0]]);
        let (rgba, w, h) = solid(16, 16, [210, 10, 10], 255);
        let opts = PixelizeOptions {
            max_colors: 4,
            snap_tolerance: 10_000,
            expand_palette: false,
            ..Default::default()
        };
        let report = pixelize_rgba(&rgba, w, h, &doc, &opts).unwrap();
        assert_eq!(report.palette_added, 0);
        assert_eq!(report.opaque_pixels, 64);
        let mut doc2 = doc.clone();
        let r2 = pixelize_into_cel(&mut doc2, "L0", "F0", &rgba, w, h, &opts).unwrap();
        assert_eq!(r2.palette_added, 0);
        assert_eq!(doc2.palette.len(), 1);
        assert!(doc2
            .cel("L0", "F0")
            .unwrap()
            .indices
            .iter()
            .all(|i| *i == 1));
    }

    #[test]
    fn transparent_source_stays_transparent() {
        let doc = doc_with(&[[10, 10, 10]]);
        let (rgba, w, h) = solid(16, 16, [200, 30, 30], 0);
        let report = pixelize_rgba(&rgba, w, h, &doc, &PixelizeOptions::default()).unwrap();
        assert_eq!(report.opaque_pixels, 0);
        assert_eq!(report.transparent_pixels, 64);
        assert_eq!(report.indices, vec![0u16; 64]);
    }

    #[test]
    fn unknown_color_is_added_to_palette() {
        let doc = doc_with(&[[0, 0, 0]]);
        let (rgba, w, h) = solid(8, 8, [12, 200, 90], 255);
        let report = pixelize_rgba(&rgba, w, h, &doc, &PixelizeOptions::default()).unwrap();
        assert_eq!(report.palette_added, 1);
        assert_eq!(report.colors_used, 1);
        let mut doc2 = doc.clone();
        pixelize_into_cel(
            &mut doc2,
            "L0",
            "F0",
            &rgba,
            w,
            h,
            &PixelizeOptions::default(),
        )
        .unwrap();
        assert_eq!(doc2.palette.len(), 2);
        assert!(doc2
            .cel("L0", "F0")
            .unwrap()
            .indices
            .iter()
            .all(|i| *i == 2));
    }

    #[test]
    fn contain_keeps_aspect_and_leaves_margins() {
        // 宽源图放进 8x8 画布：contain 应该只铺满中间一行。
        let doc = doc_with(&[[9, 9, 9]]);
        let (rgba, w, h) = solid(32, 8, [250, 250, 250], 255);
        let report = pixelize_rgba(&rgba, w, h, &doc, &PixelizeOptions::default()).unwrap();
        assert_eq!(report.dest_rect, (0, 3, 8, 2));
        assert_eq!(report.opaque_pixels, 16);
    }

    #[test]
    fn stretch_ignores_aspect() {
        let doc = doc_with(&[[9, 9, 9]]);
        let (rgba, w, h) = solid(32, 8, [250, 250, 250], 255);
        let opts = PixelizeOptions {
            fit: FitMode::Stretch,
            ..Default::default()
        };
        let report = pixelize_rgba(&rgba, w, h, &doc, &opts).unwrap();
        assert_eq!(report.dest_rect, (0, 0, 8, 8));
        assert_eq!(report.opaque_pixels, 64);
    }

    #[test]
    fn rejects_wrong_byte_count() {
        let doc = doc_with(&[]);
        let err = pixelize_rgba(&[0, 0, 0, 0], 4, 4, &doc, &PixelizeOptions::default());
        assert!(err.is_err());
    }

    #[test]
    fn larger_source_is_averaged_down() {
        // 左半黑右半白的 16x16 源图，contain 到 8x8 = 铺满，x<4 黑 x>=4 白。
        let doc = doc_with(&[[0, 0, 0], [255, 255, 255]]);
        let mut rgba = Vec::new();
        for _ in 0..16u32 {
            for x in 0..16u32 {
                if x < 8 {
                    rgba.extend_from_slice(&[0, 0, 0, 255]);
                } else {
                    rgba.extend_from_slice(&[255, 255, 255, 255]);
                }
            }
        }
        let opts = PixelizeOptions {
            snap_tolerance: 0,
            expand_palette: false,
            ..Default::default()
        };
        let report = pixelize_rgba(&rgba, 16, 16, &doc, &opts).unwrap();
        assert_eq!(report.dest_rect, (0, 0, 8, 8));
        // 每个目标像素只覆盖一种源色，因此左右半应分别命中索引 1 / 2。
        for (i, idx) in report.indices.iter().enumerate() {
            let expect = if i % 8 < 4 { 1 } else { 2 };
            assert_eq!(*idx as usize, expect, "pixel {i}");
        }
    }

    #[test]
    fn bayer_thresholds_stay_in_unit_range() {
        for y in 0..8 {
            for x in 0..8 {
                let v = bayer4(x, y);
                assert!((0.0..1.0).contains(&v), "bayer({x},{y}) = {v}");
            }
        }
    }
}
