// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 精灵表切片：把用户贴进来的「一格一个姿势」的网格图切成一格一格。
//!
//! 为什么需要它：用户拿来的行走图、四方向站位图几乎都是一整张网格表
//! （RPG Maker 的 4 行 x 3 列、Aseprite 导出的表、自己拼的一排帧）。
//! 想让模型「照着这张表改第 3 格」，就得先真的切出第 3 格来——整张表糊在
//! 画布上只会得到一张缩小的、谁也改不动的九宫格。
//!
//! 两条探测路径，都只在「真的有网格」时才认：
//! 1. 透明缝切分：内容之间有空列/空行时，按占用段切。Aseprite 表、自己拼的
//!    一排放透明底的帧，走这条。
//! 2. 模板表匹配：没有缝（角色在格子里居中留白，格子之间不画缝）时，按常见
//!    精灵表版式（4 行 3 列、4 行 4 列……）试切，只有每格都有内容、填充率
//!    相差不大、内容重心贴近格心时才认。
//!
//! 宁可认不出来，也不能把一张完整的画硬切成九宫格：切错的代价是模型拿一个
//! 错位的局部当底图改，画得再好也对不上用户的图。所以两条路径都设了死条件，
//! 不过就放模型手动传 cols/rows（`grid_for` 不谈条件，用户说要切就切）。

use super::document::MAX_DIMENSION;

/// 一列/行上判定「有内容」的 alpha 阈值。与 pixelize 的默认透明阈值一致：
/// 同一条判断标准，免得切片认的内容和量化认的内容不是一拨。
const ALPHA_BUSY: u8 = 128;

/// 最多认多少格。正常精灵表远小于此；超出这个数说明是退化的透明图案
/// （噪点、网点底图）切出来的碎格，不是表。
const MAX_CELLS: usize = 4096;

/// 模板匹配时，每格至少要有这么多比例的内容。空着的格子说明表不是这个版式。
const MIN_CELL_CONTENT: f64 = 0.02;

/// 模板匹配时，最满的格与最空的格的填充率差距上限。
/// 版式对的时候各格是「同一个角色的不同姿势」，量都差不多；差出一倍以上
/// 说明这是把一整张画硬切开了。
const MAX_FILL_SPREAD: f64 = 6.0;

/// 模板匹配时，一格内容最多能占多大比例。
///
/// 真正的精灵表每一格都有透明留白（角色在格内居中）；填满的格子意味着这一
/// 格和邻居连成一片，那是一整张画而不是表。少了这条，任何尺寸整除的实心图
/// ——包括 64x64 的模型成图——都会被认成 4x4 网格。
const MAX_CELL_CONTENT: f64 = 0.98;

/// 模板匹配时，一格内容的重心离格心允许多远（按格子的宽/高取比例）。
/// 网格表里角色在格内居中；差太远的是「画里正好有一块东西落在格心」。
const MAX_CENTER_DRIFT: f64 = 0.25;

/// 一格像素的位置与尺寸。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// 一张切好的精灵表：格子按行主序排，`cells[row * cols + col]`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SheetGrid {
    pub cols: u32,
    pub rows: u32,
    pub cells: Vec<Rect>,
    /// 格子宽/高的代表值：齐整的表各格同尺寸；不齐时取第一格的。给报错和
    /// 摘要文字用，定位永远以 `cells` 为准。
    pub cell_w: u32,
    pub cell_h: u32,
}

impl SheetGrid {
    /// 0 起算的第几格。越界返回 None。
    pub fn cell(&self, index: u32) -> Option<Rect> {
        self.cells.get(index as usize).copied()
    }

    /// 总格数。
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// 第几格是第几行第几列，0 起算。面向人/模型的一句话描述。
    pub fn locate(&self, index: u32) -> String {
        match self.cell(index) {
            None => format!(
                "cell {index} (this grid only has {} cells)",
                self.cells.len()
            ),
            Some(_) => {
                let index = index as usize;
                format!(
                    "cell {index} (row {}, column {})",
                    index / self.cols as usize,
                    index % self.cols as usize
                )
            }
        }
    }
}

