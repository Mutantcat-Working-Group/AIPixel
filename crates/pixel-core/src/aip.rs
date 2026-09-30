// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! .aip 格式：v1 遗留纯文本矩阵 + v2 增强文本格式。
//!
//! v2 参照 PixTXT 的单字符调色板符号 + RLE 思路并做了针对性改进：
//! - 图层 / 帧 / cel 是一等公民（PixTXT 需要 @image id + meta 变通）
//! - revision、图层/帧 id 精确回读，乐观锁跨重启连续
//! - 预留 @anim 元信息（fps/loop），命名带引号可含空格，支持 # 注释
//! - 符号分配与文档 RLE 上下文完全一致（同一套 rle::SYMBOLS）

use super::document::{Cel, Document, Frame, Layer, NamedPalette, Rgba};
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

/// 探测文件格式版本。
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
    if doc.palette.len() > SYMBOLS.len() + 1 {
        return err(format!(
            "palette too large for single-char symbols: {} > {}",
            doc.palette.len(),
            SYMBOLS.len() + 1
        ));
    }
    let symbol_of = |index: usize| -> char {
        if index == 0 {
            '.'
        } else {
            SYMBOLS[index - 1] as char
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
                out.push_str(&encode_row(&row, &doc.palette, &legend));
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
                let cols = decode_row(line, &symbol_to_index)?;
                if cols.len() != w as usize {
                    return err(format!(
                        "cel {layer}/{frame} row {} has {} cols, expected {w}",
                        cel_read + 1,
                        cols.len()
                    ));
                }
                match declared {
                    Some(d) if d != (w, h) => {
                        return err(format!("inconsistent cel size {w}x{h} vs {}x{}", d.0, d.1));
                    }
                    _ => declared = Some((w, h)),
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
    Ok(doc)
}

/// `id "name" builtin|custom #rrggbb ...`。名字可省，颜色一个都不许少。
fn parse_named_palette_line(line: &str, palettes: &mut Vec<NamedPalette>) -> AipResult<()> {
    let (id, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    let mut rest = rest;
    let mut name = id.to_string();
    if let Some(inner) = rest.trim_start().strip_prefix('"') {
        let end = inner.find('"').unwrap_or(inner.len());
        name = inner[..end].to_string();
        rest = &inner[end + 1..];
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

fn kv_pairs(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(pos) = rest.find('=') {
        let key = rest[..pos].trim().to_string();
        rest = &rest[pos + 1..];
        let value = if rest.starts_with('"') {
            let end = rest[1..].find('"').map(|i| i + 1).unwrap_or(rest.len());
            let value = rest[..end + 1].to_string();
            rest = &rest[end + 1..];
            value.trim_matches('"').to_string()
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
        // 引号名字取到闭合引号为止；没闭合就整段当名字，别让解析崩在半个 layer 行上
        let end = inner.find('"').unwrap_or(inner.len());
        name = inner[..end].to_string();
        rest = &inner[end + 1..];
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
fn decode_row(row: &str, symbols: &HashMap<char, u16>) -> Result<Vec<u16>, AipError> {
    let mut out = Vec::new();
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
            let sym = chars
                .next()
                .ok_or(AipError("run count without symbol".into()))?;
            let idx = *symbols
                .get(&sym)
                .ok_or_else(|| AipError(format!("unknown palette symbol {sym}")))?;
            out.extend(std::iter::repeat_n(idx, count));
        } else {
            chars.next();
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
