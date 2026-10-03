// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
// 纸娃娃白膜生成器：按 RPG Maker 角色行走图的固定网格，在每个格子里
// 铺一版走姿的平色人形剪影。
//
// 为什么它是生成器而不是又一张提示词：引擎（VX / Ace / MV / MZ）是直接按
// 行列号切图的，列数错了整张图错位一行，而「摆帧」这件事对像素画来说
// 恰恰是最容易走形的——四条腿各画各的、两个格子的脚不在一条地平线上，
// 都是这么来的。几何在这里算一次，模型只管在剪影上上色和细化。
//
// 配色只给浅灰阶，不给任何倾向色：白膜的用途就是「等着被换色」，
// 底色带倾向的话，用户之后铺什么颜色都在和这层灰打架。灰阶按部件分区给值
//（脸 / 发 / 衣 / 裤 / 远侧肢体 / 鞋与五官），分区边界就是将来换色的边界，
// 换色时轮廓不会跑。
//
// 只画一个图层的全部帧：行走图是「一个角色的一个动作」，横向铺的是同
// 一具身体的不同相位，说成不同图层没有意义。

use crate::document::{Cel, Document, Rgba};

/// 一张 RPG Maker 角色行走图的网格。行恒为 4，列按画布比例猜 3 或 4。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SheetLayout {
    /// 行数。RPG Maker 角色表固定 4 行，不是猜的。
    pub rows: u32,
    /// 列数：3 列（VX / Ace / MV / MZ）或 4 列（XP）。
    pub cols: u32,
    pub cell_w: u32,
    pub cell_h: u32,
}

impl SheetLayout {
    /// 这一行朝哪。RPG Maker 的角色表行序是死的：下、左、右、上。
    pub fn facing(self, row: u32) -> Facing {
        match row {
            0 => Facing::Down,
            1 => Facing::Left,
            2 => Facing::Right,
            _ => Facing::Up,
        }
    }
}

/// 一个朝向。`dir` 是侧视时脸的朝向（左 -1 / 右 +1），正视与背视为 0。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Facing {
    Down,
    Left,
    Right,
    Up,
}

impl Facing {
    pub fn id(self) -> &'static str {
        match self {
            Facing::Down => "down",
            Facing::Left => "left",
            Facing::Right => "right",
            Facing::Up => "up",
        }
    }

    /// -1 朝左、+1 朝右、0 正对或背对。
    fn dir(self) -> i32 {
        match self {
            Facing::Left => -1,
            Facing::Right => 1,
            _ => 0,
        }
    }

    /// 背对观察者：整颗后脑勺都是头发，不画五官。
    fn is_back(self) -> bool {
        matches!(self, Facing::Up)
    }
}

/// 白膜的浅灰阶。七个值各管一片区域，分区就是将来换色的边界。
/// 刻意做成一串等距的浅灰：白膜是占位底稿，底色越中性，之后铺什么颜色越准。
const GRAY_HILIGHT: Rgba = Rgba::rgb(0xf2, 0xf2, 0xf2);
const GRAY_FACE: Rgba = Rgba::rgb(0xe4, 0xe4, 0xe4);
const GRAY_CLOTHES: Rgba = Rgba::rgb(0xd2, 0xd2, 0xd2);
const GRAY_HAIR: Rgba = Rgba::rgb(0xc0, 0xc0, 0xc0);
const GRAY_TROUSERS: Rgba = Rgba::rgb(0xa8, 0xa8, 0xa8);
const GRAY_FAR: Rgba = Rgba::rgb(0x8e, 0x8e, 0x8e);
const GRAY_MARK: Rgba = Rgba::rgb(0x6e, 0x6e, 0x6e);

/// 灰阶清单，顺序即分区顺序：调用方据此把调色板里的同名索引取出来。
pub const BASE_GRAYS: [Rgba; 7] = [
    GRAY_HILIGHT,
    GRAY_FACE,
    GRAY_CLOTHES,
    GRAY_HAIR,
    GRAY_TROUSERS,
    GRAY_FAR,
    GRAY_MARK,
];

