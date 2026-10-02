// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! RLE 网格 + legend：agent 读回画布上下文时使用的紧凑文本编码。
//! 这是「权威状态是文本网格」约定的落地格式。

use super::document::{Document, Rgba};

/// 单字符调色板符号表（`.` 恒为透明）。
pub const SYMBOLS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Legend {
    /// 符号 -> 颜色（含 `.` -> 透明）
    pub entries: Vec<(char, Option<Rgba>)>,
    /// 悬空符号：行编码里会出现、可调色板里压根没有这个位置的颜色。
    /// cel 比调色板长时才有（索引是从 .aip 或别的进程灌进来的）。
    /// 记在这里而不是塞进 `entries` 的 `None`：`None` 是透明的颜色，
    /// 两者一混，模型就把一段悬空像素读成空白，还"贴心"地帮你擦掉。
    pub dangling: Vec<Unmapped>,
    /// 建这份图例时的调色板长度。符号算法同时看索引和这个长度：
    /// 越界的索引给 `?`。图例不带这个数的话，行编码就得自己判一次越界，
    /// 两处判法迟早长歪——行里写 `?`、图例写 `e = ...`。
    palette_len: usize,
}

/// 读不出颜色的那一种索引。两种都要报，可报的缘由完全不同：
/// 一个是颜色没定义，一个是符号用完了——后者颜色明明有，只是没法用单字符指代。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unmapped {
    /// 索引超出调色板长度：这一格的颜色压根没被定义过。
    OutsidePalette(u16),
    /// 索引在调色板里，可 62 个单字符符号已经发完，只能拿 `?` 顶一下。
    /// 颜色留着：模型还能照着 hex 写回这些像素，不至于整段变成猜。
    SymbolsExhausted { index: u16, color: Rgba },
}

/// 索引 -> 图例符号，和图例共用同一把尺子。
/// 拿不到符号的两种情况一律给 `'?'`：索引越界（cel 比调色板长），以及索引
/// 在调色板里、可 62 个单字符符号已经发完。给 `'.'` 是绝对不行的：那是透明，
/// 拿它顶替会让这些格子静默变成空白，画面少一块还查不出原因。
fn legend_symbol(index: u16, palette_len: usize) -> char {
    if index as usize > palette_len {
        return '?';
    }
    symbol_of_index(index)
}

/// cel 索引 -> 单字符符号。索引 1 基：索引 1 取 `SYMBOLS[0]`。
///
/// 这是全库唯一的索引到符号算法：`Legend::build` 排图例、`encode_row` 写行，
/// 两边必须走同一个函数，否则图例和行编码会各说各话。
fn symbol_of_index(index: u16) -> char {
    if index == 0 {
        return '.';
    }
    SYMBOLS
        .get(index as usize - 1)
        .map(|s| *s as char)
        .unwrap_or('?')
}

impl Legend {
    /// 按调色板位置分配符号：cel 索引 i+1 恒用 `SYMBOLS[i]`。
    ///
    /// 曾经这里按「索引在画面里首次出现的顺序」发符号，而 `.aip` 的 `@palette`
    /// 段和这里的编码各用各的一套。两套表一旦不一致，导出再导入就把颜色整体
    /// 换掉——写读写写字节完全稳定，用户看不到任何报错。用户画的第一种颜色
    /// 恰好是 `palette[0]` 时两套表重合、一切正常，所以旧用例一次都没踩到。
    /// `used` 保留在签名里：调用方都在算它，拿掉会连带改一圈上下文代码。
    pub fn build(palette: &[Rgba], used: &[u16]) -> Legend {
        let mut entries = vec![('.', None)];
        let mut dangling = Vec::new();
        for &idx in used {
            if idx == 0 {
                continue;
            }
            // 图例里的符号必须跟 `symbol_for_index` 用同一套算法，否则行编码
            // 写 `b`、图例写 `a = #xxxx`，模型读到的颜色跟画面整体错位。
            let symbol = legend_symbol(idx, palette.len());
            match palette.get(idx as usize - 1) {
                // 颜色有、符号也有，才进图例。符号用完了的时候别把 ('?', color)
                // 塞进 entries：同一个 `?` 会顶替好几种颜色，模型照着 `?` 写
                // 一笔，整片像素就悄悄收敛成一种色，还看不出哪儿错了。
                Some(color) if symbol != '?' => entries.push((symbol, Some(*color))),
                Some(color) => dangling.push(Unmapped::SymbolsExhausted {
                    index: idx,
                    color: *color,
                }),
                None => dangling.push(Unmapped::OutsidePalette(idx)),
            }
        }
        Legend {
            entries,
            dangling,
            palette_len: palette.len(),
        }
    }

