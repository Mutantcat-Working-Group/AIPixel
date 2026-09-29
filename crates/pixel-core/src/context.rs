//! agent 画布上下文构建：把文档当前状态渲染成提示词里的 RLE 文本块。
//! 核心约定：文本网格是权威状态，图像只是上下文。

use super::aip;
use super::document::Document;
use super::rle::{render_window, Legend};

/// 单个 cel 的紧凑上下文（legend + RLE 行 + 窗口头）。
pub fn cel_context(
    doc: &Document,
    layer: &str,
    frame: &str,
    max_chars: usize,
) -> Result<String, String> {
    let view =
        render_window(doc, layer, frame, None, max_chars).ok_or("cel not found".to_string())?;
    let mut out = String::new();
    out.push_str(&format!(
        "window {}x{} at ({},{}){}\n",
        view.width,
        view.height,
        view.x,
        view.y,
        if view.shrunk {
            " [shrunk to fit budget]"
        } else {
            ""
        }
    ));
    out.push_str("legend:\n");
    for line in view.legend.to_lines() {
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str("rows (run-length: count*count then symbol, `.` = transparent):\n");
    out.push_str(&view.rows.join("\n"));
    Ok(out)
}

/// overview 上下文：降采样全图构图。
pub fn overview_context(
    doc: &Document,
    layer: &str,
    frame: &str,
    cell: u32,
) -> Result<String, String> {
    let view =
        super::rle::render_overview(doc, layer, frame, cell).ok_or("cel not found".to_string())?;
    let mut out = String::new();
    out.push_str(&format!(
        "overview {}x{} ({}x downsample) at (0,0)\n",
        view.width,
        view.height,
        cell.max(1)
    ));
    out.push_str("legend:\n");
    for line in view.legend.to_lines() {
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str("rows:\n");
    out.push_str(&view.rows.join("\n"));
    Ok(out)
}

/// 文档目录信息（画布上下文前的结构描述）。
pub fn catalog(doc: &Document) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "document: {} ({}x{}, revision {})\n",
        doc.name, doc.width, doc.height, doc.revision
    ));
    out.push_str(&format!(
        "palette ({} colors, index 0 is transparent):\n",
        doc.palette.len()
    ));
    for (i, c) in doc.palette.iter().enumerate() {
        if i == 0 {
            out.push_str("  0 transparent\n");
        } else {
            out.push_str(&format!("  {} {}\n", i, c.to_hex()));
        }
    }
    if !doc.palettes.is_empty() {
        out.push_str(&format!("color ranges ({}):\n", doc.palettes.len()));
        for palette in &doc.palettes {
            out.push_str(&format!(
                "  {} \"{}\" [{}] {} colors\n",
                palette.id,
                palette.name,
                if palette.builtin { "builtin" } else { "custom" },
                palette.colors.len()
            ));
        }
    }
    out.push_str(&format!("layers ({}):\n", doc.layers.len()));
    for (i, l) in doc.layers.iter().enumerate() {
        let range = doc
            .layer_palette(&l.id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "unbounded".to_string());
        out.push_str(&format!(
            "  {i}: {} [{}] opacity={} {visible} range={range} {lock}\n",
            l.id,
            l.name,
            l.opacity,
            visible = if l.visible { "visible" } else { "hidden" },
            lock = if l.locked {
                "LOCKED (only these colors)"
            } else {
                "unlocked"
            }
        ));
    }
    out.push_str(&format!("frames ({}):\n", doc.frames.len()));
    for (i, f) in doc.frames.iter().enumerate() {
        out.push_str(&format!("  {i}: {} duration={}ms\n", f.id, f.duration_ms));
    }
    out
}

/// 提示词「Current canvas context」占位符的完整内容。
pub fn system_context(
    doc: &Document,
    layer: &str,
    frame: &str,
    current_color: Option<&str>,
    max_chars: usize,
) -> String {
    let mut out = catalog(doc);
    out.push('\n');
    out.push_str(&format!("active layer: {layer}\nactive frame: {frame}\n"));
    if let Some(color) = current_color {
        out.push_str(&format!("selected brush color: {color}\n"));
    }
    out.push('\n');
    out.push_str(&cel_context(doc, layer, frame, max_chars).unwrap_or_default());
    out
}

/// 仅导出图例符号表（用于把符号解释回颜色）。
pub fn legend_for(palette: &[super::document::Rgba], used: &[u16]) -> Legend {
    Legend::build(palette, used)
}

/// 把文档完整转成 .aip v2 文本（导出/持久化用）。
pub fn to_aip(doc: &Document) -> Result<String, String> {
    aip::dump_v2(doc).map_err(|e| e.to_string())
}
