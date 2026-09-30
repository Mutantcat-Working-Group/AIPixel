// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
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
use tauri::{AppHandle, Emitter, Manager};

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

/// 一条存下来的配方：名字 + 一份串好的 recipe。名字是主键，同名保存就是覆盖。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchRecipeEntry {
    pub name: String,
    pub recipe: BatchRecipe,
}

/// recipes.json 的落盘结构。与 models.json / mcp.json 并列躺在 app config 目录，
/// 重启后原样恢复。文件坏了只当空簿子：一条坏配方不该挡住整个批量工作台。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RecipesFile {
    #[serde(default)]
    pub entries: Vec<BatchRecipeEntry>,
}

/// 配方簿容量上限。几十条跑熟的配方够用，又不至于把 json 撑成日志。
pub const MAX_RECIPES: usize = 50;

/// 配方名长度上限，按字符数算：中文名二十个字左右就到顶，足以描述一个配方。
pub const MAX_RECIPE_NAME_CHARS: usize = 40;

/// 配方名校验。名字只当 JSON 里的键、不碰文件系统，所以不必防路径穿越，
/// 但控制字符会让界面标签变形、斜杠会让名字看起来像路径，两头都拒掉。
pub fn validate_recipe_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("give the recipe a name".into());
    }
    if name.chars().count() > MAX_RECIPE_NAME_CHARS {
        return Err(format!(
            "recipe name is too long (max {MAX_RECIPE_NAME_CHARS} characters)"
        ));
    }
    if name
        .chars()
        .any(|c| c.is_control() || c == '/' || c == '\\')
    {
        return Err("recipe name cannot contain slashes or control characters".into());
    }
    Ok(name.to_string())
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

    // 同 agent_send_message：同步命令在主线程上执行，那里没有 tokio 运行时上下文。
    tauri::async_runtime::spawn_blocking(move || {
        run_batch(&app, &recipe, &files, &out_dir);
    });
    Ok(())
}

/// app config 目录。配方跟模型配置、MCP 配置躺在同一个地方。
fn config_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map_err(|e| format!("cannot resolve config dir: {e}"))
}

fn load_recipes(app: &AppHandle) -> RecipesFile {
    let Ok(dir) = config_dir(app) else {
        return RecipesFile::default();
    };
    let Ok(text) = std::fs::read_to_string(dir.join("recipes.json")) else {
        return RecipesFile::default();
    };
    match serde_json::from_str::<RecipesFile>(&text) {
        Ok(file) => file,
        Err(e) => {
            eprintln!("recipes.json is broken, starting with an empty recipe book: {e}");
            RecipesFile::default()
        }
    }
}

fn save_recipes(app: &AppHandle, file: &RecipesFile) -> Result<(), String> {
    let dir = config_dir(app)?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create config dir: {e}"))?;
    let text = serde_json::to_string_pretty(file).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("recipes.json"), text)
        .map_err(|e| format!("cannot write recipes.json: {e}"))
}

/// 列配方簿。读失败就当空簿：配方簿不影响别处运行，没读到就是没有。
#[tauri::command]
pub fn batch_recipes_list(app: AppHandle) -> Vec<BatchRecipeEntry> {
    load_recipes(&app).entries
}

/// 存一条配方。同名覆盖，所以「微调后原样存回」不会悄悄多出一份重名的。
#[tauri::command]
pub fn batch_recipe_save(app: AppHandle, name: String, recipe: BatchRecipe) -> Result<(), String> {
    let name = validate_recipe_name(&name)?;
    let mut file = load_recipes(&app);
    if let Some(existing) = file.entries.iter_mut().find(|e| e.name == name) {
        existing.recipe = recipe;
    } else {
        if file.entries.len() >= MAX_RECIPES {
            return Err(format!(
                "recipe book is full (max {MAX_RECIPES}), delete one first"
            ));
        }
        file.entries.push(BatchRecipeEntry { name, recipe });
    }
    save_recipes(&app, &file)
}

/// 删一条配方。删不存在的名字不算错误：界面上连点两次和点一次没有区别。
#[tauri::command]
pub fn batch_recipe_delete(app: AppHandle, name: String) -> Result<(), String> {
    let name = validate_recipe_name(&name)?;
    let mut file = load_recipes(&app);
    let before = file.entries.len();
    file.entries.retain(|e| e.name != name);
    if file.entries.len() == before {
        return Ok(());
    }
    save_recipes(&app, &file)
}

