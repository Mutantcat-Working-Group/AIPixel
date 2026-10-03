// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 调色板文件与交付清单：GPL / PAL / ACT 三套行业标准调色板，外加一份 JSON manifest。
//!
//! 导出的位图只带索引不带名字，引擎和素材库拿到图后仍要知道「这一串 RGB
//! 叫什么、有几个、该切多大」。所以这里把文档的颜色区间按三种标准各写一份
//! 文本或二进制，再附一份清单把尺寸、色数、图层配色范围、帧时长一次说完，
//! 接手的工具不必反推。
//!
//! 三套格式按落地场景选，不重复造轮子：
//! - GPL：GIMP Palette，纯文本，名字 + 每行 `R G B`，跨工具最广
//! - PAL：JASC PAL（`RIFF` / `data` 那套），Aseprite、Photoshop 前后都在用
//! - ACT：Adobe Color Table，二进制，恒 256 槽，Autodesk / Maya / Substance 认
//!
//! 调色板文件只写「不透明的真实颜色」：本项目索引 0 恒为透明且不占调色板
//! 位置，而这三个格式都没有透明槽，写进去反而让下游少一个可用色。

use serde_json::{json, Value};

use super::document::{Document, MAX_PALETTE};

/// PAL / JASC 的固定头两行。
const PAL_HEADER: &str = "RIFF\ndata\n";
/// ACT 的魔数（Adobe Color Table 的 `8BCB`）。
const ACT_MAGIC: [u8; 4] = [0x38, 0x42, 0x43, 0x42];
/// ACT 里透明槽的哨兵值。本项目没有透明槽，写 Adobe 自己的「无透明」。
const ACT_NO_TRANSPARENT: u16 = 0xFFFF;
/// ACT 的总长：魔数 4 + 版本 2 + 256 槽 RGB + 色数 2 + 透明槽 2。
const ACT_LEN: usize = 4 + 2 + MAX_PALETTE * 3 + 2 + 2;
/// 一行排几个色块，写进 GPL 头的 `Columns:`，GIMP 会让色板按它分行。
const GPL_COLUMNS: usize = 16;

/// 真正该写进调色板文件的那套颜色，附它的名字。
///
/// 顺序有讲究：文档自己的 `palette` 是权威；空的才退回「第一个图层认领的
/// 那套范围」，再空退回第一套可用范围；全空就交白卷——编几个颜色凑数的
/// 调色板文件比没有更糟。
fn export_palette(doc: &Document) -> (String, Vec<(u8, u8, u8)>) {
    if !doc.palette.is_empty() {
        let colors = doc.palette.iter().map(|c| (c.r, c.g, c.b)).collect();
        return (doc.name.clone(), colors);
    }
    let wanted = doc
        .layers
        .first()
        .map(|layer| layer.palette_id.as_str())
        .unwrap_or("");
    let chosen = doc
        .palettes
        .iter()
        .find(|palette| palette.id == wanted)
        .or_else(|| doc.palettes.first());
    match chosen {
        Some(palette) => (
            palette.name.clone(),
            palette.colors.iter().map(|c| (c.r, c.g, c.b)).collect(),
        ),
        None => (doc.name.clone(), Vec::new()),
    }
}

/// 只取颜色，名字在 GPL 头和清单里另有一用。
pub fn palette_colors(doc: &Document) -> Vec<(u8, u8, u8)> {
    export_palette(doc).1
}

/// GPL：GIMP Palette 文本。每个真色一行 `R G B<TAB>名字`。
pub fn encode_gpl(doc: &Document) -> Vec<u8> {
    let (name, colors) = export_palette(doc);
    let mut out = String::new();
    out.push_str("GIMP Palette\n");
    out.push_str(&format!("Name: {}\n", sanitize(&name)));
    out.push_str(&format!("Columns: {GPL_COLUMNS}\n"));
    out.push_str("#\n");
    for (index, (r, g, b)) in colors.iter().enumerate() {
        out.push_str(&format!("{r:3} {g:3} {b:3}\t{}\n", label(index)));
    }
    out.into_bytes()
}

/// PAL：JASC 系调色板。`RIFF` + 色数 + `data` + 每行 `R G B`，末尾一个空行。
/// 三种调色板格式的一站式入口：格式名即文件名后缀，认不出就报错。
pub fn encode(doc: &Document, format: &str) -> Result<Vec<u8>, String> {
    match format {
        "gpl" => Ok(encode_gpl(doc)),
        "pal" => Ok(encode_pal(doc)),
        "act" => Ok(encode_act(doc)),
        other => Err(format!("unsupported palette format: {other}")),
    }
}