/// 一个格子里的一版走姿。脚下留透明边、重心上下、四肢相位错开，
/// 全是按真实步态算的，不是随便偏移几个像素。
#[derive(Debug, Clone, Copy)]
struct Pose {
    /// 近侧脚相对髋部的水平位移（像素，带符号）。
    near_foot: i32,
    /// 远侧脚相对髋部的水平位移。
    far_foot: i32,
    /// 近侧手相对肩部的水平位移。
    near_hand: i32,
    /// 远侧手相对肩部的水平位移。
    far_hand: i32,
    /// 躯干抬升（像素）。触地相最低，过腿相最高。
    lift: i32,
}

/// 一次铺白膜的结果：网格是什么、铺了几帧、实际用到的灰阶是哪些。
#[derive(Debug, Clone)]
pub struct PaperDollReport {
    pub layout: SheetLayout,
    pub frames_painted: usize,
    /// 实际落进调色板的灰阶。图层锁着时会归队到它的配色范围，值就变了。
    pub grays: Vec<Rgba>,
}

/// 从画布宽高认出这是不是一个角色行走图网格。
///
/// 只认两个候选列数（3 和 4），按格子宽高比最接近 1 挑：144x192 的 3 列
/// 格子是 48x48，128x128 的 4 列格子是 32x32，两边都整——这正是两代引擎的
/// 规格。行恒为 4，因为 RPG Maker 的角色表永远是四行四向。
pub fn detect(width: u32, height: u32) -> Option<SheetLayout> {
    let rows = 4u32;
    let cell_h = height / rows;
    // 格子小到这个地步，人形连头都放不下，别硬铺成一团糊。
    if cell_h < 8 || width < 8 {
        return None;
    }
    let mut best: Option<(u32, f64)> = None;
    for cols in [3u32, 4] {
        let cell_w = width / cols;
        if cell_w < 8 {
            continue;
        }
        let score = (cell_w as f64 / cell_h as f64 - 1.0).abs();
        if best.is_none_or(|(_, s)| score < s) {
            best = Some((cols, score));
        }
    }
    let (cols, _) = best?;
    Some(SheetLayout {
        rows,
        cols,
        cell_w: width / cols,
        cell_h,
    })
}

/// 在某个格子里的一版姿态。列号决定相位，和 RPMaker 的播放顺序对齐：
///
/// - 3 列：0 是过腿（并脚、躯干最高），1 和 2 是两次触地极值，引擎播 0-1-0-2。
/// - 4 列：0 过腿，1 触地，2 换腿过腿（不是 0 的复制），3 另一侧触地，循环播。
///
/// 4 列那一版的 index 2 之所以不能抄 index 0：抄了就成了一段「站-跨-站-跨-」，
/// 走着走着会每两步顿一下。这里摆臂方向反过来、脚错开一格，读起来才是连续的。
fn pose_for(col: u32, cols: u32, stride: i32, swing: i32) -> Pose {
    let passing = Pose {
        near_foot: 1,
        far_foot: -1,
        near_hand: -1,
        far_hand: 1,
        lift: 1,
    };
    match cols {
        4 => match col {
            0 => passing,
            1 => Pose {
                near_foot: stride,
                far_foot: -stride,
                near_hand: -swing,
                far_hand: swing,
                lift: 0,
            },
            2 => Pose {
                near_foot: -1,
                far_foot: 1,
                near_hand: -swing - 1,
                far_hand: swing - 1,
                lift: 1,
            },
            _ => Pose {
                near_foot: -stride,
                far_foot: stride,
                near_hand: swing,
                far_hand: -swing,
                lift: 0,
            },
        },
        _ => match col {
            1 => Pose {
                near_foot: stride,
                far_foot: -stride,
                near_hand: -swing,
                far_hand: swing,
                lift: 0,
            },
            2 => Pose {
                near_foot: -stride,
                far_foot: stride,
                near_hand: swing,
                far_hand: -swing,
                lift: 0,
            },
            _ => passing,
        },
    }
}