/// `.aipr` 的格式标识与版本号。带标识是为了让「拿错文件」在第一眼就被拒掉，
/// 而不是解析到一半才报一句看不懂的话。
pub const RECIPE_FILE_MAGIC: &str = "aipixel-recipe-book";
pub const RECIPE_FILE_VERSION: u32 = 1;

/// `.aipr` 是拿来分享的，无限宽容就会把本机簿子撑爆：条数先设一道上限。
pub const MAX_RECIPE_FILE_ENTRIES: usize = 200;

/// 从文件里扣出来的一条：要么是能用的配方，要么带着一条读不懂的原因。
/// 一条坏记录不该让整份文件作废，所以分开抱着走。
#[derive(Debug, Clone, PartialEq)]
pub enum RecipeFileEntry {
    Entry(BatchRecipeEntry),
    Broken { label: String, note: String },
}

/// 一条配方进来之后去了哪儿。UI 按 state 挑文案，改名的映射单独放在 final_name。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecipeImportState {
    Imported,
    Renamed,
    Skipped,
}

/// 回执里的一行：文件里请求的名字、实际落下的名字、跳过原因。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecipeImportRow {
    /// 文件里请求的名字；坏记录里可能只是个序号（#3）。
    pub name: String,
    /// 实际落下的名字；跳过时是空串。
    pub final_name: String,
    pub state: RecipeImportState,
    /// 为什么跳过；进簿子了就是空串。
    pub note: String,
}

/// 导入回执：逐条交代，外加合并后的整本簿子。前端拿 entries 直接刷新视图，
/// 不必再问一次 Rust。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecipeImportReport {
    pub rows: Vec<RecipeImportRow>,
    pub entries: Vec<BatchRecipeEntry>,
}

/// `.aipr` 的落盘信封：标识 + 版本 + 条目。读的时候不直接用它，
/// 而是走 `parse_recipe_book` 的宽容解析，坏条目在那儿单独抱着走。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecipeBookFile {
    pub format: String,
    pub version: u32,
    #[serde(default)]
    pub entries: Vec<BatchRecipeEntry>,
}

impl RecipeBookFile {
    fn new(entries: Vec<BatchRecipeEntry>) -> Self {
        Self {
            format: RECIPE_FILE_MAGIC.to_string(),
            version: RECIPE_FILE_VERSION,
            entries,
        }
    }
}

/// 读一份 `.aipr`。信封（标识 / 版本 / 条数）错了整份作废；
/// 单条坏了只坏那一条，剩下的照样能进来。
pub fn parse_recipe_book(text: &str) -> Result<Vec<RecipeFileEntry>, String> {
    #[derive(Deserialize)]
    struct Envelope {
        format: String,
        version: u32,
        #[serde(default)]
        entries: Vec<serde_json::Value>,
    }
    let file: Envelope =
        serde_json::from_str(text).map_err(|e| format!("not a readable .aipr file: {e}"))?;
    if file.format != RECIPE_FILE_MAGIC {
        return Err("not an AIPixel recipe file".into());
    }
    if file.version > RECIPE_FILE_VERSION {
        return Err(format!(
            "this recipe file needs a newer AIPixel (format v{})",
            file.version
        ));
    }
    if file.entries.len() > MAX_RECIPE_FILE_ENTRIES {
        return Err(format!(
            "this file holds more than {MAX_RECIPE_FILE_ENTRIES} recipes, split it first"
        ));
    }
    Ok(file
        .entries
        .into_iter()
        .enumerate()
        .map(|(index, raw)| {
            // 名字先单独捞一次：整条解析失败时，回执里好歹能说清是哪一条。
            let hint = raw
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            match serde_json::from_value::<BatchRecipeEntry>(raw) {
                Ok(entry) => RecipeFileEntry::Entry(entry),
                Err(e) => RecipeFileEntry::Broken {
                    label: safe_label(&hint, index),
                    note: e.to_string(),
                },
            }
        })
        .collect())
}

/// 坏记录的展示名：优先用它自己的名字，没有就用序号。
/// 控制字符会弄坏界面标签，一律抹掉。
fn safe_label(raw: &str, index: usize) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_RECIPE_NAME_CHARS)
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        format!("#{}", index + 1)
    } else {
        trimmed.to_string()
    }
}

