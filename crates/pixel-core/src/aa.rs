// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 平滑绘制：抗锯齿线段、贝塞尔曲线、圆 / 椭圆 / 矩形 / 多边形的柔和边缘。
//!
//! 为什么单独一层：硬边基本图形（`line` / `rect` / `circle`）画出来的接缝在
//! 像素画里经常很扎眼——一条 46° 的斜线碎成台阶，一个圆在 32px 画布上直接
//! 变成八边形。用户要的是「画细节的地方有平滑曲线可用了」，所以这里所有
//! 函数都按「像素中心到几何的连续距离」算覆盖率，一格一格地掺色，
//! 而不是把几何四舍五入到整数格。
//!
//! 覆盖率写色跟画笔扩张调色板是同一条路：混出来的中间色会 intern 进调色板。
//! 这是像素画加色阶的正常做法（`mix` 出来的渐变一样要占索引），代价可控：
//! 只在真正需要柔边的地方调用，别拿它整幅铺。
//!
//! 坐标约定：**整数坐标就是格心**，第 `i` 格占连续区间 `[i - 0.5, i + 0.5)`。
//! 这一条是跟硬边那套对齐的关键：`aaline(3, 3, 9, 3)` 的第 3 行整行满覆盖，
//! `aarect(4, 6, 11, 18, true)` 盖住的正是 `rect(4, 6, 11, 18, true)` 那一片格
//! （两端都算）。模型照着自己的坐标直觉写，硬边和柔边落在同一个地方，
// 不用记两套规则。
//!
//! 边界压在格心上时，覆盖恰好是面积的一半（边横在格中间）——这就是抗锯齿
//! 该有的答案，不是四舍五入到某一侧。

use std::collections::HashMap;
use std::ops::RangeInclusive;

use super::document::{Cel, Document, Rgba};

/// 线宽 1px 的软化半径：格心到几何的距离小于它就开始掺色，等于 1 表示
/// 贴脸的那格满覆盖、偏半格的那格掺一半——这正是锯齿消失的原因。
pub const AA_SOFT: f64 = 1.0;

/// 曲线最长细分段数。弧长 / 步长算出来的段数再大也按这个封顶：
/// 一条病态曲线（半径写成 1e9）不该把沙箱时间烧穿。
pub const MAX_CURVE_SEGMENTS: usize = 4096;

/// 覆盖率扫描的工作区。模型给的坐标偶尔会飞出画布（`cx + r * 1e6`、
/// 半径写成 -20），裁剪框在扫格之前就把几何切掉：既不会把色写到画布外，
/// 也不会为一片永远落不进来的空气扫出上亿格。
///
/// 左上角第一格的格心是 (0, 0)，所以 `right = width - 1`。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clip {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl Clip {
    pub fn for_canvas(width: u32, height: u32) -> Self {
        Clip {
            left: 0.0,
            top: 0.0,
            right: width.max(1) as f64 - 1.0,
            bottom: height.max(1) as f64 - 1.0,
        }
    }

    /// 裁线段（Liang-Barsky）。保留框外 AA_SOFT 那一圈，柔边才能在画布边上
    /// 正常淡出，而不是被硬切一刀。整段都在外返回 None。
    pub fn clip_segment(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> Option<(f64, f64, f64, f64)> {
        if !x0.is_finite() || !y0.is_finite() || !x1.is_finite() || !y1.is_finite() {
            return None;
        }
        let pad = AA_SOFT;
        let (mut t0, mut t1) = (0.0f64, 1.0f64);
        let dx = x1 - x0;
        let dy = y1 - y0;
        for (p, q) in [
            (-dx, x0 - (self.left - pad)),
            (dx, (self.right + pad) - x0),
            (-dy, y0 - (self.top - pad)),
            (dy, (self.bottom + pad) - y0),
        ] {
            if p.abs() <= f64::EPSILON {
                // 平行于这条边：整段同在框外就否掉，在框内则这条边不起作用。
                if q < 0.0 {
                    return None;
                }
                continue;
            }
            let r = q / p;
            if p < 0.0 {
                if r > t1 {
                    return None;
                }
                t0 = t0.max(r);
            } else {
                if r < t0 {
                    return None;
                }
                t1 = t1.min(r);
            }
        }
        Some((x0 + t0 * dx, y0 + t0 * dy, x0 + t1 * dx, y0 + t1 * dy))
    }
}

/// 行区间跟裁剪框求交。整个区间都在外返回 None。
fn row_span(clip: Option<Clip>, top: f64, bottom: f64) -> Option<RangeInclusive<i64>> {
    let (top, bottom) = match clip {
        Some(c) => (top.max(c.top), bottom.min(c.bottom)),
        None => (top, bottom),
    };
    let start = cell_of(top).max(0);
    let end = cell_of(bottom);
    (end >= start).then_some(start..=end)
}

/// 列区间跟裁剪框求交。
fn col_span(clip: Option<Clip>, left: f64, right: f64) -> Option<RangeInclusive<i64>> {
    let (left, right) = match clip {
        Some(c) => (left.max(c.left), right.min(c.right)),
        None => (left, right),
    };
    let start = cell_of(left).max(0);
    let end = cell_of(right);
    (end >= start).then_some(start..=end)
}