/// 铺白膜（纯函数，可单测）。把 `layer` 的每一帧都填上一版走姿剪影。
///
/// 原地清空目标 cel 再画：白膜是底稿，不是叠加装饰。API 改动只做这一层的
/// 覆盖，用户已经画好的东西在撤销栈里，撤一步就回来。
pub fn lay_base(doc: &mut Document, layer_id: &str) -> Result<PaperDollReport, String> {
    let layout = detect(doc.width, doc.height).ok_or_else(|| {
        format!(
            "{}x{} 不是角色行走图网格（需要 4 行，每格至少 8px）",
            doc.width, doc.height
        )
    })?;
    if !doc.layers.iter().any(|l| l.id == layer_id) {
        return Err(format!("unknown layer: {layer_id}"));
    }
    // 灰阶先进调色板：锁着的层要归队到它的配色范围，没锁就原样进去。
    // picked 是实际落地的颜色（锁层时会归队），idx 是它在调色板里的下标。
    // 两个都要：画图认下标，报告和测试认颜色。
    let mut picked = Vec::with_capacity(BASE_GRAYS.len());
    let mut idx = Vec::with_capacity(BASE_GRAYS.len());
    for gray in BASE_GRAYS {
        let (color, index) = intern_or_nearest(doc, layer_id, gray);
        picked.push(color);
        idx.push(index);
    }
    let frame_ids: Vec<String> = doc.frames.iter().map(|f| f.id.clone()).collect();
    // 画布尺寸先抄出来：下面 cel_mut 借走整个 doc，再读 doc.width 就借两次了。
    let (canvas_w, canvas_h) = (doc.width, doc.height);
    let mut painted = 0usize;
    for frame_id in &frame_ids {
        // 某个帧还没有这个层的 cel（结构操作漏建）就跳过，别为它新建：
        // 少一帧比多一帧错位的格子好，而且 draw 代码不需要 cel 一定存在。
        let Some(cel) = doc.cel_mut(layer_id, frame_id) else {
            continue;
        };
        cel.indices.iter_mut().for_each(|i| *i = 0);
        for row in 0..layout.rows {
            for col in 0..layout.cols {
                // 一格一缓冲：缓冲是格子内坐标系，混用会把上一格的姿势
                // 印到下一格里，四个朝向全糊成最后一格的样子。
                let mut buf = CellBuf::new(layout.cell_w, layout.cell_h);
                paint_cell(&mut buf, layout.facing(row), col, layout.cols, &idx);
                buf.flush(
                    cel,
                    canvas_w,
                    canvas_h,
                    col * layout.cell_w,
                    row * layout.cell_h,
                );
            }
        }
        painted += 1;
    }
    doc.bump();
    Ok(PaperDollReport {
        layout,
        frames_painted: painted,
        grays: picked,
    })
}

/// 进调色板，进不去就归到现有最接近的那一色。
///
/// 调色板 256 项满了还硬要报错的话，一张已经用到 250 色以上的图就没法铺白膜了。
/// 白膜本就是占位灰，落在哪一灰上都不影响用户之后自己上色。
fn intern_or_nearest(doc: &mut Document, layer_id: &str, gray: Rgba) -> (Rgba, u16) {
    // 锁着的层：先归队到它的配色范围，再进调色板。颜色下标一旦定下，
    // 切换范围就会破坏画布上的索引——这是文档模型的既有取舍，见 Document::color_for_layer。
    let picked = doc.color_for_layer(layer_id, gray);
    let index = doc.intern_color(picked).unwrap_or_else(|_| {
        (0..doc.palette.len())
            .min_by_key(|&i| dist2(&doc.palette[i], &picked))
            .map(|i| i as u16 + 1)
            .unwrap_or(1)
    });
    (picked, index)
}

