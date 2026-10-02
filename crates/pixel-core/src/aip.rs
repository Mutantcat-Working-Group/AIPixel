// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! .aip 格式：v1 遗留纯文本矩阵 + v2 增强文本格式。
//!
//! v2 用单字符调色板符号配 RLE 顺手好读，取舍全按本项目自己的约束来：
//! - 图层 / 帧 / cel 直接是一等公民，不必靠额外 id 段变通
//! - revision、图层/帧 id 精确回读，乐观锁跨重启连续
//! - 预留 @anim 元信息（fps/loop），命名带引号可含空格，支持 # 注释
//! - 符号分配与文档 RLE 上下文完全一致（同一套 rle::SYMBOLS）

use super::document::{
    Cel, Document, Frame, Layer, NamedPalette, Rgba, MAX_DIMENSION, MAX_TOTAL_CELLS,
};
use super::rle::{encode_row, SYMBOLS};
use std::collections::{BTreeMap, HashMap};
use std::fmt;

#[derive(Debug)]
pub struct AipError(pub String);

impl fmt::Display for AipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, ".aip error: {}", self.0)
    }
}

impl std::error::Error for AipError {}

type AipResult<T> = Result<T, AipError>;

fn err<T>(msg: impl Into<String>) -> AipResult<T> {
    Err(AipError(msg.into()))
}

/// cel 头的宽高必须落在文档限额之内。Vec 分配是这里唯一的花钱动作，
/// 所以必须在 `Cel::new` 之前把账算清。
fn check_cel_dims(width: u32, height: u32) -> AipResult<()> {
    if width < 1 || height < 1 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return err(format!("cel dims out of range: {width}x{height}"));
    }
    let cells = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| AipError(format!("cel dims overflow: {width}x{height}")))?;
    if cells > MAX_TOTAL_CELLS {
        return err(format!("cel too large: {cells} cells"));
    }
    Ok(())
}