/// PAL：JASC 系调色板。`RIFF` + 色数 + `data` + 每行 `R G B`，末尾一个空行。
pub fn encode_pal(doc: &Document) -> Vec<u8> {
    let colors = palette_colors(doc);
    let mut out = String::from(PAL_HEADER);
    out.push_str(&format!("{}\n", colors.len()));
    for (r, g, b) in &colors {
        out.push_str(&format!("{r} {g} {b}\n"));
    }
    out.push('\n');
    out.into_bytes()
}

/// ACT：Adobe Color Table，恒 256 槽的二进制。槽位不够就截断——
/// ACT 的槽位是写死的，扩表就等于换个格式。
pub fn encode_act(doc: &Document) -> Vec<u8> {
    let mut out = Vec::with_capacity(ACT_LEN);
    out.extend_from_slice(&ACT_MAGIC);
    out.extend_from_slice(&[0x01, 0x00]); // version 1.0
    let colors = palette_colors(doc);
    for (index, (r, g, b)) in colors.iter().enumerate() {
        if index >= MAX_PALETTE {
            break;
        }
        out.extend_from_slice(&[*r, *g, *b]);
    }
    out.resize(ACT_LEN - 4, 0);
    out.extend_from_slice(&(colors.len() as u16).to_be_bytes());
    out.extend_from_slice(&ACT_NO_TRANSPARENT.to_be_bytes());
    out
}

/// 清单字节。交给引擎和素材库的读者，也留给开发者自己翻。
pub fn encode_manifest(doc: &Document) -> Vec<u8> {
    serde_json::to_vec_pretty(&manifest(doc)).unwrap_or_default()
}

/// 交付清单的 JSON。字段名照下游习惯来：`size` / `palette` / `animations`
/// 是素材库最常见的三个键，不另起炉灶。
pub fn manifest(doc: &Document) -> Value {
    json!({
        "format": "aipixel-manifest/1",
        "name": doc.name,
        "size": { "w": doc.width, "h": doc.height },
        "grid": { "cells": doc.width as usize * doc.height as usize },
        "palette": palette_view(doc),
        "layers": doc.layers.iter().map(|layer| json!({
            "name": layer.name,
            "palette_id": layer.palette_id,
            "locked": layer.locked,
            "visible": layer.visible,
            "opacity": layer.opacity,
        })).collect::<Vec<_>>(),
        "frames": doc.frames.iter().map(|frame| json!({
            "id": frame.id,
            "duration_ms": frame.duration_ms,
        })).collect::<Vec<_>>(),
        "animations": animations(doc),
    })
}

