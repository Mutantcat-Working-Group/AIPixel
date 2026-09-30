// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 插帧（in-between）：在两个已有帧之间生成过渡帧。
//!
//! 像素画不能像矢量那样插值坐标，所以三种模式各有分工：
//! - `Copy`：原样复制起点帧。经典占位中间帧，之后交给 Lua 沙箱细化动作。
//! - `Blend`：颜色空间插值。只适合淡入淡出、辉光衰减、幽灵残影这类连续过渡。
//! - `Migrate`：把「起点与终点不同」的像素按顺序逐一翻面。得到形变，
//!   不会像 Blend 那样在两色之间产生浑浊的中间色，这是像素画默认该用的模式。
//!
//! 生成的新帧插在 `to_frame` 之前，因此已有的 `to_frame` 内容不会被破坏。

use super::context;
use super::document::{Cel, Document, Rgba, MAX_PALETTE};
use super::ops::{self, PixelOperation};
use serde::{Deserialize, Serialize};

/// 单次最多生成的插帧数。再多就该让模型分两轮做，而不是一次性摊薄。
pub const MAX_TWEEN_FRAMES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TweenMode {
    /// 复制起点帧：占位中间帧。
    Copy,
    /// 颜色插值：连续的颜色/透明度过渡。
    Blend,
    /// 差异像素顺序迁移：像素画意义上的形变。
    Migrate,
}

/// `Migrate` 模式下差异像素的翻面顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrateOrder {
    /// 自上而下、自左而右扫描。
    Scan,
    /// 从变化区域的质心向外扩散。
    Radial,
    /// 确定性散列顺序，读起来像溶解。
    Scatter,
}

#[derive(Debug, Clone)]
pub struct TweenOptions {
    pub mode: TweenMode,
    pub order: MigrateOrder,
    /// 迁移进度走 smoothstep，避免线性翻面带来的顿感。
    pub ease: bool,
    pub duration_ms: u32,
}

impl Default for TweenOptions {
    fn default() -> Self {
        TweenOptions {
            mode: TweenMode::Migrate,
            order: MigrateOrder::Scan,
            ease: true,
            duration_ms: 83,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TweenReport {
    pub layer: String,
    pub from_frame: String,
    pub to_frame: String,
    /// 新帧 id，按时间顺序。
    pub created: Vec<String>,
    pub changed_pixels: usize,
    pub palette_added: usize,
    pub mode: TweenMode,
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    let v = a as f32 + (b as f32 - a as f32) * t;
    v.round().clamp(0.0, 255.0) as u8
}

/// 非预乘的 rgba 插值。透明像素的 rgb 视为 0，与不透明色插值会自然得到半透明。
fn blend_rgba(a: Rgba, b: Rgba, t: f32) -> Rgba {
    Rgba {
        r: lerp_u8(a.r, b.r, t),
        g: lerp_u8(a.g, b.g, t),
        b: lerp_u8(a.b, b.b, t),
        a: lerp_u8(a.a, b.a, t),
    }
}

/// 起终点不同的像素位置，返回 (x, y)。
fn changed_positions(from: &Cel, to: &Cel, w: u32, h: u32) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if from.get(w, x, y) != to.get(w, x, y) {
                out.push((x, y));
            }
        }
    }
    out
}

/// 确定性散列：同一个 (x, y) 永远得到同一个键，测试可复现。
fn scatter_key(x: u32, y: u32) -> u64 {
    let mut h =
        (x as u64).wrapping_mul(0x9E3779B97F4A7C15) ^ (y as u64).wrapping_mul(0xBF58476D1CE4E5B9);
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58476D1CE4E5B9);
    h ^= h >> 27;
    h
}