fn dist2(a: &Rgba, b: &Rgba) -> u32 {
    let dr = a.r as i32 - b.r as i32;
    let dg = a.g as i32 - b.g as i32;
    let db = a.b as i32 - b.b as i32;
    let da = a.a as i32 - b.a as i32;
    (dr * dr + dg * dg + db * db + da * da) as u32
}

/// 一格的像素缓冲。格子内坐标系，四周各留 1px 透明边——
//. /// 相邻格子的肢体因此永远贴不到一起，导出的图不会糊成一片。
struct CellBuf {
    w: i32,
    h: i32,
    grid: Vec<u16>,
}

impl CellBuf {
    fn new(w: u32, h: u32) -> Self {
        CellBuf {
            w: w as i32,
            h: h as i32,
            grid: vec![0u16; (w * h) as usize],
        }
    }

    /// 落一格。边外一律丢弃而不是夹到边上：夹会让肢体沿格子边缘画出
    /// 一道假轮廓，看起来像衣服上多了一条边。
    fn set(&mut self, x: i32, y: i32, idx: u16) {
        if x < 1 || y < 1 || x >= self.w - 1 || y >= self.h - 1 {
            return;
        }
        self.grid[(y * self.w + x) as usize] = idx;
    }

    /// 把整格缓冲写进 cel。缩放画布时 cel 比格子长，超出部分保持原样。
    fn flush(&self, cel: &mut Cel, canvas_w: u32, canvas_h: u32, ox: u32, oy: u32) {
        for y in 0..self.h as u32 {
            for x in 0..self.w as u32 {
                if ox + x >= canvas_w || oy + y >= canvas_h {
                    continue;
                }
                cel.set(
                    canvas_w,
                    ox + x,
                    oy + y,
                    self.grid[(y * self.w as u32 + x) as usize],
                );
            }
        }
    }
}