    /// cel 索引 -> 符号。按位置直接算，不按颜色反查。
    ///
    /// 调色板里有重复颜色时，「先按颜色找到的符号」会把第二种颜色的像素也写成
    /// 第一种的符号，导入后整片串成第一种颜色。索引才是权威，颜色不是。
    pub fn symbol_for_index(&self, index: u16) -> char {
        legend_symbol(index, self.palette_len)
    }

    /// 颜色 -> 符号，只用于把「当前选中的颜色是哪个字符」告诉用户。
    ///
    /// 和 `symbol_for_index` 的目的相反：那个是模型的合同，按索引取符号；
    /// 这个反查在调色板有重复颜色时会命中第一个相等项，因此只能拿来
    /// 显示，绝不能拿它决定画布里哪个像素该写成什么。
    pub fn symbol_of(&self, color: Option<Rgba>) -> char {
        self.entries
            .iter()
            .find(|(_, c)| *c == color)
            .map(|(s, _)| *s)
            .unwrap_or('?')
    }

    /// 图例行。除了「符号 = 颜色」，还要把画不进符号的像素报出来：
    ///
    /// cel 下标越出调色板的、单字符符号用尽的，都会以 `? = ...` 的形式
    /// 追加在末尾。不报的话模型只会看到一片空格，以为那里本来就空着，
    /// 而用户的画其实丢了一整块颜色——这类静默丢失比报错难查十倍。
    pub fn to_lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .entries
            .iter()
            .map(|(sym, color)| match color {
                None => format!("{sym} = transparent"),
                Some(c) => format!("{sym} = {}", c.to_hex()),
            })
            .collect();
        let mut outside: Vec<String> = Vec::new();
        let mut exhausted: Vec<String> = Vec::new();
        for entry in &self.dangling {
            match *entry {
                Unmapped::OutsidePalette(idx) => outside.push(idx.to_string()),
                Unmapped::SymbolsExhausted { index, color } => {
                    exhausted.push(format!("{index}={}", color.to_hex()));
                }
            }
        }
        if !outside.is_empty() {
            lines.push(format!(
                "? = unmapped: cel index(es) {} are not in the palette{}",
                capped(&outside, 12),
                if outside.len() > 12 { "; and more" } else { "" }
            ));
            lines.push(
                "    redraw those areas or extend the palette - ? is not a color, do not paint it"
                    .to_string(),
            );
        }
        if !exhausted.is_empty() {
            lines.push(format!(
                "? = symbol limit: {}/{} colors in this view have no single-char symbol: {}",
                exhausted.len(),
                SYMBOLS.len(),
                capped(&exhausted, 8),
            ));
            lines.push(
                "    refer to those pixels by the hex values above; never guess a color from ?"
                    .to_string(),
            );
        }
        lines
    }
}

/// 逗号串起来，太长了掐头并说明还剩多少——图例是给模型读的token预算，
/// 一个 62 色的视图不该把整段上下文撑爆。
fn capped(items: &[String], limit: usize) -> String {
    let joined = items
        .iter()
        .take(limit)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    if items.len() > limit {
        format!("{joined}, and {} more", items.len() - limit)
    } else {
        joined
    }
}

/// 单行 RLE 编码：`count symbol`，count 为 1 时省略。
///
/// 只收一个 `legend`：符号的算法既看索引、也看当初那份调色板有多长，
/// 这俩数都装在 legend 里。以前同时收 palette 和 legend，两处各判一次越界，
/// 判法一长歪，行里写 `?`、图例写 `e = ...`，模型拿两张对不上的表。
pub fn encode_row(row: &[u16], legend: &Legend) -> String {
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
        // 按索引取符号，不按颜色反查：调色板里出现重复颜色时，反查会把两种
        // 颜色都写成同一个符号，读回来就串成一种。
        let sym = legend.symbol_for_index(idx);
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
        let encoded: Vec<String> = rows.iter().map(|r| encode_row(r, &legend)).collect();
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
    let encoded = rows.iter().map(|r| encode_row(r, &legend)).collect();
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
