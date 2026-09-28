//! 批量工作台：纯本机、确定性的批处理引擎，是 agent 会话的能力补充。
//!
//! 分工的道理：agent 主循环是「一个会话 + 一个模型 + 一个活文档」，一次一两张；
//! 这里是「一个文件夹进、一个文件夹出、一个 token 都不花」。两条操作都是单文件流程的批量形态：
//! 量化把参考图反查成 `.aip`（对应 workflow_pixelize / example/img2aip_converter.py），
//! 导出把 `.aip` 渲染成图（对应 document_export / example/aip_converter.py）。
//!
//! 三条约束：
//! - 全程本机，不碰模型、不进主循环，跑在独立的 `batch-event` 通道上；
//! - 单个文件失败不致命：跳过、记一条原因，继续跑，最后在回执里汇总；
//! - 确定性优先，同样的输入与同样的 recipe 得到同样的输出，方便版本管理与复跑。

use pixel_core::aip;
use pixel_core::decode;
use pixel_core::document::Document;
use pixel_core::pixelize::{PixelizeOptions, PixelizeReport};
use pixel_core::png;
use pixel_core::sheet;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter};

/// 单次扫描/运行列出的文件上限。防呆：用户把整个家目录拖进来时不至于把内存吃穿。
pub const MAX_BATCH_FILES: usize = 4000;

/// 批量工作台专用事件通道，与 agent-event 分开，互不打扰。
pub const BATCH_EVENT_CHANNEL: &str = "batch-event";

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "gif", "tif", "tiff"];

/// 两条本机操作。序列化成 `quantize` / `export`，前端 Segmented 直接拿来当 value。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchKind {
    /// 一批位图 -> 一堆 `.aip`。
    Quantize,
    /// 一批 `.aip` -> 一堆 PNG / GIF。
    Export,
}

/// 一次导出用什么格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Png,
    Gif,
}

/// 批量脚本：可序列化，因此天然是可复跑、可分享的 recipe。
/// 每个字段都有默认值，前端只要补齐目录就能跑。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchRecipe {
    pub kind: BatchKind,
    pub input_dir: String,
    pub output_dir: String,
    /// 量化选项；导出操作忽略它。
    #[serde(default)]
    pub options: PixelizeOptions,
    /// 量化时是否按源图尺寸建画布；false 时用 target_w x target_h + options.fit。
    #[serde(default = "default_true")]
    pub match_source_size: bool,
    #[serde(default = "default_target")]
    pub target_w: u32,
    #[serde(default = "default_target")]
    pub target_h: u32,
    /// 导出格式；量化操作忽略它。
    #[serde(default = "default_export_format")]
    pub export_format: ExportFormat,
}

fn default_true() -> bool {
    true
}

fn default_target() -> u32 {
    64
}

fn default_export_format() -> ExportFormat {
    ExportFormat::Png
}

impl Default for BatchRecipe {
    fn default() -> Self {
        BatchRecipe {
            kind: BatchKind::Quantize,
            input_dir: String::new(),
            output_dir: String::new(),
            options: PixelizeOptions::default(),
            match_source_size: true,
            target_w: 64,
            target_h: 64,
            export_format: ExportFormat::Png,
        }
    }
}

/// 只读扫描：这个目录里到底有几份对口素材。
#[derive(Debug, Clone, Serialize)]
pub struct BatchScan {
    pub kind: BatchKind,
    pub dir: String,
    pub count: usize,
    /// 是否触到了上限（是的话提示用户分批）。
    pub truncated: bool,
    pub files: Vec<String>,
}

/// 单个文件的处理结果，进 batch-event 也进最终回执的明细。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchItemState {
    Ok,
    Skipped,
    Error,
}

/// 广播给前端的批量事件（kind-tagged，与 AgentEvent 同一风格）。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BatchEvent {
    Started {
        total: usize,
    },
    Progress {
        index: usize,
        total: usize,
        file: String,
        state: BatchItemState,
        note: String,
    },
    Done {
        batch_kind: BatchKind,
        ok: usize,
        skipped: usize,
        failed: usize,
        output_dir: String,
    },
}