/// 把一份文件并进本机簿子。同名不覆盖：本机那条是好容易调出来的，
/// 被一个同名文件悄悄换掉太冤；加个序号落进来，回执里写明落在哪个名字下。
pub fn merge_recipes(
    current: &[BatchRecipeEntry],
    incoming: Vec<RecipeFileEntry>,
) -> RecipeImportReport {
    let mut entries: Vec<BatchRecipeEntry> = current.to_vec();
    let mut rows = Vec::new();
    for item in incoming {
        match item {
            RecipeFileEntry::Entry(entry) => {
                let name = match validate_recipe_name(&entry.name) {
                    Ok(name) => name,
                    Err(note) => {
                        let label = safe_label(&entry.name, rows.len());
                        rows.push(RecipeImportRow {
                            name: label,
                            final_name: String::new(),
                            state: RecipeImportState::Skipped,
                            note,
                        });
                        continue;
                    }
                };
                if entries.len() >= MAX_RECIPES {
                    rows.push(RecipeImportRow {
                        name,
                        final_name: String::new(),
                        state: RecipeImportState::Skipped,
                        note: "recipe book is full".into(),
                    });
                    continue;
                }
                if entries.iter().any(|e| e.name == name) {
                    let final_name = unique_recipe_name(&name, &entries);
                    rows.push(RecipeImportRow {
                        name,
                        final_name: final_name.clone(),
                        state: RecipeImportState::Renamed,
                        note: String::new(),
                    });
                    entries.push(BatchRecipeEntry {
                        name: final_name,
                        recipe: entry.recipe,
                    });
                } else {
                    rows.push(RecipeImportRow {
                        name: name.clone(),
                        final_name: name.clone(),
                        state: RecipeImportState::Imported,
                        note: String::new(),
                    });
                    entries.push(BatchRecipeEntry {
                        name,
                        recipe: entry.recipe,
                    });
                }
            }
            RecipeFileEntry::Broken { label, note } => rows.push(RecipeImportRow {
                name: label,
                final_name: String::new(),
                state: RecipeImportState::Skipped,
                note,
            }),
        }
    }
    RecipeImportReport { rows, entries }
}

/// 给撞名的配方找个落脚名：原名、原名 (2)、原名 (3)……原名太长就截尾巴，
/// 序号和长度上限两头都保住。
fn unique_recipe_name(base: &str, entries: &[BatchRecipeEntry]) -> String {
    let free = |candidate: &str| !entries.iter().any(|e| e.name == candidate);
    let candidate = (2..=MAX_RECIPES + 1).find_map(|suffix| {
        let tail = format!(" ({suffix})");
        let head: String = base
            .chars()
            .take(MAX_RECIPE_NAME_CHARS.saturating_sub(tail.chars().count()))
            .collect();
        let full = format!("{head}{tail}");
        free(&full).then_some(full)
    });
    // 簿子最多 50 条，理论上必有空位；兜底截断原名，绝不死循环。
    candidate.unwrap_or_else(|| base.chars().take(MAX_RECIPE_NAME_CHARS).collect())
}

/// 落盘时补后缀：用户在对话框里手打的名字不保证带 `.aipr`，
/// 补一次总比存出一个没有关联程序的文件强。
fn with_recipe_extension(path: &str) -> String {
    let has = Path::new(path)
        .extension()
        .map(|e| e.eq_ignore_ascii_case("aipr"))
        .unwrap_or(false);
    if has {
        path.to_string()
    } else {
        format!("{path}.aipr")
    }
}

/// 把这几条配方写成 `.aipr`。条目由前端给——可能是簿子里的，也可能是手上这份
/// 还没存过的——Rust 只负责校验名字和落盘：调完参数直接分享，不必先存进簿子。
/// 同名两条一起写会被拒：分享出去的文件自己撞名，导入时只会得到一堆改名。
#[tauri::command]
pub fn batch_recipe_export(entries: Vec<BatchRecipeEntry>, path: String) -> Result<String, String> {
    if entries.is_empty() {
        return Err("no recipe selected for export".into());
    }
    let mut picked: Vec<BatchRecipeEntry> = Vec::with_capacity(entries.len());
    for entry in entries {
        let name = validate_recipe_name(&entry.name)?;
        if picked.iter().any(|e| e.name == name) {
            return Err(format!("recipe listed twice: {name}"));
        }
        picked.push(BatchRecipeEntry {
            name,
            recipe: entry.recipe,
        });
    }
    let out = with_recipe_extension(&path);
    if let Some(parent) = Path::new(&out).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
    }
    let text =
        serde_json::to_string_pretty(&RecipeBookFile::new(picked)).map_err(|e| e.to_string())?;
    std::fs::write(&out, text).map_err(|e| format!("cannot write {out}: {e}"))?;
    Ok(out)
}