/// 探测一张图的网格。认不出来就 None，由调用方决定整张用还是报错。
pub fn detect(rgba: &[u8], width: u32, height: u32) -> Option<SheetGrid> {
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return None;
    }
    if rgba.len() < (width as usize) * (height as usize) * 4 {
        return None;
    }
    gap_grid(rgba, width, height).or_else(|| template_grid(rgba, width, height))
}

/// 不管切成什么样，直接按给定列行数切。模型/用户明说了要这样切时走这条：
/// 探测只是帮忙，说清楚了的要听。
pub fn grid_for(cols: u32, rows: u32, width: u32, height: u32) -> Option<SheetGrid> {
    if cols == 0
        || rows == 0
        || width == 0
        || height == 0
        || (cols as usize).saturating_mul(rows as usize) > MAX_CELLS
    {
        return None;
    }
    let xs = boundaries(width, cols);
    let ys = boundaries(height, rows);
    let mut cells = Vec::with_capacity(xs.len() * ys.len());
    for &(y, y_end) in &ys {
        for &(x, x_end) in &xs {
            cells.push(Rect {
                x,
                y,
                w: x_end - x,
                h: y_end - y,
            });
        }
    }
    Some(SheetGrid {
        cols,
        rows,
        cell_w: cells.first().map(|c| c.w).unwrap_or(0),
        cell_h: cells.first().map(|c| c.h).unwrap_or(0),
        cells,
    })
}

/// 把第 index 格按原尺寸拷出来（RGBA，左上角对齐）。
///
/// 拷的是整个格子而不是内容包围盒：行走图各格得保持同一位置，缩掉一点留白
/// 就会让角色在一帧帧之间跳。
pub fn cell_rgba(
    rgba: &[u8],
    width: u32,
    height: u32,
    grid: &SheetGrid,
    index: u32,
) -> Option<(Vec<u8>, u32, u32)> {
    let cell = grid.cell(index)?;
    if cell.w == 0 || cell.h == 0 {
        return None;
    }
    // 宽高对不上就别切：这个函数是公开 API，调用方拿错一张图的尺寸时，
    // 下面那个 copy_from_slice 会直接 panic，而 panic 在 agent turn 里等于
    // 整个回合蒸发。宁肯返回 None 让上层报一句「这一格取不出来」。
    if rgba.len() < (width as usize) * (height as usize) * 4
        || cell.x + cell.w > width
        || cell.y + cell.h > height
    {
        return None;
    }
    let mut out = vec![0u8; (cell.w * cell.h * 4) as usize];
    for row in 0..cell.h {
        let src = (((cell.y + row) * width + cell.x) * 4) as usize;
        let dst = (row * cell.w * 4) as usize;
        let len = (cell.w * 4) as usize;
        out[dst..dst + len].copy_from_slice(&rgba[src..src + len]);
    }
    Some((out, cell.w, cell.h))
}

/// 把一段长度均分成 parts 份，返回每份的 [start, end)。
/// 除不尽的零头摊到前面的格子上，各格最多差 1px——整表尺寸不整除时也不塌。
fn boundaries(total: u32, parts: u32) -> Vec<(u32, u32)> {
    // 乘法的中间量必须走 u64：解码闸门放行的是「总像素数」不超 4M，
    // 一张 2000000x2 的长条参考图完全合法，而 `total * i` 在 u32 里
    // 4096 等分时能到 8e9，debug 下溢出 panic、release 下绕回，
    // 切出来的格子会莫名其妙少几块。u64 装得下这个量级。
    (0..parts)
        .map(|i| {
            let total = total as u64;
            let parts = parts as u64;
            (
                (total * i as u64 / parts) as u32,
                (total * (i as u64 + 1) / parts) as u32,
            )
        })
        .filter(|(s, e)| e > s)
        .collect()
}