/// 在格子缓冲里画一整个人形。`blob` 是七个灰阶的调色板下标。
fn paint_cell(buf: &mut CellBuf, facing: Facing, col: u32, cols: u32, gray: &[u16]) {
    let (cw, ch) = (buf.w, buf.h);
    let cw = cw as f64;
    let ch = ch as f64;
    let stride = (cw * 0.16).round().max(2.0) as i32;
    let swing = (cw * 0.16).round().max(2.0) as i32;
    let pose = pose_for(col, cols, stride, swing);

    // ---- 竖向比例：先按画布高度算一遍，装不下再整体等比压缩 ----
    let pad_top = 2i32;
    let pad_bottom = (ch / 16.0 + 1.0).round().clamp(2.0, 6.0) as i32;
    let avail = (ch as i32 - pad_top - pad_bottom).max(4) as f64;
    let mut head_d = (ch * 0.26).round().clamp(4.0, 22.0);
    let mut neck = (ch * 0.045).round().clamp(1.0, 3.0);
    let mut torso = (ch * 0.28).round().clamp(3.0, 22.0);
    let mut legs = (ch * 0.30).round().clamp(3.0, 22.0);
    let want = head_d + neck + torso + legs;
    let k = (avail / want).min(1.0);
    head_d = (head_d * k).round().max(3.0);
    neck = (neck * k).round().max(1.0);
    torso = (torso * k).round().max(2.0);
    legs = (legs * k).round().max(2.0);

    let foot_y = ch - pad_bottom as f64 - pose.lift as f64;
    let hip_y = foot_y - legs;
    let shoulder_y = hip_y - torso;
    let head_r = head_d / 2.0;
    let head_cy = shoulder_y - neck - head_r;
    let cx = cw / 2.0;
    let dir = facing.dir() as f64;

    // ---- 横向比例：肩宽撑住衣服，胯窄一点，四肢独立宽度 ----
    let shoulder_w = (cw * 0.34).round().max(4.0);
    let hip_w = (cw * 0.26).round().max(3.0);
    let limb_w = (cw * 0.12).round().max(2.0);
    let shoe_w = limb_w * 0.62;
    let shoe_len = limb_w * 0.85;

    // ---- 远侧肢体：先画，之后被躯干盖住一截，读起来就是在身后 ----
    let far_shoulder = cx + dir.signum() * (shoulder_w / 2.0 - limb_w / 2.0);
    let far_hand = far_shoulder + pose.far_hand as f64;
    tube(
        buf,
        (far_shoulder, shoulder_y + torso * 0.15),
        (far_hand, hip_y + limb_w * 0.4),
        limb_w / 2.0,
        limb_w * 0.42,
        gray[5],
    );
    let far_hip = cx + dir.signum() * (hip_w / 2.0 - limb_w / 2.0);
    let far_foot = far_hip + pose.far_foot as f64;
    tube(
        buf,
        (far_hip, hip_y),
        (far_foot, foot_y - shoe_w),
        limb_w / 2.0,
        limb_w * 0.45,
        gray[5],
    );
    tube(
        buf,
        (far_foot - dir.signum() * shoe_len, foot_y - shoe_w * 0.5),
        (far_foot, foot_y - shoe_w * 0.5),
        shoe_w * 0.6,
        shoe_w * 0.6,
        gray[6],
    );

    // ---- 躯干 + 腰带 ----
    tube(
        buf,
        (cx, shoulder_y),
        (cx, hip_y),
        shoulder_w / 2.0,
        hip_w / 2.0,
        gray[2],
    );
    // 腰带用发色：衣裤分区要能一眼切开，将来换色才知道换到哪儿。
    disc(
        buf,
        cx,
        hip_y + limb_w * 0.2,
        (hip_w / 2.0).min(limb_w * 0.9),
        gray[3],
    );

    // ---- 近侧腿与鞋 ----
    let near_hip = cx - dir.signum() * (hip_w / 2.0 - limb_w / 2.0);
    let near_foot = near_hip + pose.near_foot as f64;
    tube(
        buf,
        (near_hip, hip_y),
        (near_foot, foot_y - shoe_w),
        limb_w / 2.0,
        limb_w * 0.45,
        gray[4],
    );
    tube(
        buf,
        (near_foot - dir.signum() * shoe_len, foot_y - shoe_w * 0.5),
        (near_foot, foot_y - shoe_w * 0.5),
        shoe_w * 0.6,
        shoe_w * 0.6,
        gray[6],
    );

    // ---- 近侧手臂 ----
    let near_shoulder = cx - dir.signum() * (shoulder_w / 2.0 - limb_w / 2.0);
    let near_hand = near_shoulder + pose.near_hand as f64;
    tube(
        buf,
        (near_shoulder, shoulder_y + torso * 0.15),
        (near_hand, hip_y + limb_w * 0.4),
        limb_w / 2.0,
        limb_w * 0.42,
        gray[2],
    );

    // ---- 头、发、五官 ----
    paint_head(buf, cx, head_cy, head_r, facing, gray);
}