fn emit(app: &AppHandle, event: BatchEvent) {
    let _ = app.emit(BATCH_EVENT_CHANNEL, event);
}

/// 扫一个目录，数清楚有几份对口素材。只读，不写任何东西。
#[tauri::command]
pub fn batch_scan(input_dir: String, kind: BatchKind) -> Result<BatchScan, String> {
    let dir = PathBuf::from(&input_dir);
    if dir.as_os_str().is_empty() {
        return Err("pick an input folder first".into());
    }
    if !dir.is_dir() {
        return Err(format!("not a folder: {input_dir}"));
    }
    let files = collect_files(&dir, kind)?;
    let truncated = files.len() > MAX_BATCH_FILES;
    let files: Vec<String> = files
        .into_iter()
        .take(MAX_BATCH_FILES)
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    Ok(BatchScan {
        kind,
        dir: input_dir,
        count: files.len(),
        truncated,
        files,
    })
}

/// 跑一次批量。命令立即返回，过程走 `batch-event`；重活在 blocking 线程上做，
/// 不占异步运行时。单个文件失败被吞成 skipped/error 行，整批照跑。
#[tauri::command]
pub fn batch_run(app: AppHandle, recipe: BatchRecipe) -> Result<(), String> {
    let dir = PathBuf::from(&recipe.input_dir);
    if dir.as_os_str().is_empty() {
        return Err("pick an input folder first".into());
    }
    if recipe.output_dir.trim().is_empty() {
        return Err("pick an output folder first".into());
    }
    if !dir.is_dir() {
        return Err(format!("not a folder: {}", recipe.input_dir));
    }
    // 提前列好并校验能写，避免跑到一半才发现输出目录建不出来。
    let files = collect_files(&dir, recipe.kind)?;
    if files.is_empty() {
        return Err(match recipe.kind {
            BatchKind::Quantize => "no images found in that folder".into(),
            BatchKind::Export => "no .aip files found in that folder".into(),
        });
    }
    let files: Vec<PathBuf> = files.into_iter().take(MAX_BATCH_FILES).collect();
    let out_dir = PathBuf::from(&recipe.output_dir);
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        return Err(format!("cannot create output folder: {e}"));
    }

    tokio::task::spawn_blocking(move || {
        run_batch(&app, &recipe, &files, &out_dir);
    });
    Ok(())
}

fn run_batch(app: &AppHandle, recipe: &BatchRecipe, files: &[PathBuf], out_dir: &Path) {
    let total = files.len();
    emit(app, BatchEvent::Started { total });
    let mut ok = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    for (index, path) in files.iter().enumerate() {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let result = match recipe.kind {
            BatchKind::Quantize => quantize_one(path, out_dir, recipe),
            BatchKind::Export => export_one(path, out_dir, recipe),
        };
        let (state, note) = match result {
            Ok(note) => {
                ok += 1;
                (BatchItemState::Ok, note)
            }
            Err(e) => {
                // 以 `skip:` 开头的算跳过（将来哨兵场景），其余算错误。两条都继续跑。
                if let Some(rest) = e.strip_prefix("skip:") {
                    skipped += 1;
                    (BatchItemState::Skipped, rest.to_string())
                } else {
                    failed += 1;
                    (BatchItemState::Error, e)
                }
            }
        };
        emit(
            app,
            BatchEvent::Progress {
                index,
                total,
                file: name,
                state,
                note,
            },
        );
    }
    emit(
        app,
        BatchEvent::Done {
            batch_kind: recipe.kind,
            ok,
            skipped,
            failed,
            output_dir: out_dir.to_string_lossy().into_owned(),
        },
    );
}