/// 逐列/逐行的占用（有没有一个像素够亮）。一趟扫完出两张表。
fn occupancy(rgba: &[u8], width: u32, height: u32) -> (Vec<bool>, Vec<bool>) {
    let mut columns = vec![false; width as usize];
    let mut rows = vec![false; height as usize];
    for y in 0..height {
        let row = (y * width * 4) as usize;
        for x in 0..width {
            if rgba[row + (x * 4) as usize + 3] >= ALPHA_BUSY {
                columns[x as usize] = true;
                rows[y as usize] = true;
            }
        }
    }
    (columns, rows)
}

/// 占用表切成连续的段，返回每段的 (起点, 长度)。
fn runs(used: &[bool]) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let mut index = 0usize;
    while index < used.len() {
        if !used[index] {
            index += 1;
            continue;
        }
        let start = index;
        while index < used.len() && used[index] {
            index += 1;
        }
        out.push((start as u32, (index - start) as u32));
    }
    out
}

/// 段长里最长与最短的差。格子宽差 1px 是抗锯齿与描边的正常抖动。
fn spread(segs: &[(u32, u32)]) -> u32 {
    let mut min = u32::MAX;
    let mut max = 0;
    for &(_, len) in segs {
        min = min.min(len);
        max = max.max(len);
    }
    max.saturating_sub(min)
}

/// 路径一：透明缝切分。
fn gap_grid(rgba: &[u8], width: u32, height: u32) -> Option<SheetGrid> {
    let (columns, rows) = occupancy(rgba, width, height);
    let col_runs = runs(&columns);
    let row_runs = runs(&rows);
    let total = col_runs.len().saturating_mul(row_runs.len());
    // 一列一段、一行一段 = 整张图没有缝，那不是表。
    if !(2..=MAX_CELLS).contains(&total) {
        return None;
    }
    // 缝切出来的格子必须一样大：宽窄不一是画上去的形状（飘带、触手、鬃毛），
    // 不是格子的间隙。差 1px 放过，那是描边粗细的抖动。
    if spread(&col_runs) > 1 || spread(&row_runs) > 1 {
        return None;
    }
    let mut cells = Vec::with_capacity(total);
    for &(y, y_len) in &row_runs {
        for &(x, x_len) in &col_runs {
            cells.push(Rect {
                x,
                y,
                w: x_len,
                h: y_len,
            });
        }
    }
    // 缝只保证每个格子里「某一行/某列」有内容，不保证交叉点上真的有像素；
    // 整格空白的版式不成立。
    for cell in &cells {
        if cell_stats(rgba, width, height, *cell).count == 0 {
            return None;
        }
    }
    Some(SheetGrid {
        cols: col_runs.len() as u32,
        rows: row_runs.len() as u32,
        cell_w: col_runs[0].1,
        cell_h: row_runs[0].1,
        cells,
    })
}

/// 一格内容的统计量。
#[derive(Default)]
struct CellStats {
    count: u64,
    /// 内容重心，单位是格子内的比例（0..1）。按像素中心算，避免落在格线上。
    center_x: f64,
    center_y: f64,
}

fn cell_stats(rgba: &[u8], width: u32, height: u32, cell: Rect) -> CellStats {
    let mut stats = CellStats::default();
    let mut sum_x = 0u64;
    let mut sum_y = 0u64;
    for y in cell.y..(cell.y + cell.h).min(height) {
        for x in cell.x..(cell.x + cell.w).min(width) {
            let index = (((y * width + x) * 4) + 3) as usize;
            if rgba[index] >= ALPHA_BUSY {
                sum_x += x as u64;
                sum_y += y as u64;
                stats.count += 1;
            }
        }
    }
    if stats.count > 0 {
        stats.center_x =
            (sum_x as f64 + stats.count as f64 / 2.0) / stats.count as f64 - cell.x as f64;
        stats.center_y =
            (sum_y as f64 + stats.count as f64 / 2.0) / stats.count as f64 - cell.y as f64;
        stats.center_x /= cell.w as f64;
        stats.center_y /= cell.h as f64;
    }
    stats
}