fn order_positions(mut positions: Vec<(u32, u32)>, order: MigrateOrder) -> Vec<(u32, u32)> {
    match order {
        MigrateOrder::Scan => {}
        MigrateOrder::Scatter => positions.sort_by_key(|&(x, y)| scatter_key(x, y)),
        MigrateOrder::Radial => {
            let n = positions.len().max(1) as f32;
            let cx = positions.iter().map(|&(x, _)| x as f32).sum::<f32>() / n;
            let cy = positions.iter().map(|&(_, y)| y as f32).sum::<f32>() / n;
            positions.sort_by_key(|&(x, y)| {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                (dx * dx + dy * dy).round() as i64
            });
        }
    }
    positions
}

/// 第 `offset` 个未被占用的帧 id，形如 F3。
/// 一次 tween 要生成多个 id，而帧还没真正插入文档，所以必须靠 offset 保证不自我重复。
fn next_frame_id(doc: &Document, offset: usize) -> String {
    let mut n = doc.frames.len() + offset;
    loop {
        let candidate = format!("F{n}");
        if !doc.frames.iter().any(|f| f.id == candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// 把 `from` 与 `to` 两帧按比例混合后写进 `cel`。
/// 需要的颜色会追加进调色板；调色板已满则报错，绝不静默丢色。
fn write_blended_cel(
    doc: &mut Document,
    cel: &mut Cel,
    from: &Cel,
    to: &Cel,
    w: u32,
    h: u32,
    t: f32,
) -> Result<usize, String> {
    let mut added = 0usize;
    for y in 0..h {
        for x in 0..w {
            let a = from.get(w, x, y).unwrap_or(0);
            let b = to.get(w, x, y).unwrap_or(0);
            if a == b {
                cel.set(w, x, y, a);
                continue;
            }
            let (ca, cb) = match (doc.color_of(a), doc.color_of(b)) {
                (Some(ca), Some(cb)) => (ca, cb),
                _ => return Err("palette index out of range while blending".into()),
            };
            let blended = blend_rgba(ca, cb, t);
            let before = doc.palette.len();
            let idx = doc
                .intern_color(blended)
                .map_err(|e| format!("cannot blend: palette full ({e})"))?;
            if doc.palette.len() != before {
                added += 1;
            }
            cel.set(w, x, y, idx);
        }
    }
    Ok(added)
}

/// 在 `from_frame` 之后插入 `count` 个插帧，内容由 `mode` 决定。
/// `to_frame` 及其原有的时间位置不受影响。
pub fn insert_tween_frames(
    doc: &mut Document,
    layer: &str,
    from_frame: &str,
    to_frame: &str,
    count: usize,
    opts: &TweenOptions,
) -> Result<TweenReport, String> {
    if count == 0 {
        return Err("tween needs at least 1 frame".into());
    }
    if count > MAX_TWEEN_FRAMES {
        return Err(format!(
            "tween asked for {count} frames; the limit is {MAX_TWEEN_FRAMES}"
        ));
    }
    if doc.cel(layer, from_frame).is_none() {
        return Err(format!("unknown layer/frame: {layer}/{from_frame}"));
    }
    if doc.cel(layer, to_frame).is_none() {
        return Err(format!("unknown layer/frame: {layer}/{to_frame}"));
    }
    let (w, h) = (doc.width, doc.height);
    if from_frame == to_frame {
        return Err("from_frame and to_frame must differ".into());
    }

    let from_snapshot = doc.cel(layer, from_frame).unwrap().clone();
    let to_snapshot = doc.cel(layer, to_frame).unwrap().clone();
    let changed = changed_positions(&from_snapshot, &to_snapshot, w, h);

    // Blend 会往调色板里加颜色，先探一遍预算，避免帧建好了才发现装不下。
    if opts.mode == TweenMode::Blend && !changed.is_empty() {
        let mut pairs: Vec<(u16, u16)> = Vec::new();
        for &(x, y) in &changed {
            let pair = (
                from_snapshot.get(w, x, y).unwrap_or(0),
                to_snapshot.get(w, x, y).unwrap_or(0),
            );
            if !pairs.contains(&pair) {
                pairs.push(pair);
            }
        }
        let mut needed: Vec<Rgba> = Vec::new();
        for &(a, b) in &pairs {
            let (ca, cb) = (
                doc.color_of(a).ok_or("palette index out of range")?,
                doc.color_of(b).ok_or("palette index out of range")?,
            );
            for step in 1..=count {
                let t = step as f32 / (count as f32 + 1.0);
                let t = if opts.ease { smoothstep(t) } else { t };
                let c = blend_rgba(ca, cb, t);
                // 只算真正要新增的：调色板里已有的颜色不占预算。
                if !needed.contains(&c) && !doc.palette.contains(&c) {
                    needed.push(c);
                }
            }
        }
        let room = MAX_PALETTE - doc.palette.len();
        if needed.len() > room {
            return Err(format!(
                "blend tween needs {} new palette colors but only {room} slots are free; use fewer frames or mode=migrate",
                needed.len()
            ));
        }
    }

    let mut ops: Vec<PixelOperation> = Vec::with_capacity(count);
    let mut ids: Vec<String> = Vec::with_capacity(count);
    let mut after = from_frame.to_string();
    for offset in 0..count {
        let id = next_frame_id(doc, offset);
        ops.push(PixelOperation::CreateFrame {
            after: Some(after.clone()),
            duration_ms: opts
                .duration_ms
                .clamp(1, super::document::MAX_FRAME_DURATION_MS),
            id: Some(id.clone()),
        });
        after = id.clone();
        ids.push(id);
    }
    ops::apply_batch(doc, &ops).map_err(|e| format!("tween could not create frames: {e}"))?;

    let positions = if opts.mode == TweenMode::Migrate {
        order_positions(changed.clone(), opts.order)
    } else {
        Vec::new()
    };

    let mut palette_added = 0usize;
    for (step, id) in ids.iter().enumerate() {
        let t = (step + 1) as f32 / (count as f32 + 1.0);
        let t = if opts.ease { smoothstep(t) } else { t };
        let base = from_snapshot.clone();
        let mut cel = base;
        match opts.mode {
            TweenMode::Copy => {}
            TweenMode::Blend => {
                palette_added +=
                    write_blended_cel(doc, &mut cel, &from_snapshot, &to_snapshot, w, h, t)?;
            }
            TweenMode::Migrate => {
                let flip = ((positions.len() as f32) * t).round() as usize;
                for &(x, y) in positions.iter().take(flip) {
                    cel.set(w, x, y, to_snapshot.get(w, x, y).unwrap_or(0));
                }
            }
        }
        let slot = doc
            .cel_mut(layer, id)
            .ok_or_else(|| format!("new frame {id} lost its cel"))?;
        *slot = cel;
    }

    doc.bump();
    Ok(TweenReport {
        layer: layer.to_string(),
        from_frame: from_frame.to_string(),
        to_frame: to_frame.to_string(),
        created: ids,
        changed_pixels: changed.len(),
        palette_added,
        mode: opts.mode,
    })
}

/// 供提示词使用的洋葱皮摘要：两帧的 RLE + 差异统计。
/// 让模型在决定「怎么补中间帧」之前先看清两端。
pub fn onion_summary(doc: &Document, layer: &str, from_frame: &str, to_frame: &str) -> String {
    let mut out = String::new();
    let changed = match (doc.cel(layer, from_frame), doc.cel(layer, to_frame)) {
        (Some(a), Some(b)) => changed_positions(a, b, doc.width, doc.height).len(),
        _ => return "(tween: missing source cel)".into(),
    };
    let total = (doc.width * doc.height) as usize;
    let pct = if total == 0 {
        0.0
    } else {
        (changed as f32 / total as f32) * 100.0
    };
    out.push_str(&format!(
        "in-between {layer} {from_frame} -> {to_frame}: {changed} of {total} pixels differ ({pct:.1}%)\n"
    ));
    for frame in [from_frame, to_frame] {
        out.push_str(&format!("--- {frame} ---\n"));
        match context::cel_context(doc, layer, frame, 1200) {
            Ok(s) => out.push_str(&s),
            Err(e) => out.push_str(&format!("(unavailable: {e})\n")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::PixelOperation;

    fn doc() -> Document {
        let mut d = Document::new("t", 4, 1).unwrap();
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
    fn migrate_inserts_frames_before_to_frame_and_keeps_it_intact() {
        let mut d = doc();
        d.intern_color(Rgba::rgb(0, 0, 0)).unwrap();
        d.intern_color(Rgba::rgb(255, 0, 0)).unwrap();
        paint(&mut d, "L0", "F0", &[0, 1, 1, 1]);
        paint(&mut d, "L0", "F1", &[2, 2, 2, 2]);
        let before_frames = d.frames.clone();

        let report = insert_tween_frames(
            &mut d,
            "L0",
            "F0",
            "F1",
            2,
            &TweenOptions {
                mode: TweenMode::Migrate,
                order: MigrateOrder::Scan,
                ease: false,
                duration_ms: 50,
            },
        )
        .unwrap();

        assert_eq!(report.created.len(), 2);
        assert_eq!(report.changed_pixels, 4);
        let ids: Vec<&str> = d.frames.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, vec!["F0", "F2", "F3", "F1"]);
        // to_frame 的像素必须原封不动
        assert_eq!(d.cel("L0", "F1").unwrap().indices, vec![2, 2, 2, 2]);
        // 第一帧翻 1 个（2/3），第二帧翻 3 个（4/3 -> round 1.33 = 1? 见下）
        let first: Vec<u16> = d.cel("L0", "F2").unwrap().indices.clone();
        let second: Vec<u16> = d.cel("L0", "F3").unwrap().indices.clone();
        assert!(first.iter().filter(|&&v| v == 2).count() >= 1);
        assert!(
            second.iter().filter(|&&v| v == 2).count() >= first.iter().filter(|&&v| v == 2).count()
        );
        // duration 生效
        assert_eq!(d.frames[1].duration_ms, 50);
        assert_eq!(before_frames.len() + 2, d.frames.len());
    }

    #[test]
    fn blend_interpolates_colors_and_interns_them() {
        let mut d = doc();
        d.intern_color(Rgba::rgb(0, 0, 0)).unwrap();
        d.intern_color(Rgba::rgb(100, 0, 0)).unwrap();
        paint(&mut d, "L0", "F0", &[1, 1, 1, 1]);
        paint(&mut d, "L0", "F1", &[2, 2, 2, 2]);

        let report = insert_tween_frames(
            &mut d,
            "L0",
            "F0",
            "F1",
            1,
            &TweenOptions {
                mode: TweenMode::Blend,
                ease: false,
                ..Default::default()
            },
        )
        .unwrap();
        // t = 1/2 -> 50/100 -> 50
        assert_eq!(d.palette.len(), 3);
        assert_eq!(d.palette[2], Rgba::rgb(50, 0, 0));
        assert!(report.palette_added >= 1);
        assert_eq!(d.cel("L0", "F2").unwrap().indices, vec![3, 3, 3, 3]);
    }

    #[test]
    fn copy_mode_reproduces_the_start_frame() {
        let mut d = doc();
        d.intern_color(Rgba::rgb(1, 2, 3)).unwrap();
        d.intern_color(Rgba::rgb(4, 5, 6)).unwrap();
        paint(&mut d, "L0", "F0", &[1, 0, 1, 0]);
        paint(&mut d, "L0", "F1", &[2, 2, 2, 2]);
        insert_tween_frames(
            &mut d,
            "L0",
            "F0",
            "F1",
            1,
            &TweenOptions {
                mode: TweenMode::Copy,
                ease: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(d.cel("L0", "F2").unwrap().indices, vec![1, 0, 1, 0]);
        // Copy 不动调色板
        assert_eq!(d.palette.len(), 2);
    }

    #[test]
    fn tween_rejects_bad_requests() {
        let mut d = doc();
        assert!(
            insert_tween_frames(&mut d, "L0", "F0", "F0", 1, &TweenOptions::default()).is_err()
        );
        assert!(
            insert_tween_frames(&mut d, "L9", "F0", "F1", 1, &TweenOptions::default()).is_err()
        );
        assert!(
            insert_tween_frames(&mut d, "L0", "F0", "F1", 0, &TweenOptions::default()).is_err()
        );
        assert!(
            insert_tween_frames(&mut d, "L0", "F0", "F1", 999, &TweenOptions::default()).is_err()
        );
    }

    #[test]
    fn blend_reports_palette_pressure_instead_of_failing_midway() {
        let mut d = Document::new("t", 8, 1).unwrap();
        ops::apply_one(
            &mut d,
            &PixelOperation::CreateFrame {
                after: Some("F0".into()),
                duration_ms: 100,
                id: None,
            },
        )
        .unwrap();
        // 把调色板塞到只剩 2 个空位
        for i in 0..(MAX_PALETTE - 2) {
            // 避开灰阶，否则会和黑白插值的结果撞车，预算就探不出来了
            d.intern_color(Rgba::rgb(
                i as u8,
                255 - i as u8,
                (i.wrapping_mul(7) % 256) as u8,
            ))
            .unwrap();
        }
        // 两端取黑白，8 帧插值必然产生远超 2 个的新颜色
        d.palette[0] = Rgba::rgb(0, 0, 0);
        d.palette[1] = Rgba::rgb(255, 255, 255);
        let base = d.palette.len();
        paint(&mut d, "L0", "F0", &[1, 0, 0, 0, 0, 0, 0, 0]);
        paint(&mut d, "L0", "F1", &[2, 0, 0, 0, 0, 0, 0, 0]);
        let err = insert_tween_frames(
            &mut d,
            "L0",
            "F0",
            "F1",
            8,
            &TweenOptions {
                mode: TweenMode::Blend,
                ease: false,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(err.contains("palette"), "unexpected error: {err}");
        // 预检在开工之前：不该留下任何新帧，调色板也不该被动过
        assert_eq!(d.frames.len(), 2);
        assert_eq!(d.palette.len(), base);
    }

    #[test]
    fn onion_summary_mentions_both_frames_and_the_diff() {
        let mut d = doc();
        d.intern_color(Rgba::rgb(0, 0, 0)).unwrap();
        paint(&mut d, "L0", "F0", &[1, 1, 1, 1]);
        paint(&mut d, "L0", "F1", &[1, 1, 0, 0]);
        let text = onion_summary(&d, "L0", "F0", "F1");
        assert!(
            text.contains("in-between L0 F0 -> F1: 2 of 4 pixels differ"),
            "{text}"
        );
        assert!(text.contains("--- F0 ---"));
        assert!(text.contains("--- F1 ---"));
    }

    #[test]
    fn migrate_orders_by_distance_when_asked() {
        let mut d = Document::new("t", 5, 1).unwrap();
        ops::apply_one(
            &mut d,
            &PixelOperation::CreateFrame {
                after: Some("F0".into()),
                duration_ms: 100,
                id: None,
            },
        )
        .unwrap();
        d.intern_color(Rgba::rgb(0, 0, 0)).unwrap();
        d.intern_color(Rgba::rgb(255, 255, 255)).unwrap();
        paint(&mut d, "L0", "F0", &[0, 0, 0, 0, 0]);
        paint(&mut d, "L0", "F1", &[1, 1, 1, 1, 1]);
        let report = insert_tween_frames(
            &mut d,
            "L0",
            "F0",
            "F1",
            1,
            &TweenOptions {
                mode: TweenMode::Migrate,
                order: MigrateOrder::Radial,
                ease: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(report.changed_pixels, 5);
        assert_eq!(d.frames.len(), 3);
    }
}