/// 一张位图 -> 一个 `.aip`。doc 名字取文件词干，画布尺寸看 recipe。
fn quantize_one(src: &Path, out_dir: &Path, recipe: &BatchRecipe) -> Result<String, String> {
    let bytes = std::fs::read(src).map_err(|e| format!("cannot read: {e}"))?;
    let media = media_type_for(src);
    let (rgba, w, h) = decode::decode_image(&bytes, &media)?;
    let max = pixel_core::document::MAX_DIMENSION;
    let (cw, ch) = if recipe.match_source_size {
        (w.clamp(1, max), h.clamp(1, max))
    } else {
        (recipe.target_w.clamp(1, max), recipe.target_h.clamp(1, max))
    };
    let stem = src
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "untitled".into());
    let mut doc = Document::new(stem, cw, ch).map_err(|e| e.to_string())?;
    let report = pixel_core::pixelize::pixelize_into_cel(
        &mut doc,
        "L0",
        "F0",
        &rgba,
        w,
        h,
        &recipe.options,
    )?;
    let text = aip::dump_v2(&doc).map_err(|e| e.to_string())?;
    let mut out = out_dir.to_path_buf();
    out.push(format!("{}.aip", doc.name));
    std::fs::write(&out, text.as_bytes()).map_err(|e| format!("cannot write: {e}"))?;
    Ok(quantize_note(&report))
}

fn quantize_note(report: &PixelizeReport) -> String {
    format!(
        "{} colors, {} new",
        report.colors_used, report.palette_added
    )
}

/// 一个 `.aip` -> 一张 PNG 或一个 GIF。单帧 .aip 的 GIF 也能出，只是没有动画。
fn export_one(src: &Path, out_dir: &Path, recipe: &BatchRecipe) -> Result<String, String> {
    let text = std::fs::read_to_string(src).map_err(|e| format!("cannot read: {e}"))?;
    let doc = aip::import_any(&text).map_err(|e| e.to_string())?;
    let stem = src
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "untitled".into());
    let (bytes, ext) = match recipe.export_format {
        ExportFormat::Png => {
            let img = png::flatten(&doc);
            (png::encode_png(&img)?, "png")
        }
        ExportFormat::Gif => (sheet::encode_gif(&doc)?, "gif"),
    };
    let mut out = out_dir.to_path_buf();
    out.push(format!("{stem}.{ext}"));
    std::fs::write(&out, &bytes).map_err(|e| format!("cannot write: {e}"))?;
    Ok(format!("{} bytes", bytes.len()))
}