/// 路径二：模板表匹配。没缝的精灵表（角色在格内居中留白）走这条。
fn template_grid(rgba: &[u8], width: u32, height: u32) -> Option<SheetGrid> {
    for &(cols, rows) in TEMPLATES {
        // 自动探测只认整除的版式：差一两像素的「差不多」多半是硬切，不认。
        if !width.is_multiple_of(cols) || !height.is_multiple_of(rows) {
            continue;
        }
        let Some(grid) = grid_for(cols, rows, width, height) else {
            continue;
        };
        let mut fills: Vec<f64> = Vec::with_capacity(grid.cells.len());
        let mut ok = true;
        for cell in &grid.cells {
            let stat = cell_stats(rgba, width, height, *cell);
            let fill = stat.count as f64 / (cell.w as f64 * cell.h as f64);
            // 填充率得落在「有内容，但没填满」的区间里：下界挡空格子，
            // 上界挡把整张画硬切开（那样每格都是满的）。
            if !(MIN_CELL_CONTENT..=MAX_CELL_CONTENT).contains(&fill)
                || (stat.center_x - 0.5).abs() > MAX_CENTER_DRIFT
                || (stat.center_y - 0.5).abs() > MAX_CENTER_DRIFT
            {
                ok = false;
                break;
            }
            fills.push(fill);
        }
        if !ok {
            continue;
        }
        let max = fills.iter().cloned().fold(f64::MIN, f64::max);
        let min = fills.iter().cloned().fold(f64::MAX, f64::min);
        if min > 0.0 && max / min <= MAX_FILL_SPREAD {
            return Some(grid);
        }
    }
    None
}