/// 探测文件格式版本。只看开头几个字节：每个被打开的文件都要过一次，
/// 成本必须低到可以忽略；老格式没有这行前缀，只能走 legacy 分支读。
pub fn sniff(text: &str) -> Format {
    let head = text.trim_start();
    if head.starts_with("AIP 2") {
        Format::V2
    } else {
        Format::Legacy
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    V2,
    Legacy,
}

/// 按版本导入 .aip 文本。
pub fn import_any(text: &str) -> AipResult<Document> {
    match sniff(text) {
        Format::V2 => parse_v2(text),
        Format::Legacy => import_legacy(text),
    }
}

// ---------------- v2 ----------------

pub fn dump_v2(doc: &Document) -> AipResult<String> {
    // 符号表就 62 个字符：cel 索引 63 起已经没有符号可发。以前这里写的是
    // `> SYMBOLS.len() + 1`，63 色刚好从护栏底下溜过去，紧接着 `SYMBOLS[62]`
    // 就越界——用户点一次「导出 .aip」，整个进程连着画布一起消失。
    if doc.palette.len() > SYMBOLS.len() {
        return err(format!(
            "palette too large for single-char symbols: {} > {}",
            doc.palette.len(),
            SYMBOLS.len()
        ));
    }
    let symbol_of = |index: usize| -> char {
        if index == 0 {
            '.'
        } else {
            // 护栏是语义约束，这里是「再怎么样都不许 panic」：真 get 不到就当
            // 这格没符号，行编码那侧会给 '?'，也比让进程炸掉好。
            SYMBOLS.get(index - 1).map(|s| *s as char).unwrap_or('?')
        }
    };
    let mut out = String::new();
    out.push_str("AIP 2\n");

    // @meta
    let fps = (1000
        / doc
            .frames
            .first()
            .map(|f| f.duration_ms)
            .unwrap_or(100)
            .max(1))
    .max(1);
    out.push_str(&format!(
        "@meta name=\"{}\" fps={fps} loop=true revision={}\n",
        doc.name.replace('"', "'"),
        doc.revision
    ));

    // @palette
    out.push_str("@palette\n");
    out.push_str(". transparent\n");
    for (i, color) in doc.palette.iter().enumerate() {
        // i 是 0 基调色板下标，cel 索引是 i+1
        out.push_str(&format!("{} {}\n", symbol_of(i + 1), color.to_hex()));
    }

    // @layers
    out.push_str("@layers\n");
    for layer in &doc.layers {
        out.push_str(&format!(
            "{} \"{}\" {} {} palette={} {}\n",
            layer.id,
            layer.name.replace('"', "'"),
            if layer.visible { "visible" } else { "hidden" },
            layer.opacity,
            layer.palette_id,
            if layer.locked { "locked" } else { "unlocked" }
        ));
    }

    // @palettes：命名配色范围。内置的带 builtin 标记，读回来才知道动不得。
    if !doc.palettes.is_empty() {
        out.push_str("@palettes\n");
        for palette in &doc.palettes {
            let kind = if palette.builtin { "builtin" } else { "custom" };
            out.push_str(&format!(
                "{} \"{}\" {}{}\n",
                palette.id,
                palette.name.replace('"', "'"),
                kind,
                palette
                    .colors
                    .iter()
                    .map(|c| format!(" {}", c.to_hex()))
                    .collect::<String>()
            ));
        }
    }

    // @frames
    out.push_str("@frames\n");
    for frame in &doc.frames {
        out.push_str(&format!("{} {}\n", frame.id, frame.duration_ms));
    }

    // @cel 块：legend 必须覆盖所有真实用到的索引，否则 encode_row 会给出 '?'
    let mut used: Vec<u16> = Vec::new();
    for frame in &doc.frames {
        for layer in &doc.layers {
            let Some(cel) = doc.cel(&layer.id, &frame.id) else {
                return err(format!("missing cel {}/{}", layer.id, frame.id));
            };
            for &idx in &cel.indices {
                if idx != 0 && !used.contains(&idx) {
                    used.push(idx);
                }
            }
        }
    }
    let legend = super::rle::Legend::build(&doc.palette, &used);
    for frame in &doc.frames {
        for layer in &doc.layers {
            let cel = doc
                .cel(&layer.id, &frame.id)
                .ok_or_else(|| AipError(format!("missing cel {}/{}", layer.id, frame.id)))?;
            out.push_str(&format!(
                "@cel {} {} {}x{} rle\n",
                layer.id, frame.id, doc.width, doc.height
            ));
            for y in 0..doc.height {
                let row: Vec<u16> = (0..doc.width)
                    .map(|x| cel.get(doc.width, x, y).unwrap_or(0))
                    .collect();
                out.push_str(&encode_row(&row, &legend));
                out.push('\n');
            }
        }
    }

    // @anim：帧序列摘要
    if doc.frames.len() > 1 {
        out.push_str(&format!(
            "@anim sequence fps {fps} loop true frames {}\n",
            doc.frames
                .iter()
                .map(|f| format!(
                    "{}:{}",
                    doc.layers.first().map(|l| l.id.clone()).unwrap_or_default(),
                    f.id
                ))
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    Ok(out)
}

pub fn parse_v2(text: &str) -> AipResult<Document> {
    let mut lines = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .peekable();

    let header = lines
        .peek()
        .copied()
        .ok_or(AipError("empty .aip file".into()))?;
    if header != "AIP 2" {
        return err(format!("unsupported .aip header: {header}"));
    }
    lines.next();

    let mut name = "imported".to_string();
    let mut revision: Option<u64> = None;
    // palette[0] 恒为透明；cel 索引恰好等于它在这个 vec 里的位置
    let mut palette: Vec<Rgba> = vec![Rgba::TRANSPARENT];
    let mut symbol_to_index: HashMap<char, u16> = HashMap::new();
    symbol_to_index.insert('.', 0);
    let mut layers: Vec<Layer> = Vec::new();
    let mut frames: Vec<Frame> = Vec::new();
    let mut palettes: Vec<NamedPalette> = Vec::new();
    let mut cels: BTreeMap<String, BTreeMap<String, Cel>> = BTreeMap::new();
    let mut declared: Option<(u32, u32)> = None;

    // 当前所处的块：v2 写盘格式是「块头一行 + 后续数据行」
    #[derive(PartialEq)]
    enum Block {
        None,
        Palette,
        Layers,
        Palettes,
        Frames,
        Cel,
    }
    let mut block = Block::None;
    let mut cel_key: Option<(String, String)> = None;
    let mut cel_size = (0u32, 0u32);
    let mut cel_read = 0usize;

    for line in lines {
        if line.starts_with('@') {
            // 新块头开始，先收尾上一个 @cel 的行数校验
            if block == Block::Cel && cel_read != cel_size.1 as usize {
                return err(format!("cel has {cel_read} rows, expected {}", cel_size.1));
            }
            if let Some(rest) = line.strip_prefix("@meta ") {
                for (k, v) in kv_pairs(rest) {
                    match k.as_str() {
                        "name" => name = v.trim_matches('"').to_string(),
                        "revision" => revision = v.parse().ok(),
                        _ => {}
                    }
                }
            } else if line == "@palette" {
                block = Block::Palette;
            } else if line == "@layers" {
                block = Block::Layers;
            } else if line == "@palettes" {
                block = Block::Palettes;
            } else if line == "@frames" {
                block = Block::Frames;
            } else if line == "@anim" || line.starts_with("@anim ") {
                // 帧序列摘要仅作元信息，帧结构以 @frames 为准
                block = Block::None;
            } else if let Some(rest) = line.strip_prefix("@cel ") {
                let parts: Vec<&str> = rest.split_whitespace().collect();
                if parts.len() < 3 {
                    return err(format!("bad @cel header: {line}"));
                }
                let (w, h) = parts[2]
                    .split_once('x')
                    .and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?)))
                    .ok_or_else(|| AipError(format!("bad dims in {line}")))?;
                // 先验尺寸再建 cel：Cel::new 是按 w*h 一次性分配的，等行数校验
                // 跑过来时内存早就吃光了。坏文件里写个 100000x100000，整进程直接
                // 被打死，报错都递不到前端。checked_mul 防 32 位乘法溢出成小尺寸。
                check_cel_dims(w, h)?;
                cel_key = Some((parts[0].to_string(), parts[1].to_string()));
                cel_size = (w, h);
                cel_read = 0;
                block = Block::Cel;
            } else {
                return err(format!("unknown directive: {line}"));
            }
            continue;
        }

        match block {
            Block::Palette => parse_palette_line(line, &mut palette, &mut symbol_to_index)?,
            Block::Layers => parse_layer_line(line, &mut layers)?,
            Block::Palettes => parse_named_palette_line(line, &mut palettes)?,
            Block::Frames => parse_frame_line(line, &mut frames)?,
            Block::Cel => {
                let Some((layer, frame)) = cel_key.clone() else {
                    return err("cel rows without a @cel header");
                };
                let (w, h) = cel_size;
                if cel_read >= h as usize {
                    return err(format!("cel {layer}/{frame} has more than {h} rows"));
                }
                // 同一个 cel 头写两遍，第二次的尺寸必须和第一次一致：否则
                // Cel::new 已经按旧尺寸分配过，后面 copy_from_slice 会切出越界片。
                if declared.is_some_and(|d| d != (w, h)) {
                    let (dw, dh) = declared.unwrap_or((w, h));
                    return err(format!("inconsistent cel size {w}x{h} vs {dw}x{dh}"));
                }
                declared = Some((w, h));
                let cols = decode_row(line, &symbol_to_index, w as usize)?;
                if cols.len() != w as usize {
                    return err(format!(
                        "cel {layer}/{frame} row {} has {} cols, expected {w}",
                        cel_read + 1,
                        cols.len()
                    ));
                }
                let cel = cels
                    .entry(layer.clone())
                    .or_default()
                    .entry(frame.clone())
                    .or_insert_with(|| Cel::new(w, h));
                let start = cel_read * w as usize;
                cel.indices[start..start + w as usize].copy_from_slice(&cols);
                cel_read += 1;
            }
            Block::None => return err(format!("unexpected line outside block: {line}")),
        }
    }
    if block == Block::Cel && cel_read != cel_size.1 as usize {
        return err(format!("cel has {cel_read} rows, expected {}", cel_size.1));
    }

    if layers.is_empty() || frames.is_empty() {
        return err("missing @layers or @frames");
    }
    // id 重名会让 cels 的 BTreeMap 把两个图层合成一格：改一个、另一个跟着变，
    // 前端侧栏还会撞出重复的 React key。手工编辑过的文件特别容易出这个。
    if let Some(dup) = first_duplicate(layers.iter().map(|l| l.id.as_str())) {
        return err(format!("duplicate layer id: {dup}"));
    }
    if let Some(dup) = first_duplicate(frames.iter().map(|f| f.id.as_str())) {
        return err(format!("duplicate frame id: {dup}"));
    }
    let (width, height) = declared.ok_or(AipError("no @cel data".into()))?;
    for frames in cels.values() {
        for cel in frames.values() {
            if cel.indices.len() != (width * height) as usize {
                return err(format!(
                    "cel size mismatch: {} vs {width}x{height}",
                    cel.indices.len()
                ));
            }
        }
    }

    let mut doc = Document::new(name, width, height).map_err(|e| AipError(e.to_string()))?;
    // 解析时的 palette[0] 是占位透明，真实调色板从第二项开始
    doc.palette = palette.into_iter().skip(1).collect();
    doc.layers = layers;
    doc.frames = frames;
    doc.cels = cels;
    doc.palettes = palettes;
    doc.revision = revision.unwrap_or(0);
    // 老文件没有配色范围这一段；把内置库补回来、悬空引用重指默认，都在这里收尾。
    doc.ensure_palette_scope();
    // 前面只逐项看了 cel，图层/帧/调色板的条数和总格子数还没过限额——
    // v2 是自己拼文档的，绕过了 Document::new/validated，这里补上同一把尺子。
    doc.check_limits()
        .map_err(|e| AipError(format!("document over limits: {e}")))?;
    Ok(doc)
}

/// 第一个重复出现的值，没有就 None。用于导入时拒掉重名 id。
fn first_duplicate<'a>(ids: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let mut seen: Vec<&str> = Vec::new();
    for id in ids {
        if seen.contains(&id) {
            return Some(id);
        }
        seen.push(id);
    }
    None
}

