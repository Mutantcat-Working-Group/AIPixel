//! RLE 网格 + legend：agent 读回画布上下文时使用的紧凑文本编码。
//! 这是「权威状态是文本网格」约定的落地格式。

use super::document::{Document, Rgba};

/// 单字符调色板符号表（`.` 恒为透明）。
pub const SYMBOLS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Legend {
    /// 符号 -> 颜色（含 `.` -> 透明）
    pub entries: Vec<(char, Option<Rgba>)>,
}

impl Legend {
    pub fn build(palette: &[Rgba], used: &[u16]) -> Legend {
        let mut entries = vec![('.', None)];
        let mut taken = std::collections::HashSet::new();
        for &idx in used {
            if idx == 0 {
                continue;
            }
            if let Some(c) = SYMBOLS.iter().find(|s| !taken.contains(*s)) {
                taken.insert(*c);
                // cel 索引 1 基：palette[0] 对应索引 1
                entries.push((*c as char, palette.get(idx as usize - 1).copied()));
            }
        }
        Legend { entries }
    }

    pub fn symbol_of(&self, color: Option<Rgba>) -> char {
        self.entries
            .iter()
            .find(|(_, c)| *c == color)
            .map(|(s, _)| *s)
            .unwrap_or('?')
    }

    pub fn to_lines(&self) -> Vec<String> {
        self.entries
            .iter()
            .map(|(sym, color)| match color {
                None => format!("{sym} = transparent"),
                Some(c) => format!("{sym} = {}", c.to_hex()),
            })
            .collect()
    }
}

/// 单行 RLE 编码：`count symbol`，count 为 1 时省略。
pub fn encode_row(row: &[u16], palette: &[Rgba], legend: &Legend) -> String {
    let mut out = String::new();
    let mut iter = row.iter().peekable();
    while let Some(&idx) = iter.next() {
        let mut count = 1usize;
        while let Some(&&next) = iter.peek() {
            if next == idx {
                count += 1;
                iter.next();
            } else {
                break;
            }
        }
        let sym = if idx == 0 {
            '.'
        } else {
            legend.symbol_of(palette.get(idx as usize - 1).copied())
        };
        if count == 1 {
            out.push(sym);
        } else {
            out.push_str(&count.to_string());
            out.push(sym);
        }
    }
    out
}

/// 画布视图：legend + RLE 行，附截断信息。
#[derive(Debug, Clone)]
pub struct CanvasView {
    pub width: u32,
    pub height: u32,
    pub x: u32,
    pub y: u32,
    pub legend: Legend,
    pub rows: Vec<String>,
    /// 原始请求区域被收缩过（预算不足）
    pub shrunk: bool,
}

/// 读取画布窗口并编码为 RLE 文本视图。
/// `max_chars` 限制整个视图的字符预算（不含 legend），不足时向中心收缩。
pub fn render_window(
    doc: &Document,
    layer: &str,
    frame: &str,
    region: Option<(u32, u32, u32, u32)>, // x y w h
    max_chars: usize,
) -> Option<CanvasView> {
    let cel = doc.cel(layer, frame)?;
    let (rw, rh) = (doc.width, doc.height);
    let (mut x, mut y, mut w, mut h) = region.unwrap_or((0, 0, rw, rh));
    x = x.min(rw.saturating_sub(1));
    y = y.min(rh.saturating_sub(1));
    w = w.min(rw - x);
    h = h.min(rh - y);

    loop {
        let mut used: Vec<u16> = Vec::new();
        let mut rows: Vec<Vec<u16>> = Vec::with_capacity(h as usize);
        for cy in y..y + h {
            let mut row = Vec::with_capacity(w as usize);
            for cx in x..x + w {
                let idx = cel.get(rw, cx, cy).unwrap_or(0);
                row.push(idx);
                if !used.contains(&idx) {
                    used.push(idx);
                }
            }
            rows.push(row);
        }
        let legend = Legend::build(&doc.palette, &used);
        let encoded: Vec<String> = rows
            .iter()
            .map(|r| encode_row(r, &doc.palette, &legend))
            .collect();
        let total: usize = encoded.iter().map(|s| s.len() + 1).sum();
        let (cw, ch) = (w as i64, h as i64);
        if total <= max_chars || (w <= 1 && h <= 1) {
            return Some(CanvasView {
                width: w,
                height: h,
                x,
                y,
                legend,
                rows: encoded,
                shrunk: (w, h) != region.map(|(_, _, ow, oh)| (ow, oh)).unwrap_or((w, h)),
            });
        }
        // 向中心收缩一半
        let nw = (cw / 2).max(1);
        let nh = (ch / 2).max(1);
        let nx = x + ((cw - nw) / 2) as u32;
        let ny = y + ((ch - nh) / 2) as u32;
        if (nx, ny, nw as u32, nh as u32) == (x, y, w, h) {
            return Some(CanvasView {
                width: w,
                height: h,
                x,
                y,
                legend,
                rows: encoded,
                shrunk: true,
            });
        }
        x = nx;
        y = ny;
        w = nw as u32;
        h = nh as u32;
    }
}

/// overview 模式：降采样全图（2x2 均值透明/最近邻非透明），适合看构图。
pub fn render_overview(doc: &Document, layer: &str, frame: &str, cell: u32) -> Option<CanvasView> {
    let cel = doc.cel(layer, frame)?;
    let cell = cell.max(1);
    let ow = doc.width.div_ceil(cell);
    let oh = doc.height.div_ceil(cell);
    let mut rows: Vec<Vec<u16>> = Vec::with_capacity(oh as usize);
    let mut used: Vec<u16> = Vec::new();
    for cy in 0..oh {
        let mut row = Vec::with_capacity(ow as usize);
        for cx in 0..ow {
            let mut pick = 0u16;
            for dy in 0..cell {
                for dx in 0..cell {
                    let v = cel
                        .get(
                            doc.width,
                            (cx * cell + dx).min(doc.width - 1),
                            (cy * cell + dy).min(doc.height - 1),
                        )
                        .unwrap_or(0);
                    if v != 0 {
                        pick = v;
                    }
                }
            }
            if !used.contains(&pick) {
                used.push(pick);
            }
            row.push(pick);
        }
        rows.push(row);
    }
    let legend = Legend::build(&doc.palette, &used);
    let encoded = rows
        .iter()
        .map(|r| encode_row(r, &doc.palette, &legend))
        .collect();
    Some(CanvasView {
        width: ow,
        height: oh,
        x: 0,
        y: 0,
        legend,
        rows: encoded,
        shrunk: true,
    })
}