/// 常见精灵表版式，(列, 行)，按出现频率排。
///
/// 前列是 RPG Maker 系（行走图 4 行 x 3/4 列、XP 的 4x4）与常见动画表；
/// 一维条带排在最后，那是「一排帧」，条件同上只是只剩一个方向。
const TEMPLATES: &[(u32, u32)] = &[
    (3, 4),
    (4, 4),
    (4, 3),
    (4, 2),
    (3, 3),
    (4, 5),
    (5, 4),
    (5, 5),
    (3, 5),
    (5, 3),
    (6, 4),
    (4, 6),
    (8, 4),
    (4, 8),
    (6, 6),
    (2, 4),
    (4, 1),
    (3, 1),
    (1, 4),
    (1, 3),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// 一张全透明的画布，附带宽高。
    fn blank(width: u32, height: u32) -> (Vec<u8>, u32, u32) {
        (vec![0u8; (width * height * 4) as usize], width, height)
    }

    fn put(rgba: &mut [u8], width: u32, x: u32, y: u32, color: (u8, u8, u8, u8)) {
        let index = ((y * width + x) * 4) as usize;
        rgba[index..index + 4].copy_from_slice(&[color.0, color.1, color.2, color.3]);
    }

    /// 实心菱形，对角线长度 2r。特意用菱形而不是圆：菱形的边正好切过格子
    /// 中线，相邻两格的内容首尾相接，切不出透明缝——模板匹配那条路才走得通。
    fn diamond(rgba: &mut [u8], width: u32, cx: u32, cy: u32, r: u32, color: (u8, u8, u8, u8)) {
        for y in cy.saturating_sub(r)..=(cy + r).min(rgba.len() as u32 / (width * 4) - 1) {
            for x in cx.saturating_sub(r)..=(cx + r) {
                let dx = (x as i64 - cx as i64).abs();
                let dy = (y as i64 - cy as i64).abs();
                if (dx + dy) as u32 <= r {
                    put(rgba, width, x, y, color);
                }
            }
        }
    }

    const INK: (u8, u8, u8, u8) = (232, 130, 58, 255);

    /// 八个像素一格，两行两列，中间和四周全是透明缝。
    fn gapped_quadrants() -> (Vec<u8>, u32, u32) {
        let (mut rgba, width, height) = blank(8, 8);
        for &(x, y) in &[(0, 0), (4, 0), (0, 4), (4, 4)] {
            for dy in 0..3 {
                for dx in 0..3 {
                    put(&mut rgba, width, x + dx, y + dy, INK);
                }
            }
        }
        (rgba, width, height)
    }

    #[test]
    fn gap_separated_grid_cuts_by_content() {
        let (rgba, width, height) = gapped_quadrants();
        let grid = detect(&rgba, width, height).expect("finds the grid");
        assert_eq!(grid.cols, 2);
        assert_eq!(grid.rows, 2);
        assert_eq!(grid.len(), 4);
        assert_eq!(
            grid.cell(0),
            Some(Rect {
                x: 0,
                y: 0,
                w: 3,
                h: 3
            })
        );
        assert_eq!(
            grid.cell(1),
            Some(Rect {
                x: 4,
                y: 0,
                w: 3,
                h: 3
            })
        );
        assert_eq!(
            grid.cell(2),
            Some(Rect {
                x: 0,
                y: 4,
                w: 3,
                h: 3
            })
        );
        assert_eq!(
            grid.cell(3),
            Some(Rect {
                x: 4,
                y: 4,
                w: 3,
                h: 3
            })
        );
    }

    #[test]
    fn template_matches_a_centred_character_sheet() {
        // 96x128 = 12 格，每格 32x32 的正中一个切过中线的菱形：没有缝可切，
        // 只能走模板表。这正是 RPG Maker 行走图的形状。
        let (mut rgba, width, height) = blank(96, 128);
        for row in 0..4u32 {
            for col in 0..3u32 {
                diamond(
                    &mut rgba,
                    width,
                    col * 32 + 16,
                    row * 32 + 16,
                    // 半径取满格：菱形的边正好咬住格子中线，相邻两格的内容
                    // 首尾相接，一条透明缝都没有——模板匹配这条路才走得通。
                    16,
                    INK,
                );
            }
        }
        let grid = detect(&rgba, width, height).expect("finds the grid");
        assert_eq!((grid.cols, grid.rows), (3, 4));
        assert_eq!(grid.len(), 12);
        assert_eq!(
            grid.cell(2),
            Some(Rect {
                x: 64,
                y: 0,
                w: 32,
                h: 32
            })
        );
    }

    #[test]
    fn single_illustration_is_not_cut_into_a_grid() {
        // 一整张画：没有缝，模板也凑不齐（角上的格子是空的）。这条最关键——
        // 认错的代价是模型拿一个错位的局部当底图。
        let (mut rgba, width, height) = blank(96, 128);
        diamond(&mut rgba, width, 48, 64, 30, INK);
        assert!(detect(&rgba, width, height).is_none());
    }

    #[test]
    fn a_solid_picture_is_never_read_as_a_sheet() {
        // 64x64 全不透明：模型成图最常见的形状，正好还是 (4,4) 的整数倍。
        // 这条守的是「别把一整张画切开」——切错的代价是模型拿一个错位的
        // 局部当底图，用户看见的是「我贴的画被切碎了」。
        let (mut rgba, width, height) = blank(64, 64);
        for y in 0..height {
            for x in 0..width {
                put(&mut rgba, width, x, y, INK);
            }
        }
        assert!(detect(&rgba, width, height).is_none());
    }

    #[test]
    fn an_empty_cell_rules_the_template_out() {
        // 抹掉整格内容：这块版式就不成立了。断言只守「不能照 3x4 切」——
        // 之后换成别的版式还是照旧认不出来，都比认一张有洞的表要好。
        let (mut rgba, width, height) = blank(96, 128);
        for row in 0..4u32 {
            for col in 0..3u32 {
                diamond(&mut rgba, width, col * 32 + 16, row * 32 + 16, 16, INK);
            }
        }
        for y in 0..32u32 {
            for x in 0..32u32 {
                let index = ((y * width + x) * 4) as usize;
                rgba[index..index + 4].copy_from_slice(&[0, 0, 0, 0]);
            }
        }
        let grid = template_grid(&rgba, width, height);
        assert_ne!(
            grid.as_ref().map(|g| (g.cols, g.rows)),
            Some((3, 4)),
            "a sheet with an empty cell must not be cut on that template"
        );
    }

    #[test]
    fn cell_copies_out_its_own_rect() {
        let (rgba, width, height) = gapped_quadrants();
        let grid = detect(&rgba, width, height).expect("finds the grid");
        let (pixels, w, h) = cell_rgba(&rgba, width, height, &grid, 3).expect("cell exists");
        assert_eq!((w, h), (3, 3));
        assert_eq!(pixels.len(), 36);
        // 右下角那一格整格都是墨水，左上角那一格同理，拷出来不许串。
        assert_eq!(&pixels[0..4], &[INK.0, INK.1, INK.2, INK.3]);
        assert_eq!(&pixels[32..36], &[INK.0, INK.1, INK.2, INK.3]);
        assert!(cell_rgba(&rgba, width, height, &grid, 4).is_none());
    }

    #[test]
    fn grid_for_spreads_the_remainder_evenly() {
        // 100 分 3 份除不尽：零头摊给前面的格子，各格最多差 1px，不塌不重叠。
        let grid = grid_for(3, 3, 100, 100).expect("cuts");
        assert_eq!(grid.len(), 9);
        assert_eq!(
            grid.cell(0),
            Some(Rect {
                x: 0,
                y: 0,
                w: 33,
                h: 33
            })
        );
        // 行主序：2 号格还是第 0 行，所以高还是第一行的 33；34 出现在第 2 行。
        assert_eq!(
            grid.cell(2),
            Some(Rect {
                x: 66,
                y: 0,
                w: 34,
                h: 33
            })
        );
        assert_eq!(
            grid.cell(6),
            Some(Rect {
                x: 0,
                y: 66,
                w: 33,
                h: 34
            })
        );
        assert_eq!(
            grid.cell(8),
            Some(Rect {
                x: 66,
                y: 66,
                w: 34,
                h: 34
            })
        );
        assert!(grid_for(0, 2, 8, 8).is_none());
        assert!(grid_for(2, 0, 8, 8).is_none());
    }

    #[test]
    fn locate_names_row_and_column() {
        let grid = grid_for(3, 4, 96, 128).expect("cuts");
        assert_eq!(grid.locate(2), "cell 2 (row 0, column 2)");
        assert_eq!(grid.locate(11), "cell 11 (row 3, column 2)");
        assert_eq!(grid.locate(12), "cell 12 (this grid only has 12 cells)");
    }

    #[test]
    fn ragged_content_is_not_a_grid() {
        // 一块 5px 宽一块 7px 宽：那是画上去的形状（飘带、鬃毛），不是格子的缝。
        // 13x7 更是连一个模板版式都除不尽，把模板那条路也一并堵死——
        // 这两条都该认不出来，而不是硬切一个四不像。
        let (mut rgba, width, height) = blank(13, 7);
        for dy in 0..7u32 {
            for dx in 0..5 {
                put(&mut rgba, width, dx, dy, INK);
            }
            for dx in 6..13 {
                put(&mut rgba, width, dx, dy, INK);
            }
        }
        assert!(detect(&rgba, width, height).is_none());
        // 两种失败不是同一回事，两条路各查一次，别让一条的失败掩盖另一条的。
        assert!(gap_grid(&rgba, width, height).is_none());
        assert!(template_grid(&rgba, width, height).is_none());
        // 但用户/模型明说了就切：说不清的时候不猜，说清了就照办。
        let forced = grid_for(2, 1, 10, 8).expect("cuts");
        assert_eq!(forced.len(), 2);
    }

    /// 超宽长条参考图也要切得完整：以前的 u32 中间量会在 4096 等分时绕回，
    /// 切出来的格子凭空少几块，而这张图本身完全在解码闸门允许的 4M 像素内。
    #[test]
    fn a_very_wide_strip_still_cuts_into_every_cell() {
        let grid = grid_for(4096, 1, 2_000_000, 2).expect("cuts");
        assert_eq!(grid.len(), 4096, "每一格都要在，不能因溢出被丢掉");
        let first = grid.cell(0).expect("first");
        let last = grid.cell(4095).expect("last");
        assert_eq!((first.x, first.w), (0, 488));
        assert_eq!(last.x + last.w, 2_000_000, "最后一格要顶到图右边缘");
    }
}