/// 头与发。发片按「背面 + 头顶 + 侧鬓」切，背视整颗都是发——
/// 纸娃娃的头发本来就是独立部件，边界画糊了换色就会漏出去。
fn paint_head(buf: &mut CellBuf, cx: f64, cy: f64, r: f64, facing: Facing, gray: &[u16]) {
    let dir = facing.dir() as f64;
    let back = if dir == 0.0 { 0.0 } else { -dir };
    // 后脑垂下来的一束发：侧视时它是区分前后的唯一线索，正背视时是轮廓的一部分。
    let lock_x = cx + back * (r * 0.85);
    if back != 0.0 {
        tube(
            buf,
            (lock_x, cy - r * 0.3),
            (lock_x + back * r * 0.25, cy + r * 0.95),
            r * 0.32,
            r * 0.16,
            gray[3],
        );
    }
    let x0 = (cx - r).floor() as i32;
    let x1 = (cx + r).ceil() as i32;
    let y0 = (cy - r).floor() as i32;
    let y1 = (cy + r).ceil() as i32;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            if dx * dx + dy * dy > r * r + 0.12 {
                continue;
            }
            let nx = dx / r;
            let ny = dy / r;
            // 耳垂在脸之后画，所以在脸的分支里垫上去，再被发盖住半边。
            let is_hair = facing.is_back()
                || ny <= -0.28
                || (back != 0.0 && nx * back >= 0.12)
                || (back == 0.0 && nx.abs() >= 0.66 && ny <= 0.42);
            buf.set(x, y, if is_hair { gray[3] } else { gray[1] });
            // 高光只在脸的左上：光源固定左上，八格共用同一个光向，
            // 不然四行走下来像四盏灯。
            if !is_hair && ny <= -0.42 && nx <= 0.05 {
                buf.set(x, y, gray[0]);
            }
        }
    }
    // 耳朵：正背视两侧都有，侧视只留朝后那一只。
    let ear_r = (r * 0.2).max(1.0);
    for side in [-1.0f64, 1.0] {
        if back != 0.0 && side != back {
            continue;
        }
        disc(
            buf,
            cx + side * (r + ear_r * 0.35),
            cy + r * 0.06,
            ear_r,
            gray[1],
        );
    }
    // 五官：背视没有脸。眼睛用最深的灰，改动范围小，用户重画也最容易。
    if !facing.is_back() {
        let eye = (r * 0.17).max(1.0);
        let offsets: [f64; 2] = if dir == 0.0 {
            [-0.26, 0.26]
        } else {
            [dir, dir]
        };
        for off in offsets {
            disc(buf, cx + off * r, cy - r * 0.02, eye, gray[6]);
        }
    }
}

/// 实心圆。
fn disc(buf: &mut CellBuf, cx: f64, cy: f64, r: f64, idx: u16) {
    let r = r.max(0.55);
    let x0 = (cx - r).floor() as i32;
    let x1 = (cx + r).ceil() as i32;
    let y0 = (cy - r).floor() as i32;
    let y1 = (cy + r).ceil() as i32;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            if dx * dx + dy * dy <= r * r + 0.12 {
                buf.set(x, y, idx);
            }
        }
    }
}