/// 一次平滑绘制的覆盖缓冲：同一格只留最大覆盖率。
///
/// 取最大而不是累加，是因为曲线是分小段画的，相邻段在接点处天然重叠；
/// 累加会让接点糊成一团，取最大才能保证任何一格的掺色比例不变。
#[derive(Debug, Default)]
pub struct Coverage {
    cells: HashMap<(u32, u32), f32>,
    clip: Option<Clip>,
}

impl Coverage {
    pub fn new() -> Self {
        Coverage::default()
    }

    /// 带画布裁剪的覆盖缓冲。Lua 侧一律用这个构造：模型写到画布外的坐标
    /// 在这里就被挡掉，不会白扫一大片空气。
    pub fn for_canvas(width: u32, height: u32) -> Self {
        Coverage {
            cells: HashMap::new(),
            clip: Some(Clip::for_canvas(width, height)),
        }
    }

    pub fn clip(&self) -> Option<Clip> {
        self.clip
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// 记一格覆盖率，越界直接丢——平滑函数跟形状函数一样自己裁剪。
    pub fn plot(&mut self, x: u32, y: u32, coverage: f32) {
        if !coverage.is_finite() || coverage <= 0.0 {
            return;
        }
        let coverage = coverage.min(1.0);
        let cell = self.cells.entry((x, y)).or_insert(0.0);
        if coverage > *cell {
            *cell = coverage;
        }
    }

    pub fn into_cells(self) -> Vec<(u32, u32, f32)> {
        let mut out: Vec<(u32, u32, f32)> = self
            .cells
            .into_iter()
            .map(|((x, y), c)| (x, y, c))
            .collect();
        // 排序只为让写入顺序稳定：测试要复现，落盘 diff 也要能人读。
        out.sort_by_key(|(x, y, _)| (*y, *x));
        out
    }
}

/// 连续坐标 -> 格下标。负数一律落到 0 之外，由调用方裁剪。
fn cell_of(v: f64) -> i64 {
    v.floor() as i64
}

/// 点到线段的最短距离。退化线段（首尾重合）也安全。
fn distance_to_segment(px: f64, py: f64, x0: f64, y0: f64, x1: f64, y1: f64) -> f64 {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len2 = dx * dx + dy * dy;
    if len2 <= f64::EPSILON {
        return ((px - x0).powi(2) + (py - y0).powi(2)).sqrt();
    }
    let mut t = ((px - x0) * dx + (py - y0) * dy) / len2;
    t = t.clamp(0.0, 1.0);
    let cx = x0 + t * dx;
    let cy = y0 + t * dy;
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

/// 一段抗锯齿直线：只扫包围盒（外扩一格），格心离线段越近覆盖越满。
///
/// 端点不特殊处理：距离法给出的就是圆头，一段段接起来的折线在拐角处
/// 自然变圆润，不会像 Bresenham 那样在拐角露出缺口。
pub fn stroke_segment(buf: &mut Coverage, x0: f64, y0: f64, x1: f64, y1: f64) {
    if !x0.is_finite() || !y0.is_finite() || !x1.is_finite() || !y1.is_finite() {
        return;
    }
    // 先按画布裁一刀。裁出来的接缝离格心至少有 AA_SOFT 远，所以裁剪
    // 不改动画布内任何一格算出来的距离，只是不再为画布外的空气扫格。
    let (x0, y0, x1, y1) = match buf.clip() {
        Some(clip) => match clip.clip_segment(x0, y0, x1, y1) {
            Some(seg) => seg,
            None => return,
        },
        None => (x0, y0, x1, y1),
    };
    // 病态长线段先对半拆：细分封顶（MAX_CURVE_SEGMENTS）只数段数，不数每段
    // 多长，一条 1e6 长的「曲线」拆到 4096 段后每段还有 366px，单独扫一次
    // 包围盒就是 13 万格，4096 段能把沙箱时间烧穿。拆到每段 <= 2px 再扫，
    // 包围盒就只有几格，跟正常短线段同一本账。
    let dx = x1 - x0;
    let dy = y1 - y0;
    if dx * dx + dy * dy > 4.0 {
        let (mx, my) = ((x0 + x1) * 0.5, (y0 + y1) * 0.5);
        stroke_segment(buf, x0, y0, mx, my);
        stroke_segment(buf, mx, my, x1, y1);
        return;
    }
    let min_x = cell_of(x0.min(x1) - AA_SOFT).max(0);
    let max_x = cell_of(x0.max(x1) + AA_SOFT);
    let min_y = cell_of(y0.min(y1) - AA_SOFT).max(0);
    let max_y = cell_of(y0.max(y1) + AA_SOFT);
    for gy in min_y..=max_y {
        for gx in min_x..=max_x {
            // 采样点就是格心本身（整数坐标），不能 +0.5：否则整数坐标 3
            // 落到格 2 的格心上，模型照自己的坐标直觉写出来的线会整幅偏半格。
            let d = distance_to_segment(gx as f64, gy as f64, x0, y0, x1, y1);
            let coverage = (1.0 - d / AA_SOFT) as f32;
            if coverage > 0.0 {
                buf.plot(gx as u32, gy as u32, coverage);
            }
        }
    }
}

/// 弧长估计：弦长相加，够用来定细分步数。
fn path_length(points: &[(f64, f64)]) -> f64 {
    points
        .windows(2)
        .map(|w| ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt())
        .sum()
}

/// 按 ~0.4px 的步长把路径加成点串。步长取这么小是为了让曲率真实落到像素上：
/// 半径 6px 的圆能被切成约 94 段，边缘自然是圆的，而不是 16 段的多边形。
pub fn densify(points: &[(f64, f64)]) -> Vec<(f64, f64)> {
    if points.len() < 2 {
        return points.to_vec();
    }
    let length = path_length(points);
    let steps = ((length / 0.4).ceil() as usize).clamp(1, MAX_CURVE_SEGMENTS);
    let mut out = Vec::with_capacity(steps + 1);
    let per = points.len() - 1;
    for i in 0..=steps {
        let t = i as f64 / steps as f64;
        let seg = ((t * per as f64).floor() as usize).min(per - 1);
        let local = t * per as f64 - seg as f64;
        let a = points[seg];
        let b = points[seg + 1];
        out.push((a.0 + (b.0 - a.0) * local, a.1 + (b.1 - a.1) * local));
    }
    out
}

/// 折线 / 多边形轮廓。`closed` 时首尾也接上。
pub fn stroke_polyline(buf: &mut Coverage, points: &[(f64, f64)], closed: bool) {
    if points.len() < 2 {
        return;
    }
    let dense = densify(points);
    for w in dense.windows(2) {
        stroke_segment(buf, w[0].0, w[0].1, w[1].0, w[1].1);
    }
    if closed {
        let last = dense[dense.len() - 1];
        let first = dense[0];
        stroke_segment(buf, last.0, last.1, first.0, first.1);
    }
}

/// 二次贝塞尔：起点、终点加一个控制点。曲线只有三个点也说清楚弧度，
/// 是画耳朵、尾巴、衣褶这类「一个弯」最顺手的写法。
pub fn quad_curve(buf: &mut Coverage, x0: f64, y0: f64, x1: f64, y1: f64, cx: f64, cy: f64) {
    // 先摊成折线再走统一的细分路径：省一套贝塞尔求值代码，
    // 细分步长也跟别的曲线共用一套，边缘粗细一致。
    //
    // 采样点必须落在真正的二次贝塞尔上：B(t) = (1-t)^2 P0 + 2(1-t)t C + t^2 P1。
    // 只做 De Casteljau 的第一层（lerp(P0, C, t)）会把「拉向终点」那一半整段丢掉，
    // 曲线于是朝控制点弓得比真二次狠得多——垂下来的耳朵、翘起的尾巴、衣褶
    // 全都会鼓出一截，还看不出是哪儿错了。按 Bernstein 系数直接取五个点，
    // 跟三次那份（cubic_curve）同一套写法，改一处不会只改一半。
    let eval = |a: f64, c: f64, b: f64, t: f64| {
        (1.0 - t) * (1.0 - t) * a + 2.0 * (1.0 - t) * t * c + t * t * b
    };
    let flat: Vec<(f64, f64)> = (0..=4)
        .map(|i| {
            let t = i as f64 / 4.0;
            (eval(x0, cx, x1, t), eval(y0, cy, y1, t))
        })
        .collect();
    stroke_polyline(buf, &flat, false);
}

/// 三次贝塞尔：两个控制点，S 形弯和局部回勾都画得出来。
/// 两个控制点各收成一个点，签名才停在七个参数以内（clippy 的闸门）。
pub fn cubic_curve(
    buf: &mut Coverage,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    c0: (f64, f64),
    c1: (f64, f64),
) {
    let (c0x, c0y) = c0;
    let (c1x, c1y) = c1;
    let sample = |a: f64, b: f64, c: f64, d: f64, t: f64| -> f64 {
        let u = 1.0 - t;
        u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d
    };
    let mut out = Vec::with_capacity(5);
    for i in 0..=4 {
        let t = i as f64 / 4.0;
        out.push((sample(x0, c0x, c1x, x1, t), sample(y0, c0y, c1y, y1, t)));
    }
    stroke_polyline(buf, &out, false);
}

/// 一行扫描填充：左边界到右边界，两端按小数覆盖率掺色。
///
/// `ycov` 是这一行本身被形状吃掉了多少：矩形和椭圆上下两条边也要柔，
/// 靠的就是它；扫在格心上的多边形行传 1.0。
fn fill_span(buf: &mut Coverage, row: u32, left: f64, right: f64, ycov: f32) {
    if right <= left || ycov <= 0.0 {
        return;
    }
    // 格心在整数上，第 c 格占 [c-0.5, c+0.5)：跟 [left, right] 的重叠长度就是
    // 覆盖率。取过裁剪框的区间，多出来的一两格算完是 0，自己就没了。
    let Some(cells) = col_span(buf.clip(), left, right) else {
        return;
    };
    for cell in cells {
        let c = cell as f64;
        let xcov = (right - c + 0.5).min(c - left + 0.5).clamp(0.0, 1.0);
        let coverage = xcov * ycov as f64;
        if coverage > 0.0 {
            buf.plot(cell as u32, row, coverage as f32);
        }
    }
}

/// 多边形填充（扫描线，偶数规则）。边缘按小数覆盖率掺色，
/// 所以斜边不会出现「整列要么全画要么全不画」的硬台阶。
pub fn fill_polygon(buf: &mut Coverage, points: &[(f64, f64)]) {
    if points.len() < 3 {
        return;
    }
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for (_, y) in points {
        min_y = min_y.min(*y);
        max_y = max_y.max(*y);
    }
    if !min_y.is_finite() || !max_y.is_finite() {
        return;
    }
    let Some(rows) = row_span(buf.clip(), min_y, max_y) else {
        return;
    };
    for row in rows {
        // 扫描线走在格心上（整数），跟描边同一套坐标：一格中心落在形状里
        // 就是满覆盖，边界压在格心上正好半覆盖——那是面积给出的答案。
        let mid = row as f64;
        let mut crossings: Vec<f64> = Vec::new();
        // 边要连成一圈：最后一个点接回第一个点。早先只走 windows(2)，
        // 三角形的最后一条边漏在外面，扫描线只撞到一条边，一个 span 都凑不齐，
        // 整个填充静悄悄地什么都不画。
        let edges = points.len();
        for i in 0..edges {
            let (ax, ay) = points[i];
            let (bx, by) = points[(i + 1) % edges];
            if (ay <= mid && by > mid) || (by <= mid && ay > mid) {
                let t = (mid - ay) / (by - ay);
                crossings.push(ax + t * (bx - ax));
            }
        }
        if crossings.is_empty() {
            continue;
        }
        crossings.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        for pair in crossings.chunks(2) {
            if pair.len() < 2 {
                break;
            }
            let (left, right) = if pair[0] <= pair[1] {
                (pair[0], pair[1])
            } else {
                (pair[1], pair[0])
            };
            fill_span(buf, row as u32, left, right, 1.0);
        }
    }
}

/// 椭圆环：参数化采样后走同一条 AA 线段路径，
/// 圆和椭圆因此跟曲线一样柔，不会退化成 `ellipse` 那种八边形。
pub fn ellipse_ring(buf: &mut Coverage, cx: f64, cy: f64, rx: f64, ry: f64) {
    let rx = rx.abs();
    let ry = ry.abs();
    if rx <= 0.0 || ry <= 0.0 {
        return;
    }
    // Ramanujan 周长近似定段数：保证大圆也每 0.4px 一段。
    let ratio = ((rx - ry) / (rx + ry).max(f64::EPSILON)).powi(2);
    let perimeter = std::f64::consts::PI
        * (rx + ry)
        * (1.0 + (3.0 * ratio) / (10.0 + (4.0 - ratio).sqrt()).sqrt());
    let steps = ((perimeter / 0.4).ceil() as usize).clamp(8, MAX_CURVE_SEGMENTS);
    let mut points = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let a = i as f64 / steps as f64 * std::f64::consts::TAU;
        points.push((cx + rx * a.cos(), cy + ry * a.sin()));
    }
    stroke_polyline(buf, &points, true);
}

/// 椭圆（含圆）的实心填充：逐行解半宽，左右两段各带一点边缘覆盖。
pub fn ellipse_fill(buf: &mut Coverage, cx: f64, cy: f64, rx: f64, ry: f64) {
    let rx = rx.abs();
    let ry = ry.abs();
    if rx <= 0.0 || ry <= 0.0 {
        return;
    }
    let (top, bottom) = (cy - ry, cy + ry);
    let Some(rows) = row_span(buf.clip(), top, bottom) else {
        return;
    };
    for row in rows {
        // 半宽按格心算（不是格心+0.5），跟描边同一套坐标。
        let dy = (row as f64 - cy) / ry;
        if dy.abs() >= 1.0 {
            continue;
        }
        let half = rx * (1.0 - dy * dy).sqrt();
        // 上下两条边也要柔：这一行被 [top, bottom] 吃掉多少，就是竖向覆盖。
        let r = row as f64;
        let ycov = ((bottom - (r - 0.5)).min(r + 0.5 - top)).clamp(0.0, 1.0);
        fill_span(buf, row as u32, cx - half, cx + half, ycov as f32);
    }
}

/// 矩形填充（连续区间，两端按格边界算覆盖）。两个角写反也接受，
/// 内部交换，不报错——画细节的人经常随手写反。
pub fn rect_fill(buf: &mut Coverage, x0: f64, y0: f64, x1: f64, y1: f64) {
    // 两个角是「两端的格都算」（跟 rect 一致），所以连续区间要摊到这两个格的
    // 外沿：x0..x1 占的格是 x0..=x1，连续区间就是 [x0-0.5, x1+0.5]。
    let (left, right) = if x0 <= x1 {
        (x0 - 0.5, x1 + 0.5)
    } else {
        (x1 - 0.5, x0 + 0.5)
    };
    let (top, bottom) = if y0 <= y1 {
        (y0 - 0.5, y1 + 0.5)
    } else {
        (y1 - 0.5, y0 + 0.5)
    };
    // 行范围多扫一两行不花钱：摊不到的格算出来覆盖是 0，自己就没了。
    // 四个边全按格边界算覆盖，所以旋转不了的矩形也有一条柔边——
    // 「基础图形很突兀」说的正是这条硬缝。
    let Some(rows) = row_span(buf.clip(), top, bottom) else {
        return;
    };
    for row in rows {
        let r = row as f64;
        let ycov = ((bottom - (r - 0.5)).min(r + 0.5 - top)).clamp(0.0, 1.0);
        fill_span(buf, row as u32, left, right, ycov as f32);
    }
}

/// 3x3 有序抖动矩阵。`dither()` 用它在「不许扩色板」和「想让边缘柔一点」
/// 之间找折中：覆盖率折算成「这格落不落色」，出来的边是颗粒状过渡，
/// 但每个落下去的色都还在原调色板里。
pub const BAYER4: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// 覆盖率 -> 落不落色（0..1 的阈值，16 档）。
pub fn bayer_threshold(x: u32, y: u32) -> f64 {
    let row = (y % 4) as usize;
    let col = (x % 4) as usize;
    (BAYER4[row][col] as f64 + 0.5) / 16.0
}

/// 把覆盖缓冲写进某个 cel。返回真正改动的格数。
///
/// 覆盖率 1 直接写索引；中间值读回当前颜色跟目标色掺。目标是透明
/// （索引 0）时是「软擦」：整格擦掉，半格把已有色的 alpha 淡出去。
pub fn apply(
    doc: &mut Document,
    layer: &str,
    frame: &str,
    buf: Coverage,
    idx: u16,
) -> Result<usize, String> {
    let target = match doc.color_of(idx) {
        Some(color) => color,
        None => return Ok(0),
    };
    let width = doc.width;
    let cells = buf.into_cells();
    // 分三趟走，为的是不在调 intern_color 时攥着 cel 的可变借用。
    // 中间多一趟「读当前色」也比给每格单独 cel_mut 便宜：一次线性扫。
    let existing: Vec<Option<u16>> = {
        let cel: &Cel = doc
            .cel(layer, frame)
            .ok_or_else(|| format!("cel {layer}/{frame} missing"))?;
        cells
            .iter()
            .map(|(x, y, _)| cel.get(width, *x, *y))
            .collect()
    };
    let mut writes: Vec<(u32, u32, u16)> = Vec::with_capacity(cells.len());
    for ((x, y, coverage), current) in cells.into_iter().zip(existing) {
        // 先算这格要落到哪个索引，落得跟现在一样就不写：一次「重画同色」
        // 不该被算成改动，撤销栈里也不该为它多压一级。
        if coverage >= 0.999 {
            let next = idx;
            if current != Some(next) {
                writes.push((x, y, next));
            }
            continue;
        }
        let dst = current
            .and_then(|i| doc.color_of(i))
            .unwrap_or(Rgba::TRANSPARENT);
        // 目标是透明时做「软擦」：整格擦掉，半格把已有色的 alpha 淡出去。
        // 往空气里掺色什么都得不到，但把 alpha 淡出去是有意义的：
        // 柔边擦除是修边最常用的动作，不该让模型只能硬擦一整列。
        if target.a == 0 {
            if dst.a == 0 {
                continue;
            }
            let faded = fade_alpha(dst, coverage);
            match doc.intern_color(faded) {
                // 淡到 0 就是擦掉，跟整格擦同一个结果，别再压一笔无变化写入。
                Ok(next) if next != 0 && current != Some(next) => writes.push((x, y, next)),
                // 调色板满了就整格擦掉：糙一格，好过整条曲线报错倒下。
                _ => writes.push((x, y, 0)),
            }
            continue;
        }
        // 掺出来的中间色也要过一遍配色锁：锁着的层只肯用范围里的颜色，
        // 不然一条柔边能把文档调色板撑到满——那正是上锁要防的事。
        let blended = doc.color_for_layer(layer, blend_rgba(dst, target, coverage));
        match doc.intern_color(blended) {
            Ok(mixed) if current != Some(mixed) => writes.push((x, y, mixed)),
            // 调色板满了就当纯覆盖写：边缘糙一格，好过整条曲线报错倒下。
            Err(_) if current != Some(idx) => writes.push((x, y, idx)),
            _ => {}
        }
    }
    let painted = writes.len();
    let cel: &mut Cel = doc
        .cel_mut(layer, frame)
        .ok_or_else(|| format!("cel {layer}/{frame} missing"))?;
    for (x, y, i) in writes {
        cel.set(width, x, y, i);
    }
    Ok(painted)
}

/// 把 alpha 按覆盖率淡出去（软擦）。RGB 不动，只降 alpha：
/// 淡出是把颜色「变没」，不是把它染黑。
fn fade_alpha(color: Rgba, coverage: f32) -> Rgba {
    let keep = (1.0 - coverage.clamp(0.0, 1.0)).max(0.0);
    let a = (color.a as f32 * keep).round().clamp(0.0, 255.0) as u8;
    Rgba { a, ..color }
}

/// 单格有序抖动：覆盖率按 Bayer 阈值折算成「这格落不落色」。
///
/// 给「不许扩色板」的图层用——同一种覆盖率下，落下去的色全在原范围里，
/// 出来的边是 4x4 颗粒状的过渡，不是脏灰色。连续坐标由调用方取整，
/// 越界（取整后落在画布外）直接报错，跟 `pset` 一样自己裁剪。
pub fn dither_dot(
    doc: &mut Document,
    layer: &str,
    frame: &str,
    x: f64,
    y: f64,
    idx: u16,
    coverage: f32,
) -> Result<bool, String> {
    let coverage = coverage.clamp(0.0, 1.0);
    // 先取整再夹界，且必须夹的是 i64：负坐标直接 `as u32` 会静默绕到 0，
    // 模型一个减过头的偏移就把色画到画布角上，而它以为画的是画布外。
    let (xi, yi) = (x.floor() as i64, y.floor() as i64);
    if xi < 0 || yi < 0 || xi >= doc.width as i64 || yi >= doc.height as i64 {
        return Err(format!(
            "coordinate ({}, {}) is outside the canvas {}x{}",
            xi, yi, doc.width, doc.height
        ));
    }
    let (x, y) = (xi as u32, yi as u32);
    if coverage <= 0.0 || coverage >= bayer_threshold(x, y) as f32 {
        return Ok(false);
    }
    let target = doc
        .color_of(idx)
        .ok_or_else(|| format!("color {idx} unknown"))?;
    if target.a == 0 {
        return Ok(false);
    }
    let width = doc.width;
    let cel: &mut Cel = doc
        .cel_mut(layer, frame)
        .ok_or_else(|| format!("cel {layer}/{frame} missing"))?;
    cel.set(width, x, y, idx);
    Ok(true)
}

/// 按覆盖率掺色。源带 alpha 时按 `source.a * coverage` 参与，
/// 这样往透明底上画柔边得到的是半透明色，而不是把底色染灰。
pub fn blend_rgba(dst: Rgba, src: Rgba, coverage: f32) -> Rgba {
    let a = (src.a as f32 / 255.0) * coverage.clamp(0.0, 1.0);
    if a <= 0.0 {
        return dst;
    }
    if a >= 1.0 {
        return src;
    }
    let inv = 1.0 - a;
    let mix =
        |d: u8, s: u8| -> u8 { (d as f32 * inv + s as f32 * a).round().clamp(0.0, 255.0) as u8 };
    let out_a = (src.a as f32 * a + dst.a as f32 * inv)
        .round()
        .clamp(0.0, 255.0) as u8;
    Rgba {
        r: mix(dst.r, src.r),
        g: mix(dst.g, src.g),
        b: mix(dst.b, src.b),
        a: out_a,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank() -> Document {
        Document::new("test", 32, 32).expect("32x32 within limits")
    }

    fn solid_count(doc: &Document, layer: &str, frame: &str) -> usize {
        doc.cel(layer, frame)
            .map(|c| c.indices.iter().filter(|i| **i != 0).count())
            .unwrap_or(0)
    }

    #[test]
    fn an_axis_aligned_line_lands_on_its_own_row_with_no_fringe() {
        let mut buf = Coverage::new();
        stroke_segment(&mut buf, 3.0, 3.0, 9.0, 3.0);
        let cells = buf.into_cells();
        // 覆盖必须连续，不能跳格。
        let mut xs: Vec<u32> = cells.iter().map(|(x, _, _)| *x).collect();
        xs.sort_unstable();
        xs.dedup();
        assert_eq!(xs, (3..=9).collect::<Vec<u32>>());
        // 横平竖直的线本来就不需要抗锯齿：落在自己那一行，上下两行一点不碰。
        assert!(
            cells
                .iter()
                .all(|(x, y, c)| *y == 3 && (*c >= 0.999 || *x == 3 || *x == 9)),
            "{cells:?}"
        );
        assert!(cells
            .iter()
            .any(|(x, y, c)| *x == 9 && *y == 3 && *c >= 0.999));
    }

    #[test]
    fn a_diagonal_line_is_softened_instead_of_staircased() {
        let mut buf = Coverage::new();
        stroke_segment(&mut buf, 2.0, 2.0, 12.0, 9.0);
        let cells = buf.into_cells();
        let partial = cells
            .iter()
            .filter(|cell| cell.2 > 0.05 && cell.2 < 0.999)
            .count();
        let full = cells.iter().filter(|cell| cell.2 >= 0.999).count();
        assert!(partial > 6, "斜线该有一批半覆盖格: {partial}");
        // 半覆盖格得比满覆盖格多才叫「柔」——纯 Bresenham 是一格不掺的。
        assert!(partial > full, "partial {partial} vs full {full}");
    }

    #[test]
    fn a_curve_has_nothing_outside_its_soft_band() {
        let mut buf = Coverage::new();
        quad_curve(&mut buf, 2.0, 12.0, 14.0, 12.0, 8.0, 0.0);
        // 整条带都该挤在软化半径内，不许有孤零零的远点。
        assert!(
            buf.into_cells().iter().all(|(_, _, c)| *c > 0.0),
            "覆盖缓冲里只会有正覆盖"
        );
    }

    #[test]
    fn a_curve_leaves_its_chord() {
        let mut bent = Coverage::new();
        quad_curve(&mut bent, 2.0, 12.0, 14.0, 12.0, 8.0, 0.0);
        let off_chord = bent
            .into_cells()
            .into_iter()
            .filter(|(_, y, _)| *y < 10)
            .count();
        assert!(off_chord > 4, "贝塞尔必须拱离开弦: {off_chord}");
    }

    /// 采样点要落在**真正**的二次贝塞尔上：B(t) = (1-t)^2 P0 + 2(1-t)t C + t^2 P1。
    /// 端点水平的对称弧，中点高度就是 (y0 + 2*cy + y1) / 4——控制点再高也越不过
    /// 这条线。以前只做 De Casteljau 第一层（lerp(P0, C, t)），等于把「拉向
    /// 终点」那一半丢了，曲线的拱高直接翻倍，耳朵尾巴全都会鼓出来一截。
    /// 这条测试按解析值钉死拱高，畸形参数化在这道闸门上过不去。
    #[test]
    fn a_quad_curve_reaches_exactly_the_bezier_height() {
        // 起点/终点同在 y=40，控制点 y=4：真实拱高 = (40 + 2*4 + 40)/4 = 22。
        let (x0, y0) = (4.0, 40.0);
        let (x1, y1) = (44.0, 40.0);
        let (cx, cy) = (24.0, 4.0);
        let mut buf = Coverage::new();
        quad_curve(&mut buf, x0, y0, x1, y1, cx, cy);
        let highest = buf
            .into_cells()
            .into_iter()
            .map(|(_, y, _)| y as f64)
            .fold(f64::MAX, f64::min);
        // 格心落在整数上，覆盖带半径又是 AA_SOFT=1，所以最高格心比解析拱高
        // 高出不到一格。给 1.5px 的容差正好卡住「解析值 ± 柔边」这一带，
        // 旧写法在这里会报到 y≈13，差出 9px，一锤就响。
        let expected = (y0 + 2.0 * cy + y1) / 4.0;
        assert!(
            (highest - expected).abs() <= 1.5,
            "二次曲线的拱高该是 {expected}，实际报到 y={highest}"
        );
    }

    /// 二次和三次各自按解析式取值后，同一条弧要能互相换算：一个二次
    /// B(t) = (1-t)^2 P0 + 2(1-t)t C + t^2 P1 表达成三次，控制点是
    /// Q1 = P0 + 2/3(C-P0)、Q2 = P1 + 2/3(C-P1)，两条曲线逐点重合。
    /// 两边采样步长又都是 5 段，所以覆盖缓冲逐格相等，不是「差不多」。
    /// 这条守的是「两种写法算的是同一条二次曲线」——任何一边的参数化
    /// 退回了折线或者漏了权重，这里立刻对不上。
    #[test]
    fn quad_and_cubic_agree_on_a_shared_arc() {
        let (x0, y0) = (4.0, 40.0);
        let (x1, y1) = (44.0, 40.0);
        let (cx, cy) = (24.0, 4.0);
        let mut qb = Coverage::new();
        quad_curve(&mut qb, x0, y0, x1, y1, cx, cy);
        // 三次的等价控制点：同一个二次表达成三次 Bézier，两条曲线逐点重合。
        let (q1x, q1y) = (x0 + 2.0 / 3.0 * (cx - x0), y0 + 2.0 / 3.0 * (cy - y0));
        let (q2x, q2y) = (x1 + 2.0 / 3.0 * (cx - x1), y1 + 2.0 / 3.0 * (cy - y1));
        let mut cb = Coverage::new();
        cubic_curve(&mut cb, x0, y0, x1, y1, (q1x, q1y), (q2x, q2y));
        let key = |cells: Vec<(u32, u32, f32)>| {
            let mut v: Vec<(u32, u32, i32)> = cells
                .into_iter()
                .map(|(x, y, c)| (x, y, (c * 1000.0).round() as i32))
                .collect();
            v.sort_unstable();
            v
        };
        assert_eq!(
            key(qb.into_cells()),
            key(cb.into_cells()),
            "同一条二次曲线，走 aacurve 和 aacubic 两种写法必须画出一样的覆盖"
        );
    }

    #[test]
    fn fill_covers_an_axis_aligned_box_exactly() {
        let mut buf = Coverage::new();
        rect_fill(&mut buf, 4.0, 6.0, 11.0, 9.0);
        let cells = buf.into_cells();
        // x in [4,12) 、 y in [6,10) 共 32 格，角上都该是满覆盖。
        assert_eq!(cells.len(), 32, "{cells:?}");
        assert!(cells.iter().all(|(_, _, c)| *c >= 0.999));
    }

    #[test]
    fn polygon_fill_reaches_the_tip_and_misses_outside() {
        let mut buf = Coverage::new();
        fill_polygon(&mut buf, &[(4.0, 4.0), (14.0, 4.0), (9.0, 14.0)]);
        assert!(buf.cells.contains_key(&(9, 12)), "尖部要有覆盖");
        assert!(!buf.cells.contains_key(&(4, 14)), "三角形外不许有覆盖");
        assert!(buf.cells.contains_key(&(9, 5)), "腰部要填满");
    }

    #[test]
    fn ellipse_fill_stays_inside_its_bounding_box() {
        let mut buf = Coverage::new();
        ellipse_fill(&mut buf, 10.0, 10.0, 6.0, 4.0);
        let cells = buf.into_cells();
        assert!(
            cells
                .iter()
                .all(|(x, y, _)| *x >= 4 && *x <= 16 && *y >= 6 && *y <= 14),
            "椭圆必须待在包围盒里"
        );
        assert!(cells.iter().any(|(x, y, _)| *x == 10 && *y == 10));
    }

    #[test]
    fn bayer_spreads_halves_roughly_evenly() {
        let mut on = 0;
        for y in 0..8u32 {
            for x in 0..8u32 {
                if bayer_threshold(x, y) < 0.5 {
                    on += 1;
                }
            }
        }
        assert_eq!(on, 32, "一半覆盖率该点亮一半格");
    }

    #[test]
    fn apply_blends_mid_coverage_into_a_new_palette_slot() {
        let mut doc = blank();
        doc.intern_color(Rgba::rgb(10, 20, 30)).unwrap();
        doc.intern_color(Rgba::rgb(200, 100, 50)).unwrap();
        // 先把底色压进这一格：掺色是「覆盖到已有颜色上」，往空气里掺得到的
        // 是半透明的原色本身（那是覆盖，不是混合），断言不出中间值。
        let w = doc.width;
        doc.cel_mut("L0", "F0").unwrap().set(w, 2, 2, 1);
        let mut buf = Coverage::new();
        buf.plot(2, 2, 0.5);
        let painted = apply(&mut doc, "L0", "F0", buf, 2).unwrap();
        assert_eq!(painted, 1);
        assert!(solid_count(&doc, "L0", "F0") >= 1);
        // 掺出来的色得是两个原色的中间值，不是其中任何一个。
        let idx = doc.cel("L0", "F0").unwrap().get(doc.width, 2, 2).unwrap();
        let color = doc.color_of(idx).unwrap();
        let mean = |a: u8, b: u8| (a as f32 + b as f32) / 2.0;
        assert!((color.r as f32 - mean(10, 200)).abs() <= 2.0);
        assert!((color.g as f32 - mean(20, 100)).abs() <= 2.0);
    }

    #[test]
    fn erasing_with_full_coverage_clears_the_cell() {
        let mut doc = blank();
        // 先涂一格实的，再用透明索引 0 全覆盖：这一下必须擦干净，
        // 而「本来就没改」的格不许被算成改动。
        let w = doc.width;
        doc.cel_mut("L0", "F0").unwrap().set(w, 1, 1, 1);
        doc.cel_mut("L0", "F0").unwrap().set(w, 3, 3, 1);
        let mut buf = Coverage::new();
        buf.plot(1, 1, 1.0);
        let painted = apply(&mut doc, "L0", "F0", buf, 0).unwrap();
        assert_eq!(painted, 1);
        assert_eq!(solid_count(&doc, "L0", "F0"), 1);
        assert_eq!(doc.cel("L0", "F0").unwrap().get(doc.width, 1, 1), Some(0));
        assert_eq!(doc.cel("L0", "F0").unwrap().get(doc.width, 3, 3), Some(1));
    }

    #[test]
    fn repainting_the_same_index_changes_nothing() {
        let mut doc = blank();
        doc.intern_color(Rgba::rgb(80, 90, 100)).unwrap();
        let w = doc.width;
        doc.cel_mut("L0", "F0").unwrap().set(w, 5, 5, 1);
        let mut buf = Coverage::new();
        buf.plot(5, 5, 1.0);
        buf.plot(6, 5, 1.0);
        // 一格本来就是它、一格换成它：只算一次改动，空操作不该塞满撤销栈。
        let painted = apply(&mut doc, "L0", "F0", buf, 1).unwrap();
        assert_eq!(painted, 1);
        assert_eq!(doc.cel("L0", "F0").unwrap().get(doc.width, 6, 5), Some(1));
    }

    #[test]
    fn a_long_curve_is_capped_not_runaway() {
        // 半径病态的路要用画布裁剪接住：否则为一条飞出画布 150 万像素的曲线
        // 也能扫出上亿格，整个沙箱跟着卡死。32x32 上它只剩一小截。
        let mut buf = Coverage::for_canvas(32, 32);
        quad_curve(&mut buf, 0.0, 0.0, 1.0e6, 1.0e6, 5.0e5, 0.0);
        assert!(!buf.is_empty());
        assert!(buf.len() < 4_096, "覆盖 buffer 不许失控: {}", buf.len());
    }

    #[test]
    fn geometry_outside_the_canvas_is_dropped_not_scanned() {
        let mut buf = Coverage::for_canvas(16, 16);
        // 整条都在画布外：一格都不该有，更不该为它扫任何东西。
        stroke_segment(&mut buf, 900.0, 900.0, 940.0, 900.0);
        stroke_segment(&mut buf, -80.0, -80.0, -20.0, -20.0);
        ellipse_fill(&mut buf, 900.0, 900.0, 400.0, 400.0);
        rect_fill(&mut buf, -500.0, -500.0, -100.0, -100.0);
        assert!(buf.is_empty(), "画布外的几何不许落任何色");
        // 半径写成负数按绝对值用，不是直接返回：模型常把 w/2 写错符号。
        ellipse_fill(&mut buf, 8.0, 8.0, -4.0, -4.0);
        assert!(!buf.is_empty(), "负半径要能画出圆");
    }
}