/// 从 `.aipr` 读配方并进来。回执逐条交代；一条都没进来就不落盘。
#[tauri::command]
pub fn batch_recipe_import(app: AppHandle, path: String) -> Result<RecipeImportReport, String> {
    let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let incoming = parse_recipe_book(&text)?;
    let book = load_recipes(&app);
    let report = merge_recipes(&book.entries, incoming);
    let changed = report
        .rows
        .iter()
        .any(|r| r.state != RecipeImportState::Skipped);
    if changed {
        save_recipes(
            &app,
            &RecipesFile {
                entries: report.entries.clone(),
            },
        )?;
    }
    Ok(report)
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

    #[test]
    fn recipe_names_get_trimmed_and_bad_ones_refused() {
        assert_eq!(validate_recipe_name("  sword-32  ").unwrap(), "sword-32");
        assert!(validate_recipe_name("   ").is_err());
        assert!(validate_recipe_name("sword/32").is_err());
        assert!(validate_recipe_name("sword\n32").is_err());
        let too_long = "x".repeat(MAX_RECIPE_NAME_CHARS + 1);
        assert!(validate_recipe_name(&too_long).is_err());
        // 中文按字符数算：二十个字到不了上限。
        assert!(validate_recipe_name("一把长剑的三十二色配方").is_ok());
    }

    #[test]
    fn recipe_book_round_trips_as_json_with_defaults_for_missing_fields() {
        let file = RecipesFile {
            entries: vec![BatchRecipeEntry {
                name: "sword".into(),
                recipe: BatchRecipe::default(),
            }],
        };
        let text = serde_json::to_string_pretty(&file).unwrap();
        let back: RecipesFile = serde_json::from_str(&text).unwrap();
        assert_eq!(back, file);

        // 前端早先存下的老配方可能缺新字段：#[serde(default)] 得把它们补齐。
        let legacy = r#"{"entries":[{"name":"old","recipe":{"kind":"quantize","input_dir":"","output_dir":""}}]}"#;
        let back: RecipesFile = serde_json::from_str(legacy).unwrap();
        assert_eq!(back.entries[0].recipe, BatchRecipe::default());

        // 连 options 里单个字段缺了也要补齐：老配方缺一个选项，整本簿子不该跟着报废。
        let partial = r#"{"entries":[{"name":"old","recipe":{"kind":"quantize","input_dir":"","output_dir":"","options":{"dither":true}}}]}"#;
        let back: RecipesFile = serde_json::from_str(partial).unwrap();
        let options = &back.entries[0].recipe.options;
        assert!(options.dither);
        assert_eq!(options.max_colors, 32);
        assert_eq!(options.alpha_threshold, 128);
        assert_eq!(options.fit, pixel_core::pixelize::FitMode::Contain);
    }

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

    fn recipe_named(name: &str) -> BatchRecipeEntry {
        BatchRecipeEntry {
            name: name.into(),
            recipe: BatchRecipe::default(),
        }
    }

    #[test]
    fn recipe_book_file_round_trips_and_turns_away_strangers() {
        let file = RecipeBookFile::new(vec![recipe_named("sword")]);
        let text = serde_json::to_string_pretty(&file).unwrap();
        let back = parse_recipe_book(&text).unwrap();
        assert_eq!(back, vec![RecipeFileEntry::Entry(recipe_named("sword"))]);

        // 拿错文件：一句人话，不是解析到一半才崩。
        assert!(parse_recipe_book(r#"{"entries":[]}"#).is_err());
        let stranger = r#"{"format":"some-other-tool","version":1,"entries":[]}"#;
        assert!(parse_recipe_book(stranger)
            .unwrap_err()
            .contains("not an AIPixel recipe file"));

        // 未来版本存下的文件：明说需要更新的 AIPixel，别硬读。
        let newer = r#"{"format":"aipixel-recipe-book","version":99,"entries":[]}"#;
        assert!(parse_recipe_book(newer)
            .unwrap_err()
            .contains("newer AIPixel"));
    }

    #[test]
    fn one_broken_entry_does_not_sink_the_whole_file() {
        // 原样字符串里的花括号不用转义：这条 manifest 按字面写，读出来才是文件真容。
        let text = r#"{"format":"aipixel-recipe-book","version":1,"entries":[
            {"name":"sword","recipe":{"kind":"quantize","input_dir":"in","output_dir":"out"}},
            {"name":"broken","recipe":{"input_dir":"in"}},
            {"name":"also-fine","recipe":{"kind":"export","input_dir":"in","output_dir":"out"}}
        ]}"#;
        let parsed = parse_recipe_book(text).unwrap();
        assert_eq!(parsed.len(), 3);
        let report = merge_recipes(&[], parsed);
        // 两条好的进来，坏的那条被点名跳过。
        assert_eq!(report.entries.len(), 2);
        let broken = report
            .rows
            .iter()
            .find(|r| r.state == RecipeImportState::Skipped)
            .expect("坏条目该被跳过");
        assert_eq!(broken.name, "broken");
        assert!(!broken.note.is_empty());
    }

    #[test]
    fn merging_renames_collisions_instead_of_overwriting_local_recipes() {
        let current = vec![recipe_named("sword")];
        let incoming = vec![
            RecipeFileEntry::Entry(recipe_named("sword")),
            RecipeFileEntry::Entry(recipe_named("shield")),
        ];
        let report = merge_recipes(&current, incoming);
        assert_eq!(report.entries.len(), 3);
        // 本机那条原地不动，文件里的那条加序号落在旁边。
        assert_eq!(report.entries[0].name, "sword");
        assert_eq!(report.entries[1].name, "sword (2)");
        assert_eq!(report.entries[2].name, "shield");
        let renamed = report
            .rows
            .iter()
            .find(|r| r.state == RecipeImportState::Renamed)
            .expect("撞名该被改名");
        assert_eq!(renamed.name, "sword");
        assert_eq!(renamed.final_name, "sword (2)");
        assert!(
            report.rows.iter().all(|r| r.note.is_empty()),
            "进来的都不该带原因"
        );
    }

    #[test]
    fn renamed_slots_keep_the_name_length_limit() {
        let long = "x".repeat(MAX_RECIPE_NAME_CHARS);
        let current = vec![recipe_named(&long)];
        let incoming = vec![
            RecipeFileEntry::Entry(recipe_named(&long)),
            RecipeFileEntry::Entry(recipe_named(&long)),
        ];
        let report = merge_recipes(&current, incoming);
        let names: Vec<&str> = report.entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names
            .iter()
            .all(|n| n.chars().count() <= MAX_RECIPE_NAME_CHARS));
        assert!(names.contains(&format!("{} (2)", "x".repeat(MAX_RECIPE_NAME_CHARS - 4)).as_str()));
        assert!(names.contains(&format!("{} (3)", "x".repeat(MAX_RECIPE_NAME_CHARS - 4)).as_str()));
    }

    #[test]
    fn a_full_book_refuses_new_recipes_one_by_one() {
        let current: Vec<BatchRecipeEntry> = (0..MAX_RECIPES)
            .map(|i| recipe_named(&format!("r{i}")))
            .collect();
        let incoming = vec![RecipeFileEntry::Entry(recipe_named("newcomer"))];
        let report = merge_recipes(&current, incoming);
        assert_eq!(report.entries.len(), MAX_RECIPES);
        assert_eq!(report.rows.len(), 1);
        assert_eq!(report.rows[0].state, RecipeImportState::Skipped);
        assert!(
            report.rows[0].note.contains("full"),
            "{}",
            report.rows[0].note
        );
    }

    #[test]
    fn an_unusable_name_is_skipped_with_its_label() {
        let incoming = vec![RecipeFileEntry::Entry(recipe_named("bad/name"))];
        let report = merge_recipes(&[], incoming);
        assert!(report.entries.is_empty());
        assert_eq!(report.rows[0].state, RecipeImportState::Skipped);
        assert_eq!(report.rows[0].name, "bad/name");
    }

    #[test]
    fn broken_entries_without_a_name_fall_back_to_a_position_label() {
        let text = r#"{"format":"aipixel-recipe-book","version":1,"entries":[{"recipe":{}}]}"#;
        let parsed = parse_recipe_book(text).unwrap();
        let report = merge_recipes(&[], parsed);
        assert_eq!(report.rows[0].name, "#1");
        assert_eq!(report.rows[0].state, RecipeImportState::Skipped);
    }

    #[test]
    fn the_recipe_extension_gets_added_when_the_user_skips_it() {
        assert_eq!(with_recipe_extension("/tmp/book.aipr"), "/tmp/book.aipr");
        assert_eq!(with_recipe_extension("/tmp/book"), "/tmp/book.aipr");
        assert_eq!(
            with_recipe_extension("/tmp/book.json"),
            "/tmp/book.json.aipr"
        );
        assert_eq!(with_recipe_extension("/tmp/book.AIPR"), "/tmp/book.AIPR");
    }
}