/// 按 kind 收集目录下的对口文件，按名字排序保证确定性（无关文件系统顺序）。
fn collect_files(dir: &Path, kind: BatchKind) -> Result<Vec<PathBuf>, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("cannot read folder: {e}"))?;
    let mut out: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let wanted = match kind {
            BatchKind::Quantize => IMAGE_EXTS.contains(&ext.as_str()),
            BatchKind::Export => ext == "aip",
        };
        if wanted {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

/// 按扩展名摸 media type；认不出来交给 decode 做内容嗅探（当 png）。
fn media_type_for(path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        _ => "image/png",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("aipixel/batch-test").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 造一张 4x4 的红块 PNG（palette index 1）。
    fn red_png_bytes() -> Vec<u8> {
        let mut d = Document::new("red", 4, 4).unwrap();
        d.palette = vec![pixel_core::Rgba::rgb(255, 0, 0)];
        d.cel_mut("L0", "F0")
            .unwrap()
            .indices
            .iter_mut()
            .for_each(|i| *i = 1);
        png::encode_png(&png::flatten(&d)).unwrap()
    }

    #[test]
    fn collects_only_the_wanted_extensions_and_sorts() {
        let dir = tmp("collect");
        std::fs::write(dir.join("b.png"), b"x").unwrap();
        std::fs::write(dir.join("a.png"), b"x").unwrap();
        std::fs::write(dir.join("note.txt"), b"x").unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();
        let imgs = collect_files(&dir, BatchKind::Quantize).unwrap();
        assert_eq!(imgs.len(), 2);
        assert!(imgs[0].ends_with("a.png"));
        assert!(imgs[1].ends_with("b.png"));

        let aips = collect_files(&dir, BatchKind::Export).unwrap();
        assert!(aips.is_empty());
    }

    #[test]
    fn quantize_then_export_round_trips_through_disk() {
        let in_dir = tmp("qin");
        let out_dir = tmp("qout");
        std::fs::write(in_dir.join("sword.png"), red_png_bytes()).unwrap();

        let recipe = BatchRecipe {
            kind: BatchKind::Quantize,
            input_dir: in_dir.to_string_lossy().into_owned(),
            output_dir: out_dir.to_string_lossy().into_owned(),
            options: PixelizeOptions::default(),
            match_source_size: true,
            target_w: 8,
            target_h: 8,
            export_format: ExportFormat::Png,
        };
        let note = quantize_one(&in_dir.join("sword.png"), &out_dir, &recipe).unwrap();
        assert!(note.contains("colors"), "{note}");
        let aip_path = out_dir.join("sword.aip");
        assert!(aip_path.exists());

        // 读回来还是那块红：量化 -> 落盘 -> 解析 -> 渲染，红色铺满 4x4。
        let back = aip::import_any(&std::fs::read_to_string(&aip_path).unwrap()).unwrap();
        let png_bytes = png::encode_png(&png::flatten(&back)).unwrap();
        let (rgba, w, h) = decode::decode_image(&png_bytes, "image/png").unwrap();
        assert_eq!((w, h), (4, 4));
        let red = rgba
            .chunks(4)
            .filter(|p| p[0] == 255 && p[1] == 0 && p[2] == 0)
            .count();
        assert_eq!(red, 16);
    }

    #[test]
    fn target_size_resizes_when_not_matching_source() {
        let in_dir = tmp("resize");
        let out_dir = tmp("resize-out");
        std::fs::write(in_dir.join("s.png"), red_png_bytes()).unwrap();
        let recipe = BatchRecipe {
            kind: BatchKind::Quantize,
            input_dir: in_dir.to_string_lossy().into_owned(),
            output_dir: out_dir.to_string_lossy().into_owned(),
            options: PixelizeOptions::default(),
            match_source_size: false,
            target_w: 16,
            target_h: 16,
            export_format: ExportFormat::Png,
        };
        quantize_one(&in_dir.join("s.png"), &out_dir, &recipe).unwrap();
        let doc =
            aip::import_any(&std::fs::read_to_string(out_dir.join("s.aip")).unwrap()).unwrap();
        assert_eq!((doc.width, doc.height), (16, 16));
    }

    #[test]
    fn export_writes_png_for_a_single_frame_doc() {
        let in_dir = tmp("expin");
        let out_dir = tmp("expout");
        let mut d = Document::new("hero", 4, 4).unwrap();
        d.palette = vec![pixel_core::Rgba::rgb(0, 128, 255)];
        d.cel_mut("L0", "F0")
            .unwrap()
            .indices
            .iter_mut()
            .for_each(|i| *i = 1);
        let text = aip::dump_v2(&d).unwrap();
        std::fs::write(in_dir.join("hero.aip"), text).unwrap();

        let recipe = BatchRecipe {
            kind: BatchKind::Export,
            input_dir: in_dir.to_string_lossy().into_owned(),
            output_dir: out_dir.to_string_lossy().into_owned(),
            options: PixelizeOptions::default(),
            match_source_size: true,
            target_w: 4,
            target_h: 4,
            export_format: ExportFormat::Png,
        };
        let note = export_one(&in_dir.join("hero.aip"), &out_dir, &recipe).unwrap();
        assert!(note.contains("bytes"), "{note}");
        assert!(out_dir.join("hero.png").exists());
    }

    #[test]
    fn a_broken_file_fails_loudly_instead_of_panicking() {
        let in_dir = tmp("bad");
        let out_dir = tmp("bad-out");
        std::fs::write(in_dir.join("junk.png"), b"not a real png").unwrap();
        let recipe = BatchRecipe::default();
        let err = quantize_one(&in_dir.join("junk.png"), &out_dir, &recipe).unwrap_err();
        assert!(!err.is_empty());
    }
}