/// `id "name" builtin|custom #rrggbb ...`。名字可省，颜色一个都不许少。
/// 取出引号内的内容与引号后的剩余部分。缺收尾引号时把整段当内容、剩余为空，
/// 这样残缺文件只会退化解析，不会在多字节边界上切出越界切片。
fn split_quoted(inner: &str) -> (&str, &str) {
    match inner.find('"') {
        Some(end) => (&inner[..end], &inner[end + 1..]),
        None => (inner, ""),
    }
}

fn parse_named_palette_line(line: &str, palettes: &mut Vec<NamedPalette>) -> AipResult<()> {
    let (id, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    let mut rest = rest;
    let mut name = id.to_string();
    if let Some(inner) = rest.trim_start().strip_prefix('"') {
        let (quoted, tail) = split_quoted(inner);
        name = quoted.to_string();
        rest = tail;
    }
    let mut tokens = rest.split_whitespace();
    let builtin = match tokens.next() {
        Some("builtin") => true,
        Some("custom") => false,
        Some(other) => return err(format!("bad palette kind: {other}")),
        None => return err(format!("palette {id} has no colors")),
    };
    let mut colors = Vec::new();
    for token in tokens {
        let color =
            Rgba::parse_hex(token).ok_or_else(|| AipError(format!("bad palette color {token}")))?;
        if !colors.contains(&color) {
            colors.push(color);
        }
    }
    if colors.is_empty() {
        return err(format!("palette {id} has no colors"));
    }
    palettes.push(NamedPalette {
        id: id.to_string(),
        name,
        colors,
        builtin,
    });
    Ok(())
}

/// `key=value` 段：值可以是裸词，也可以是带引号的整串。图层名、
/// 调色板名都允许含空格，所以引号分支不是锦上添花，是名字能存下来的前提。
fn kv_pairs(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(pos) = rest.find('=') {
        let key = rest[..pos].trim().to_string();
        rest = &rest[pos + 1..];
        let value = if rest.starts_with('"') {
            // 引号没闭合就取到行尾：缺的是收尾那半个引号，不是半个键值对。
            let (quoted, tail) = split_quoted(&rest[1..]);
            rest = tail;
            quoted.to_string()
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let value = rest[..end].to_string();
            rest = &rest[end..];
            value
        };
        out.push((key, value));
    }
    out
}

fn parse_palette_line(
    line: &str,
    palette: &mut Vec<Rgba>,
    symbols: &mut HashMap<char, u16>,
) -> AipResult<()> {
    let Some((sym, color)) = line.split_once(char::is_whitespace) else {
        return err(format!("bad palette line: {line}"));
    };
    if sym.chars().count() != 1 {
        return err(format!("palette symbol must be one char: {sym}"));
    }
    let sym = sym.chars().next().unwrap();
    let color = color.trim();
    let rgba = if color == "transparent" {
        Rgba::TRANSPARENT
    } else {
        Rgba::parse_hex(color).ok_or_else(|| AipError(format!("bad palette color {color}")))?
    };
    let idx = palette
        .iter()
        .position(|c| *c == rgba)
        .map(|i| i as u16)
        .unwrap_or_else(|| {
            palette.push(rgba);
            (palette.len() - 1) as u16
        });
    symbols.insert(sym, idx);
    Ok(())
}

fn parse_layer_line(line: &str, layers: &mut Vec<Layer>) -> AipResult<()> {
    let (id, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    // 行格式：`id "name" visible|hidden <opacity> palette=<id> locked`
    // 老文件没有后两段，解析不出来就默认值：颜色范围交给 ensure_palette_scope 收拾。
    let mut rest = rest;
    let mut name = format!("Layer {}", layers.len() + 1);
    if let Some(inner) = rest.trim_start().strip_prefix('"') {
        // 引号名字取到闭合引号为止；没闭合就整段当名字。降级路径见 split_quoted。
        let (quoted, tail) = split_quoted(inner);
        name = quoted.to_string();
        rest = tail;
    }
    let mut visible = true;
    let mut opacity = 255u8;
    let mut palette_id = super::document::MISSING_PALETTE_ID.to_string();
    let mut locked = false;
    for token in rest.split_whitespace() {
        match token {
            "visible" => visible = true,
            "hidden" => visible = false,
            "locked" => locked = true,
            "unlocked" => locked = false,
            other => {
                if let Some(value) = other.strip_prefix("palette=") {
                    palette_id = value.to_string();
                } else if let Ok(parsed) = other.parse::<u8>() {
                    opacity = parsed;
                }
            }
        }
    }
    layers.push(Layer {
        id: id.to_string(),
        name,
        visible,
        opacity,
        palette_id,
        locked,
    });
    Ok(())
}

fn parse_frame_line(line: &str, frames: &mut Vec<Frame>) -> AipResult<()> {
    let mut parts = line.split_whitespace();
    let id = parts
        .next()
        .ok_or(AipError(format!("bad frame line {line}")))?;
    let duration = parts
        .next()
        .and_then(|d| d.parse::<u32>().ok())
        .ok_or(AipError(format!("bad frame duration in {line}")))?;
    frames.push(Frame {
        id: id.to_string(),
        duration_ms: duration.clamp(1, 60_000),
    });
    Ok(())
}

/// RLE 行解码：`12a` = 12 个 'a'，`.` = 透明。
/// 解一行 RLE。`limit` 是这一行的宽度上限：坏文件里一个巨大的 run 计数
/// 会先按 count 把内存吃干，而这一行只可能有 limit 个格子，超了就是坏数据。
fn decode_row(row: &str, symbols: &HashMap<char, u16>, limit: usize) -> Result<Vec<u16>, AipError> {
    let mut out: Vec<u16> = Vec::new();
    let mut chars = row.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            let mut num = String::new();
            while let Some(&d) = chars.peek() {
                if d.is_ascii_digit() {
                    num.push(d);
                    chars.next();
                } else {
                    break;
                }
            }
            let count: usize = num
                .parse()
                .map_err(|_| AipError(format!("bad run count {num}")))?;
            if count > limit || out.len().saturating_add(count) > limit {
                return err(format!("run count {count} exceeds row width {limit}"));
            }
            let sym = chars
                .next()
                .ok_or(AipError("run count without symbol".into()))?;
            let idx = *symbols
                .get(&sym)
                .ok_or_else(|| AipError(format!("unknown palette symbol {sym}")))?;
            out.extend(std::iter::repeat_n(idx, count));
        } else {
            chars.next();
            if out.len() >= limit {
                return err(format!("row wider than {limit}"));
            }
            let idx = *symbols
                .get(&c)
                .ok_or_else(|| AipError(format!("unknown palette symbol {c}")))?;
            out.push(idx);
        }
    }
    Ok(out)
}