/// 两头粗细不一样的圆头笔（锥形胶囊）。肢体用它：上臂粗、手腕细，
/// 一条线段画不死，两条线段接不齐，锥形正好把「一段肢体」表达成一个调用。
fn tube(buf: &mut CellBuf, from: (f64, f64), to: (f64, f64), r0: f64, r1: f64, idx: u16) {
    let dx = to.0 - from.0;
    let dy = to.1 - from.1;
    let steps = (dx.abs().max(dy.abs()) * 2.0).ceil().max(1.0) as usize;
    for i in 0..=steps {
        let t = i as f64 / steps as f64;
        disc(
            buf,
            from.0 + dx * t,
            from.1 + dy * t,
            r0 + (r1 - r0) * t,
            idx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(w: u32, h: u32) -> Document {
        Document::new("doll", w, h).unwrap()
    }

    #[test]
    fn both_engine_sizes_are_recognised() {
        // VX / Ace / MV / MZ 的 3 列，XP 的 4 列。
        for (w, h, cols, cell) in [
            (144u32, 192u32, 3u32, 48u32),
            (96, 128, 3, 32),
            (128, 128, 4, 32),
            (72, 128, 3, 24),
        ] {
            let got = detect(w, h).unwrap_or_else(|| panic!("{w}x{h} 没认出来"));
            assert_eq!(
                (got.rows, got.cols, got.cell_w, got.cell_h),
                (4, cols, cell, h / 4),
                "{w}x{h}"
            );
        }
        assert!(detect(64, 64).is_some());
        assert!(detect(12, 100).is_none(), "格子太窄，不该硬铺");
    }

    #[test]
    fn a_base_lands_in_every_frame_and_keeps_the_margin() {
        let mut d = doc(144, 192);
        // 补三帧，验证铺的是整列而不是当前帧。帧得走正规操作路径建，
        // cel 才会跟着长出来：直接往 frames 里 push 的新帧没有 cel，
        // 铺白膜的循环一帧都找不到。
        for _ in 0..3 {
            crate::ops::apply_batch(
                &mut d,
                &[crate::ops::PixelOperation::CreateFrame {
                    after: None,
                    duration_ms: 100,
                    id: None,
                }],
            )
            .expect("加帧不该失败");
        }
        let report = lay_base(&mut d, "L0").expect("144x192 该能铺");
        assert_eq!(report.layout.cols, 3);
        assert_eq!(report.frames_painted, 4);
        // 每帧都画了人形，且贴着格子边框的像素一律透明：留 1px 透明边，
        // 相邻两格的肢体才不会糊成一片。
        for frame in &d.frames {
            let cel = d.cel("L0", &frame.id).unwrap();
            let mut painted = 0usize;
            for row in 0..d.height {
                for col in 0..d.width {
                    let value = cel.get(d.width, col, row).unwrap_or(0);
                    if value != 0 {
                        painted += 1;
                    }
                    let on_border =
                        col % 48 == 0 || col % 48 == 47 || row % 48 == 0 || row % 48 == 47;
                    assert!(value == 0 || !on_border, "{},{} 贴边了", col, row);
                }
            }
            assert!(painted > 0, "帧 {} 是空白的", frame.id);
        }
    }

    #[test]
    fn the_four_columns_are_four_real_poses() {
        let mut d = doc(128, 128);
        lay_base(&mut d, "L0").expect("128x128 该能铺");
        let cel = d.cel("L0", "F0").unwrap();
        let cells: Vec<Vec<u16>> = (0..4)
            .map(|c| {
                let mut v = Vec::new();
                for y in 0..32 {
                    for x in 0..32 {
                        v.push(cel.get(d.width, c * 32 + x, y).unwrap());
                    }
                }
                v
            })
            .collect();
        // 每一列都得有人形。
        for (i, cell) in cells.iter().enumerate() {
            assert!(cell.iter().any(|&v| v != 0), "第 {i} 列是空的");
        }
        // 姿势两两不同：index 2 不能是 index 0 的复制。
        assert_ne!(cells[0], cells[2], "index 2 抄了 index 0");
        for i in 1..4 {
            assert_ne!(cells[i - 1], cells[i], "第 {i} 列和第 {} 列一样", i - 1);
        }
    }

    #[test]
    fn a_locked_layer_snaps_the_grays_into_its_range() {
        let mut d = doc(144, 192);
        d.layers[0].locked = true;
        d.layers[0].palette_id = "sweetie16".into();
        let report = lay_base(&mut d, "L0").expect("铺成功");
        // 七个灰阶全部归队到范围里的颜色，没有一个凭空落进调色板。
        let range = d.palette_by_id("sweetie16").unwrap();
        for gray in &report.grays {
            assert!(range.colors.contains(gray), "{gray:?} 没归队");
        }
    }

    #[test]
    fn the_four_rows_are_four_different_facings() {
        let mut d = doc(96, 128);
        lay_base(&mut d, "L0").expect("铺成功");
        let cel = d.cel("L0", "F0").unwrap();
        let grab = |row: u32, col: u32| -> Vec<u16> {
            (0..32)
                .flat_map(|y| (0..32).map(move |x| (col * 32 + x, row * 32 + y)))
                .map(|(x, y)| cel.get(d.width, x, y).unwrap())
                .collect()
        };
        // 正脸画了两只眼，背脸一颗眼都没有：两边像素分布必然不同。
        assert_ne!(grab(0, 0), grab(3, 0), "正脸和背脸一模一样");
        assert_ne!(grab(0, 0), grab(1, 0), "正脸和左侧一模一样");
    }
}
