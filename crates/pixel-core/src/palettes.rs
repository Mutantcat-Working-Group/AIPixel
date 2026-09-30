// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 命名配色范围：图层调色板的一等公民来源。
//!
//! 这里放「内置的那几套」，色值与界面历史上那批预设逐字一致——
//! 老文档读回来时 `ensure_palette_scope` 灌进来的就是同一套，
//! 用户不会看到自己的画面莫名其妙换了色板。
//!
//! 内置那几种是改不得的：用户想改就复制一份再改（见 `fork_builtin`）。
//! 这不是洁癖——内置色板是共享资产，一个用户手滑改花的后果是
//! 所有引用它的图层一起花，而且没法一键还原。

use super::document::{NamedPalette, Rgba};

/// 文档里最多养多少套配色范围。够用，也够挡住手滑生成的一堆空库。
pub const MAX_PALETTES: usize = 64;

/// 一套内置预设。`hex` 全是不带 alpha 的六位小写写法，和 `Rgba::to_hex` 对齐。
struct BuiltinSpec {
    id: &'static str,
    name: &'static str,
    hex: &'static [&'static str],
}

/// 按「自由度」从高到低排：越靠后越是风格化限制，不是越靠后越好。
/// 顺序会原样出现在界面下拉里，也是「第一套当默认」时的兜底次序。
const BUILTIN_SPECS: &[BuiltinSpec] = &[
    BuiltinSpec {
        id: "gray8",
        name: "Gray 8",
        hex: &[
            "#000000", "#242424", "#484848", "#6c6c6c", "#909090", "#b4b4b4", "#d8d8d8", "#ffffff",
        ],
    },
    BuiltinSpec {
        id: "sweetie16",
        name: "Sweetie 16",
        hex: &[
            "#1a1c2c", "#5d275d", "#b13e53", "#ef7d57", "#ffcd75", "#a7f070", "#38b764", "#257179",
            "#29366f", "#3b5dc9", "#41a6f6", "#73eff7", "#f4f4f4", "#94b0c2", "#566c86", "#333c57",
        ],
    },
    BuiltinSpec {
        id: "db16",
        name: "DawnBringer 16",
        hex: &[
            "#140c1c", "#442434", "#30346d", "#4e4a4e", "#854c30", "#346524", "#d04648", "#757161",
            "#597dce", "#d27d2c", "#8595a1", "#6daa2c", "#d2aa99", "#6dc2ca", "#dad45e", "#deeed6",
        ],
    },
    BuiltinSpec {
        id: "pico8",
        name: "PICO-8",
        hex: &[
            "#000000", "#1d2b53", "#7e2553", "#008751", "#ab5236", "#5f574f", "#c2c3c7", "#fff1e8",
            "#ff004d", "#ffa300", "#ffec27", "#00e436", "#29adff", "#83769c", "#ff77a8", "#ffccaa",
        ],
    },
    BuiltinSpec {
        id: "gameboy",
        name: "Game Boy",
        hex: &["#0f380f", "#306230", "#8bac0f", "#9bbc0f"],
    },
    BuiltinSpec {
        id: "onebit",
        name: "1-bit",
        hex: &["#000000", "#ffffff"],
    },
];

/// 全新一套内置库。每次调用都现造，别让调用方拿到能改坏共享状态的引用。
pub fn builtin_palettes() -> Vec<NamedPalette> {
    BUILTIN_SPECS
        .iter()
        .map(|spec| NamedPalette {
            id: spec.id.to_string(),
            name: spec.name.to_string(),
            colors: parse_colors(spec.hex),
            builtin: true,
        })
        .collect()
}

fn parse_colors(hexes: &[&str]) -> Vec<Rgba> {
    hexes
        .iter()
        .filter_map(|hex| Rgba::parse_hex(hex))
        .collect()
}

pub fn is_builtin_id(id: &str) -> bool {
    BUILTIN_SPECS.iter().any(|spec| spec.id == id)
}

/// 名字 -> 稳定小写 id。中文、空格、标点都折叠成连字符，
/// 只保留 ascii 字母数字，免得把空格和引号塞进文件格式里。
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !out.is_empty() && !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "palette".to_string()
    } else {
        trimmed
    }
}

/// 在已有库里给 `base` 找个没占用的 id。撞了就从 `-2` 开始往后数，
/// 不用随机数：同一个名字在同一文档里始终拿到同一个 id，导入导出才稳定。
pub fn unique_palette_id(existing: &[NamedPalette], base: &str) -> String {
    let taken = |id: &str| existing.iter().any(|p| p.id == id);
    if !taken(base) {
        return base.to_string();
    }
    for suffix in 2..=MAX_PALETTES {
        let candidate = format!("{base}-{suffix}");
        if !taken(&candidate) {
            return candidate;
        }
    }
    // 理论上到不了这里：MAX_PALETTES 已经把库里数量卡住了。
    format!("{base}-{}", existing.len() + 1)
}

/// 把内置预设复制成一套可改的。改内置预设时界面与操作层都走这里，
/// 保证「动了内置就落到副本上，原来的那套一个像素都不变」。
pub fn fork_builtin(source: &NamedPalette, existing: &[NamedPalette]) -> NamedPalette {
    let base = format!("{}-copy", slugify(&source.name));
    NamedPalette {
        id: unique_palette_id(existing, &base),
        name: format!("{} copy", source.name),
        colors: source.colors.clone(),
        builtin: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_ids_are_unique() {
        let palettes = builtin_palettes();
        let ids: Vec<&str> = palettes.iter().map(|p| p.id.as_str()).collect();
        let mut deduped = ids.clone();
        deduped.sort_unstable();
        deduped.dedup();
        assert_eq!(ids.len(), deduped.len());
    }

    #[test]
    fn builtin_colors_are_opaque_and_nonempty() {
        for palette in builtin_palettes() {
            assert!(!palette.colors.is_empty(), "{} has no colors", palette.id);
            for color in &palette.colors {
                assert_eq!(color.a, 255, "{} has a transparent entry", palette.id);
            }
        }
    }

    #[test]
    fn slugify_collapses_junk() {
        assert_eq!(slugify("Sweetie 16"), "sweetie-16");
        assert_eq!(slugify("我的配色"), "palette");
        assert_eq!(slugify("  --Hi--  "), "hi");
    }

    #[test]
    fn unique_id_skips_taken_names() {
        let existing = builtin_palettes();
        assert_eq!(unique_palette_id(&existing, "sweetie16"), "sweetie16-2");
        assert_eq!(unique_palette_id(&existing, "fresh"), "fresh");
    }

    #[test]
    fn fork_keeps_source_intact() {
        let existing = builtin_palettes();
        let source = existing.iter().find(|p| p.id == "pico8").unwrap();
        let fork = fork_builtin(source, &existing);
        assert!(!fork.builtin);
        assert_ne!(fork.id, source.id);
        assert_eq!(fork.colors, source.colors);
        // 原有那一套一个色都不动。
        assert_eq!(
            existing.iter().find(|p| p.id == "pico8").unwrap().colors,
            source.colors
        );
    }
}