// ---------------- v1 legacy ----------------

/// 导入 v1 遗留格式（width/height + image 矩阵 + color 列表）。
pub fn import_legacy(text: &str) -> AipResult<Document> {
    let mut width: Option<u32> = None;
    let mut height: Option<u32> = None;
    let mut image_rows: Vec<Vec<u16>> = Vec::new();
    let mut colors: Vec<Rgba> = vec![Rgba::TRANSPARENT];

    let mut section = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(key) = line.strip_suffix(':') {
            section = key.to_string();
            continue;
        }
        match section.as_str() {
            "image" => {
                let row: Result<Vec<u16>, _> = line
                    .split_whitespace()
                    .map(|t| {
                        t.parse::<u16>()
                            .map_err(|_| AipError(format!("bad pixel index {t}")))
                    })
                    .collect();
                image_rows.push(row?);
            }
            "color" => {
                let c =
                    Rgba::parse_hex(line).ok_or_else(|| AipError(format!("bad color {line}")))?;
                colors.push(c);
            }
            _ => {
                if let Some((key, value)) = line.split_once(':') {
                    match key.trim() {
                        "width" => {
                            width = Some(
                                value
                                    .trim()
                                    .parse()
                                    .map_err(|_| AipError("bad width".into()))?,
                            )
                        }
                        "height" => {
                            height = Some(
                                value
                                    .trim()
                                    .parse()
                                    .map_err(|_| AipError("bad height".into()))?,
                            )
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    let width = width.ok_or(AipError("legacy .aip missing width".into()))?;
    let height = height.ok_or(AipError("legacy .aip missing height".into()))?;
    if image_rows.len() != height as usize {
        return err(format!(
            "image section has {} rows, height says {height}",
            image_rows.len()
        ));
    }
    for row in &image_rows {
        if row.len() != width as usize {
            return err(format!(
                "image row has {} cols, width says {width}",
                row.len()
            ));
        }
    }
    let mut doc =
        Document::new("aip-import", width, height).map_err(|e| AipError(e.to_string()))?;
    // colors[0] 是占位透明，真实调色板从第二项开始
    doc.palette = colors.into_iter().skip(1).collect();
    // 遗留格式不限 color 段条数，顺手按文档限额收一次口。
    doc.check_limits()
        .map_err(|e| AipError(format!("document over limits: {e}")))?;
    let mut frames = BTreeMap::new();
    frames.insert(
        "F0".into(),
        Cel {
            indices: image_rows.into_iter().flatten().collect(),
        },
    );
    doc.cels.insert("L0".into(), frames);
    Ok(doc)
}

/// 回写为 v1 遗留格式（供旧工具链读取）。多图层多帧时按 cel 顺序分段。
pub fn dump_legacy(doc: &Document) -> String {
    let mut out = String::new();
    out.push_str(&format!("width: {}\nheight: {}\n", doc.width, doc.height));
    out.push_str("\nimage:\n");
    for frame in &doc.frames {
        for layer in &doc.layers {
            let cel = doc.cel(&layer.id, &frame.id);
            for y in 0..doc.height {
                let row: Vec<String> = (0..doc.width)
                    .map(|x| {
                        cel.and_then(|c| c.get(doc.width, x, y))
                            .unwrap_or(0)
                            .to_string()
                    })
                    .collect();
                out.push_str(&row.join(" "));
                out.push('\n');
            }
        }
    }
    out.push_str("\ncolor:\n");
    for color in &doc.palette {
        out.push_str(&color.to_hex());
        out.push('\n');
    }
    out
}