/// 色板视图：主调色板的数量与 hex 列表，外加文档里的全部配色范围。
fn palette_view(doc: &Document) -> Value {
    let (_, colors) = export_palette(doc);
    json!({
        "count": colors.len(),
        "colors": colors
            .iter()
            .map(|(r, g, b)| format!("#{r:02x}{g:02x}{b:02x}"))
            .collect::<Vec<_>>(),
        "ranges": doc.palettes.iter().map(|palette| json!({
            "id": palette.id,
            "name": palette.name,
            "builtin": palette.builtin,
            "count": palette.colors.len(),
            "colors": palette.colors.iter().map(|c| c.to_hex()).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

/// 帧时长表。文档没有「动画」这个一等公民，所以清单老实汇报默认播放段，
/// 再把每帧时长摊开——真要命名剪辑的人在素材库里自己切。
fn animations(doc: &Document) -> Value {
    let ids = doc.frames.iter().map(|f| f.id.clone()).collect::<Vec<_>>();
    if ids.len() > 1 {
        json!({ "loop": { "frames": ids, "loop": true } })
    } else {
        json!({ "static": { "frames": ids, "loop": false } })
    }
}

/// 色块名。GPL 的行尾标签，写「第一个」比写 `Untitled` 好认。
fn label(index: usize) -> String {
    match index {
        0 => "first".to_string(),
        _ => format!("color {index}"),
    }
}

/// 名字里不能有换行：GPL 头是行结构，混进一个换行整份文件就散了。
fn sanitize(name: &str) -> String {
    let cleaned = name.replace(['\n', '\r'], " ");
    if cleaned.trim().is_empty() {
        "AIPixel palette".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rgba;

    /// 一份 3 色文档：GPL 三行、PAL 三行、ACT 恒长且色数写对。
    #[test]
    fn the_three_palette_files_carry_the_same_colors() {
        let mut doc = Document::new("pal", 4, 4).unwrap();
        doc.palette = vec![
            Rgba {
                r: 10,
                g: 20,
                b: 30,
                a: 255,
            },
            Rgba {
                r: 40,
                g: 50,
                b: 60,
                a: 255,
            },
            Rgba {
                r: 70,
                g: 80,
                b: 90,
                a: 255,
            },
        ];
        let gpl = String::from_utf8(encode_gpl(&doc)).unwrap();
        assert!(gpl.starts_with("GIMP Palette\n"));
        assert!(gpl.contains("Columns: 16\n"));
        assert!(gpl.contains("Name: pal\n"));
        for line in [" 10  20  30", " 40  50  60", " 70  80  90"] {
            assert!(gpl.contains(line), "{gpl}");
        }
        assert!(gpl.contains("\tfirst\n"));

        let pal = String::from_utf8(encode_pal(&doc)).unwrap();
        assert!(pal.starts_with("RIFF\ndata\n3\n"));
        assert!(pal.contains("10 20 30\n40 50 60\n70 80 90\n"));
        assert!(pal.ends_with("\n\n"));

        let act = encode_act(&doc);
        assert_eq!(act.len(), ACT_LEN);
        assert_eq!(&act[..4], &ACT_MAGIC);
        assert_eq!(&act[6..9], &[10, 20, 30]);
        assert_eq!(&act[act.len() - 4..act.len() - 2], &[0, 3]);
        assert_eq!(&act[act.len() - 2..], &[0xff, 0xff]);
    }

    /// 文档自己的 palette 是空的，就退回第一个图层认领的那套配色范围，
    /// 所以新文档照样交得出调色板文件（内置范围 Sweetie 16 那 16 色）。
    #[test]
    fn an_empty_palette_exports_a_valid_file() {
        let doc = Document::new("blank", 4, 4).unwrap();
        assert!(doc.palette.is_empty(), "新文档的 palette 确实是空的");
        let scope = doc.layers[0].palette_id.clone();
        let gpl = String::from_utf8(encode_gpl(&doc)).unwrap();
        // 15 色 + 1 行 first，共 16 行带 TAB 的色块。
        assert_eq!(
            gpl.lines().filter(|l| l.contains('\t')).count(),
            16,
            "{gpl}"
        );
        // PAL 的第一行是色数：`RIFF` / `data` / `16` 起头。
        assert_eq!(
            String::from_utf8(encode_pal(&doc)).unwrap().lines().nth(2),
            Some("16")
        );
        assert_eq!(encode_act(&doc).len(), ACT_LEN);
        assert_eq!(manifest(&doc)["palette"]["count"], 16);
        // 用的确实是第一个图层认领的那套范围，不是随手抓的第一套。
        assert!(gpl.contains("Name:"), "{gpl}");
        let name = gpl
            .lines()
            .find_map(|l| l.strip_prefix("Name: "))
            .unwrap_or_default();
        assert_eq!(
            name,
            doc.palette_by_id(&scope)
                .map(|p| p.name.as_str())
                .unwrap_or(""),
            "{gpl}"
        );
    }

    /// 名字里带换行不能把 GPL 冲散。
    #[test]
    fn a_newline_in_the_name_cannot_break_the_file() {
        let mut doc = Document::new("bad\nname", 4, 4).unwrap();
        doc.palette = vec![Rgba {
            r: 1,
            g: 2,
            b: 3,
            a: 255,
        }];
        let gpl = String::from_utf8(encode_gpl(&doc)).unwrap();
        assert_eq!(gpl.lines().count(), 5, "{gpl}");
        assert!(gpl.contains("Name: bad name"));
    }

    /// 清单要把尺寸、色数、图层范围、帧时长都交代清楚。
    #[test]
    fn the_manifest_reports_everything_a_consumer_needs() {
        use super::super::ops::{apply_batch, PixelOperation};

        let mut doc = Document::new("sheet", 32, 16).unwrap();
        doc.palette = vec![Rgba {
            r: 1,
            g: 2,
            b: 3,
            a: 255,
        }];
        let value = manifest(&doc);
        assert_eq!(value["size"]["w"], 32);
        assert_eq!(value["grid"]["cells"], 512);
        assert_eq!(value["palette"]["colors"][0], "#010203");
        assert_eq!(value["layers"][0]["locked"], false);
        assert_eq!(value["animations"]["static"]["loop"], false);

        apply_batch(
            &mut doc,
            &[PixelOperation::CreateFrame {
                after: None,
                duration_ms: 120,
                id: None,
            }],
        )
        .unwrap();
        // 两帧以上就是循环段，下游按剪辑接。
        let value = manifest(&doc);
        assert_eq!(
            value["animations"]["loop"]["frames"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(value["animations"]["loop"]["loop"], true);
        assert_eq!(value["frames"][1]["duration_ms"], 120);
    }
}
