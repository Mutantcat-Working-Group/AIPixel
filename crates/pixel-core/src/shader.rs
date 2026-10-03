// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 沙箱 Lua 绘制：LLM 的生图主通道。
//! 「不让模型手写矩阵」——脚本体量与画布尺寸无关，循环/噪声/插值由 runtime 执行。
//!
//! 预算（经验值）：20M 指令 / 5 秒 / 64KB 脚本。

use super::aa;
use super::document::{Cel, Document, Rgba};
use mlua::{Function, Lua, Table, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

pub const MAX_SCRIPT_BYTES: usize = 64 * 1024;
pub const DEFAULT_MAX_INSTRUCTIONS: u64 = 20_000_000;
pub const DEFAULT_MAX_SECONDS: u64 = 5;
/// Lua 侧内存上限。字符串拼接是在一条 C 调用里完成的，指令预算和时间预算
/// 都拦不住它，只能靠分配器本身收口。
pub const LUA_MEMORY_LIMIT: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ShaderBudget {
    pub max_instructions: u64,
    pub max_seconds: u64,
}

impl Default for ShaderBudget {
    fn default() -> Self {
        Self {
            max_instructions: DEFAULT_MAX_INSTRUCTIONS,
            max_seconds: DEFAULT_MAX_SECONDS,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ShaderError {
    #[error("script exceeds {MAX_SCRIPT_BYTES} bytes ({0})")]
    ScriptTooLarge(usize),
    #[error("lua error: {message}")]
    Lua { message: String, line: Option<i32> },
    #[error("budget exhausted: {0}")]
    Budget(String),
    #[error("document error: {0}")]
    Document(String),
}

#[derive(Debug, Clone)]
pub struct ShaderOutcome {
    pub frames_rendered: usize,
    pub opaque_pixels: u64,
    /// 脚本运行过程中新并入调色板的颜色数
    pub palette_additions: usize,
}

impl From<mlua::Error> for ShaderError {
    fn from(e: mlua::Error) -> Self {
        let message = e.to_string();
        let line = lua_error_line(&message);
        ShaderError::Lua { message, line }
    }
}

/// 从 Lua 原文里把行号抠出来：`[string "shader"]:12: attempt to ...`。
/// 抠不到就返回 None——没行号的报错照样能读，只是模型要多找一圈。
/// 只认第一个 `]:`：那段位置前缀永远排在消息最前面，往后正文里的方括号
/// 一律不管，免得把报错文本本身当成位置。
fn lua_error_line(message: &str) -> Option<i32> {
    let at = message.find("]:")? + 2;
    let digits: String = message[at..]
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// 沙箱里注册的 helper 与注入全局。模型最容易栽的坑就是拿这些名字当局部
/// 变量：`local line = ...` 一写，后面每个 `line(...)` 都在调字符串。
const LUA_BUILTINS: &[&str] = &[
    "pal",
    "hex",
    "mix",
    "hsv",
    "alpha",
    "rand",
    "noise",
    "canvas",
    "pset",
    "pget",
    "line",
    "rect",
    "rectfill",
    "ellipse",
    "ellipfill",
    "ellipsefill",
    "circle",
    "circfill",
    "circlefill",
    "flood",
    "replace",
    "outline",
    "clear",
    "stamp",
    "aaline",
    "aaseg",
    "aacurve",
    "aaquad",
    "aacubic",
    "aabez",
    "aapoly",
    "aapath",
    "aapolyfill",
    "aafill",
    "aacircle",
    "aacirc",
    "aaellipse",
    "aarect",
    "blend",
    "aablend",
    "dither",
    "width",
    "height",
    "canvas_w",
    "canvas_h",
    "layer",
    "frame_index",
    "frame_count",
    "time",
    "phase",
];

/// 从 `attempt to call a string value (local 'line')` 这类报错里取出被点名的名字，
/// 撞上内置名就补一句人话。没点名或名字不在册就安静返回 None。
fn lua_hint(msg: &str) -> Option<String> {
    let name = ["local", "global"].iter().find_map(|scope| {
        let marker = format!("({scope} '");
        let start = msg.find(&marker)? + marker.len();
        let end = msg[start..].find('\'')? + start;
        Some(msg[start..end].to_string())
    })?;
    if !LUA_BUILTINS.contains(&name.as_str()) {
        return None;
    }
    Some(format!(
        "hint: `{name}` is a builtin here, and your own local shadows it; rename the local (e.g. ln, {name}_i) instead of reusing the builtin name"
    ))
}

/// 运行 shader。`animate=true` 时逐帧执行，注入 time/phase/frame_index/frame_count，
/// 每帧执行前清空目标 cel；否则在目标 cel 上增量绘制（保留既有像素）。
pub fn run_shader(
    doc: &mut Document,
    layer: &str,
    script: &str,
    animate: bool,
    budget: &ShaderBudget,
) -> Result<ShaderOutcome, ShaderError> {
    if script.len() > MAX_SCRIPT_BYTES {
        return Err(ShaderError::ScriptTooLarge(script.len()));
    }
    let first_frame = doc
        .frames
        .first()
        .map(|f| f.id.clone())
        .ok_or_else(|| ShaderError::Document("document has no frames".into()))?;
    if doc.cel(layer, &first_frame).is_none() {
        return Err(ShaderError::Document(format!("unknown layer {layer}")));
    }

    // 脚本可能碰到的 cel 先备一份：animate 会把整个图层全清一遍，所以每帧都备；
    // 单帧只碰那一帧，只备它就够，不必 clone 整份文档。
    // 为什么单帧也必须备：脚本跑到一半报错时，前半截画的东西已经落在 cel 上，
    // 而错误路径同样会 bump。前端比 revision 会当真，把半截画坏的玩意儿画上屏；
    // 模型从结果里读到「画了多少个像素」，也以为自己已经画上了，接着在错的
    // 底子上往下补。留半截在画布上，比一个像素都不留更难收拾。
    // 只备沙盒可能碰到的部分：目标图层的 cel 和调色板（大文档整份 clone 太贵）。
    let touched: Vec<String> = if animate {
        doc.frames.iter().map(|f| f.id.clone()).collect()
    } else {
        vec![first_frame.clone()]
    };
    let mut cel_backup: Vec<(String, Cel)> = Vec::new();
    for frame_id in &touched {
        if let Some(cel) = doc.cel(layer, frame_id) {
            cel_backup.push((frame_id.clone(), cel.clone()));
        }
    }
    let palette_backup = doc.palette.clone();
    let palette_before = doc.palette.len();
    let frame_count = doc.frames.len().max(1);
    let frames_to_render = if animate { frame_count } else { 1 };

    let sandbox = Sandbox::new(doc, layer.to_string(), budget.clone())?;

    for fi in 0..frames_to_render {
        let frame_id = if animate {
            doc.frames[fi].id.clone()
        } else {
            first_frame.clone()
        };
        if animate {
            let cel = doc
                .cel_mut(layer, &frame_id)
                .ok_or_else(|| ShaderError::Document(format!("unknown frame {frame_id}")))?;
            cel.indices.iter_mut().for_each(|i| *i = 0);
        }
        sandbox.set_target(layer, &frame_id);
        sandbox.set_frame_globals(fi, frames_to_render, doc.frames[fi].duration_ms);
        if let Err(e) = sandbox.exec(script) {
            drop(sandbox);
            // 还原被清空、或只画了一半的那些帧，并把沙盒新塞进调色板的颜色撤掉。
            for (frame_id, cel) in &cel_backup {
                if let Some(slot) = doc.cel_mut(layer, frame_id) {
                    slot.indices = cel.indices.clone();
                }
            }
            doc.palette = palette_backup;
            // 撤干净了就是撤干净了，连 revision 都不该动：它还停在原来的地方，
            // 前端比 revision 才知道「画布没变」。留个 bump 在这儿，等于告诉
            // 全世界「画布变了」，前端白刷一次，重放记忆也跟着失效。
            return Err(e);
        }
    }
    drop(sandbox);

    doc.bump();
    Ok(ShaderOutcome {
        frames_rendered: frames_to_render,
        opaque_pixels: opaque_counts(doc),
        palette_additions: doc.palette.len().saturating_sub(palette_before),
    })
}

fn opaque_counts(doc: &Document) -> u64 {
    doc.cels
        .values()
        .flat_map(|frames| frames.values())
        .map(|c| c.indices.iter().filter(|i| **i != 0).count())
        .sum::<usize>() as u64
}

struct Sandbox {
    lua: Lua,
    /// 运行期文档指针：回调同步执行、无重入，安全。
    doc_ptr: *mut Document,
    layer: Rc<RefCell<String>>,
    frame: Rc<RefCell<String>>,
    rng: Rc<Cell<u64>>,
    budget: ShaderBudget,
    started: Instant,
}

// 单线程同步使用；原始指针不进跨线程边界
unsafe impl Send for Sandbox {}

impl Sandbox {
    fn new(doc: &mut Document, layer: String, budget: ShaderBudget) -> Result<Self, ShaderError> {
        let lua = Lua::new();
        // 超限时让分配动作本身失败：`('x'):rep(2e8)` 这类写法会在一条指令里
        // 吃掉几百兆，指令钩子和墙钟都来不及反应。
        let _ = lua.set_memory_limit(LUA_MEMORY_LIMIT);
        // 显式移除文件/进程面，双保险
        for name in [
            "io", "os", "package", "debug", "require", "dofile", "loadfile", "load",
        ] {
            let _ = lua.globals().set(name, Value::Nil);
        }

        let sandbox = Sandbox {
            lua,
            doc_ptr: doc as *mut Document,
            layer: Rc::new(RefCell::new(layer)),
            frame: Rc::new(RefCell::new(String::new())),
            rng: Rc::new(Cell::new(0x2545F4914F6CDD1D)),
            budget,
            started: Instant::now(),
        };
        sandbox.install()?;
        Ok(sandbox)
    }

    fn doc(&self) -> &Document {
        // Safety: 闭包仅在 exec 期间同步调用，文档生命周期覆盖整个 run_shader
        // 只读视图：doc_ptr 由 run_shader 独占到结束，写路径在各闭包里自己重新解出
        unsafe { &*(self.doc_ptr as *const Document) }
    }

    fn set_target(&self, layer: &str, frame: &str) {
        *self.layer.borrow_mut() = layer.to_string();
        *self.frame.borrow_mut() = frame.to_string();
    }

    fn set_frame_globals(&self, index: usize, count: usize, duration_ms: u32) {
        let g = self.lua.globals();
        let _ = g.set("frame_index", index as i64);
        let _ = g.set("frame_count", count as i64);
        let _ = g.set("time", index as f64 * duration_ms as f64 / 1000.0);
        let _ = g.set(
            "phase",
            if count > 1 {
                index as f64 / (count - 1) as f64
            } else {
                0.0
            },
        );
        let _ = g.set("layer", self.layer.borrow().clone());
    }

    fn exec(&self, script: &str) -> Result<(), ShaderError> {
        let result = self.lua.load(script).set_name("shader").eval::<Value>();
        match result {
            Ok(_) => {
                self.check_budget()?;
                Ok(())
            }
            Err(e) => {
                let msg = e.to_string();
                if self.started.elapsed() > Duration::from_secs(self.budget.max_seconds)
                    || msg.contains("budget exhausted")
                {
                    return Err(ShaderError::Budget("execution budget exhausted".into()));
                }
                // 报错点名的往往正是被自家局部变量遮蔽的内置函数，
                // 这句话替模型省掉一整轮「报错-瞎改-再报错」。
                let line = lua_error_line(&msg);
                let mut msg = msg;
                if let Some(hint) = lua_hint(&msg) {
                    msg.push_str("; ");
                    msg.push_str(&hint);
                }
                Err(ShaderError::Lua { message: msg, line })
            }
        }
    }

    fn check_budget(&self) -> Result<(), ShaderError> {
        if self.started.elapsed() > Duration::from_secs(self.budget.max_seconds) {
            return Err(ShaderError::Budget("time limit exceeded".into()));
        }
        Ok(())
    }

    // ---------- 安装全局 ----------

    fn install(&self) -> Result<(), ShaderError> {
        self.install_palette_functions()
            .map_err(ShaderError::from)?;
        self.install_canvas_functions().map_err(ShaderError::from)?;
        self.install_dimension_globals()
            .map_err(ShaderError::from)?;
        self.mirror_canvas_functions().map_err(ShaderError::from)?;
        self.install_instruction_guard();
        Ok(())
    }

    /// 画布尺寸：run_shader 期间不会变，装沙箱时定一次。
    /// 提示词承诺的是 `width` / `height` 与 `canvas.width` / `canvas.height`，
    /// 四个名字都得在；canvas_w / canvas_h 只是给换了写法的模型留的别名。
    /// 曾经只设了 canvas_w / canvas_h，模型照着提示词写 width 拿到 nil，
    /// 一个条件判断就能把整张图画成空的。
    fn install_dimension_globals(&self) -> mlua::Result<()> {
        let globals = self.lua.globals();
        let (w, h) = {
            let doc = self.doc();
            (doc.width, doc.height)
        };
        globals.set("width", w)?;
        globals.set("height", h)?;
        globals.set("canvas_w", w)?;
        globals.set("canvas_h", h)?;

        // 尺寸适配助手：模型对着 64x64 调好的常量画 24x24 的画布，一小半坐标
        // 直接越界、剩下的挤在左上角，这是「脚本和画布不适配」最常见的样子。
        // 把「按比例」写成 canvas.scale(0.5) 比写四个魔数短，模型才会真的用。
        let canvas: Table = globals.get("canvas")?;
        canvas.set("width", w)?;
        canvas.set("height", h)?;
        canvas.set("cx", w as f64 / 2.0)?;
        canvas.set("cy", h as f64 / 2.0)?;
        canvas.set("min", w.min(h))?;
        canvas.set("max", w.max(h))?;
        // scale(k)：把「按 64px 设计」的常量折算到当前画布的长边比例。
        let unit = w.max(h) as f64 / 64.0;
        let scale_fn = self.lua.create_function(move |_lua, k: f64| Ok(k * unit))?;
        canvas.set("scale", scale_fn)?;
        // grid(cols, rows)：把画布切成 cols x rows 的格子，返回格子的整数宽高。
        // 至少留一格：除零会产出 NaN 坐标，那比直接报错难查得多。
        let (fw, fh) = (w as f64, h as f64);
        let grid_fn = self
            .lua
            .create_function(move |_lua, (cols, rows): (f64, f64)| {
                let cols = if cols >= 1.0 { cols } else { 1.0 };
                let rows = if rows >= 1.0 { rows } else { 1.0 };
                Ok(((fw / cols).floor().max(1.0), (fh / rows).floor().max(1.0)))
            })?;
        canvas.set("grid", grid_fn)?;
        Ok(())
    }

    /// 把绘图函数再挂一份到 `canvas` 表上。
    /// 模型很爱写 `canvas.pset(...)`，两种写法都得能跑：
    /// 别让一次命名习惯上的出入，白白吃掉一整个输出预算。
    fn mirror_canvas_functions(&self) -> mlua::Result<()> {
        let globals = self.lua.globals();
        let canvas: Table = globals.get("canvas")?;
        for name in [
            "width",
            "height",
            "pset",
            "pget",
            "line",
            "rect",
            "rectfill",
            "ellipse",
            "ellipfill",
            "ellipsefill",
            "circle",
            "circfill",
            "circlefill",
            "flood",
            "replace",
            "outline",
            "clear",
            "stamp",
            "aaline",
            "aaseg",
            "aacurve",
            "aaquad",
            "aacubic",
            "aabez",
            "aapoly",
            "aapath",
            "aapolyfill",
            "aafill",
            "aacircle",
            "aacirc",
            "aaellipse",
            "aarect",
            "blend",
            "aablend",
            "dither",
        ] {
            let value: Value = globals.get(name)?;
            if !value.is_nil() {
                canvas.set(name, value)?;
            }
        }
        Ok(())
    }

    fn install_palette_functions(&self) -> mlua::Result<()> {
        let doc_ptr = self.doc_ptr as usize;
        let globals = self.lua.globals();

        // pal(i) -> "#rrggbb"
        let pal = self.lua.create_function(move |lua, idx: Value| {
            let doc = unsafe { &*(doc_ptr as *const Document) };
            let i = index_from(&idx, doc.palette.len())?;
            match doc.color_of(i) {
                // 索引 0 在 Lua 侧表达为 transparent，避免被当成黑色
                Some(c) if c == Rgba::TRANSPARENT => {
                    Ok(Value::String(lua.create_string(b"transparent")?))
                }
                Some(c) => Ok(Value::String(lua.create_string(c.to_hex().as_bytes())?)),
                None => Err(mlua::Error::RuntimeError(format!(
                    "palette index {i} out of range (0..{}, 0 is transparent) - \
                     add colors first with pixel_apply_operations add_palette_colors, \
                     or use a \"#RRGGBB\" string, which every drawing call also takes",
                    doc.palette.len()
                ))),
            }
        })?;
        globals.set("pal", pal)?;

        // hex(value) -> "#rrggbb"（#hex 先 intern 进当前层；transparent -> 0）
        // 层 id 一起带进去：锁着的层在 hex() 里就要归队，
        // 不然模型以为写进去的颜色生效了，实际落在另一个色上。
        let layer_cell = self.layer.clone();
        let hex_fn = self.lua.create_function(move |lua, v: Value| {
            let doc = unsafe { &mut *(doc_ptr as *mut Document) };
            let layer = layer_cell.borrow().clone();
            let idx = resolve_color_for_layer(doc, &layer, v)?;
            let color = doc.color_of(idx).unwrap_or(Rgba::TRANSPARENT);
            // 交出去的是 hex 串而不是索引：pal / mix / hsv / alpha / pget 全说 hex，
            // 只有 hex() 说索引的话 `mix(hex('#e74c3c'), '#000', .35)` 必然炸，
            // 白烧模型一个来回。颜色在 Lua 侧就该是一种能到处传的值。
            Ok(Value::String(lua.create_string(color.to_hex().as_bytes())?))
        })?;
        globals.set("hex", hex_fn)?;

        // mix(a, b, t) -> "#hex"
        let mix_fn = self
            .lua
            .create_function(move |lua, (a, b, t): (Value, Value, f64)| {
                let doc = unsafe { &*(doc_ptr as *const Document) };
                let ca = value_to_rgba(&a, doc)?;
                let cb = value_to_rgba(&b, doc)?;
                let t = t.clamp(0.0, 1.0);
                let out = Rgba {
                    r: lerp(ca.r, cb.r, t),
                    g: lerp(ca.g, cb.g, t),
                    b: lerp(ca.b, cb.b, t),
                    a: lerp(ca.a, cb.a, t),
                };
                Ok(Value::String(lua.create_string(out.to_hex().as_bytes())?))
            })?;
        globals.set("mix", mix_fn)?;

        // hsv(h, s, v[, a]) -> "#hex"（h 取 0..360，a 接受 0..1 或 0..255）
        // 提示词里那个 `[, a]` 曾经只是装饰：mlua 的元组解构对多出来的实参
        // 只字不提地丢掉，模型写 hsv(30, .8, .9, .5) 拿回一个完全不透明的
        // 颜色，不报错也不提示，半透明一整层就这么凭空消失。
        let hsv_fn = self.lua.create_function(|lua, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let (h, s, v) = (
                num_arg(it.next())?,
                num_arg(it.next())?,
                num_arg(it.next())?,
            );
            // 给错类型要喊出来，别像元组解构那样一声不响地丢掉。
            let a = match it.next().unwrap_or(Value::Nil) {
                Value::Nil => None,
                other => Some(num_arg(Some(other))?),
            };
            let mut color = hsv_to_rgb(h / 360.0, s, v);
            if let Some(a) = a {
                color.a = if a <= 1.0 {
                    (a * 255.0).round().clamp(0.0, 255.0) as u8
                } else {
                    a.round().clamp(0.0, 255.0) as u8
                };
            }
            // 带 alpha 时必须走 rgba 表示，否则 to_hex 会把那 8 个位丢掉。
            let text = if color.a == 255 {
                color.to_hex()
            } else {
                color.to_rgba_hex()
            };
            Ok(Value::String(lua.create_string(text.as_bytes())?))
        })?;
        globals.set("hsv", hsv_fn)?;

        // alpha(color, a) -> "#rrggbbaa"（a 接受 0..1 或 0..255）
        let alpha_fn = self
            .lua
            .create_function(move |lua, (color, a): (Value, f64)| {
                let doc = unsafe { &*(doc_ptr as *const Document) };
                let base = value_to_rgba(&color, doc)?;
                let alpha = if a <= 1.0 {
                    (a * 255.0).round() as u8
                } else {
                    a.round() as u8
                };
                let out = Rgba { a: alpha, ..base };
                Ok(Value::String(
                    lua.create_string(out.to_rgba_hex().as_bytes())?,
                ))
            })?;
        globals.set("alpha", alpha_fn)?;

        // rand() -> 0..1 确定性；rand(a, b) -> [a, b] 整数
        let rng = self.rng.clone();
        let rand_fn = self
            .lua
            .create_function(move |_lua, args: mlua::MultiValue| {
                let mut it = args.into_iter();
                let lo = it.next().and_then(|v| v.as_integer());
                let hi = it.next().and_then(|v| v.as_integer());
                let mut x = rng.get();
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                rng.set(x);
                let u = ((x >> 11) as f64) / ((1u64 << 53) as f64);
                Ok(match (lo, hi) {
                    (Some(a), Some(b)) if b >= a => {
                        Value::Integer(a + (u * (b - a + 1) as f64) as i64)
                    }
                    _ => Value::Number(u),
                })
            })?;
        globals.set("rand", rand_fn)?;

        // noise(x, y) -> 0..1 确定性
        let noise_fn =
            self.lua
                .create_function(|_lua, (x, y, scale): (f64, f64, Option<f64>)| {
                    // scale 是密度：0.3 表示噪声格被拉开三倍，画竖条纹就靠它。
                    let s = scale.unwrap_or(1.0).max(0.0001);
                    Ok(value_noise(x * s, y * s))
                })?;
        globals.set("noise", noise_fn)?;

        let canvas = self.lua.create_table()?;
        let (w, h) = (self.doc().width, self.doc().height);
        canvas.set("width", w)?;
        canvas.set("height", h)?;
        globals.set("canvas", canvas)?;
        Ok(())
    }

    fn install_canvas_functions(&self) -> mlua::Result<()> {
        let globals = self.lua.globals();

        let pset = self.canvas_fn(|doc, layer, frame, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let x = num_arg(it.next())?;
            let y = num_arg(it.next())?;
            let color = it.next().unwrap_or(Value::Nil);
            let (x, y) = (coord(x, doc.width)?, coord(y, doc.height)?);
            let idx = resolve_color(doc, color)?;
            // 先把宽高读出来，再把 cel 的可变借用拿走，两个借用不重叠。
            let (w, _h) = (doc.width, doc.height);
            let cel = cel_mut(doc, layer, frame)?;
            cel.set(w, x, y, idx);
            Ok(())
        })?;
        globals.set("pset", pset)?;

        let pget = self.canvas_fn(|doc, layer, frame, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let x = num_arg(it.next())?;
            let y = num_arg(it.next())?;
            let cel = cel(doc, layer, frame)?;
            let color = cel
                .get(doc.width, coord(x, doc.width)?, coord(y, doc.height)?)
                .and_then(|i| doc.color_of(i))
                .unwrap_or(Rgba::TRANSPARENT);
            let text = if color == Rgba::TRANSPARENT {
                "transparent".to_string()
            } else {
                color.to_hex()
            };
            Ok(text)
        })?;
        globals.set("pget", pget)?;

        let line = self.canvas_fn(|doc, layer, frame, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let (x0, y0, x1, y1) = (
                num_arg(it.next())?,
                num_arg(it.next())?,
                num_arg(it.next())?,
                num_arg(it.next())?,
            );
            let color = it.next().unwrap_or(Value::Nil);
            let idx = resolve_color(doc, color)?;
            let (w, h) = (doc.width, doc.height);
            let cel = cel_mut(doc, layer, frame)?;
            // line 的两端可以落在画布外：夹到包围盒即可，别按原值跑 Bresenham，
            // 否则一个 u32::MAX 的端点能让这条线在超时预算内跑不完。
            let clamp = |v: f64, max: u32| -> u32 {
                if !v.is_finite() || v < 0.0 {
                    0
                } else {
                    (v.round().min(max as f64 - 1.0).max(0.0)) as u32
                }
            };
            super::ops::draw_line(
                cel,
                w,
                h,
                clamp(x0, w),
                clamp(y0, h),
                clamp(x1, w),
                clamp(y1, h),
                idx,
            );
            Ok(())
        })?;
        globals.set("line", line)?;

        let rect = self.canvas_fn(|doc, layer, frame, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let nums: Vec<f64> = (0..4)
                .map(|_| num_arg(it.next()))
                .collect::<mlua::Result<_>>()?;
            let color = it.next().unwrap_or(Value::Nil);
            let filled = it.next().and_then(|v| v.as_boolean()).unwrap_or(false);
            let idx = resolve_color(doc, color)?;
            let (w, h) = (doc.width, doc.height);
            let cel = cel_mut(doc, layer, frame)?;
            super::ops::draw_rect(
                cel,
                w,
                h,
                nums[0] as u32,
                nums[1] as u32,
                nums[2] as u32,
                nums[3] as u32,
                idx,
                filled,
            );
            Ok(())
        })?;
        globals.set("rect", rect.clone())?;
        let rectfill = self.canvas_fn(|doc, layer, frame, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let nums: Vec<f64> = (0..4)
                .map(|_| num_arg(it.next()))
                .collect::<mlua::Result<_>>()?;
            let color = it.next().unwrap_or(Value::Nil);
            let idx = resolve_color(doc, color)?;
            let (w, h) = (doc.width, doc.height);
            let cel = cel_mut(doc, layer, frame)?;
            super::ops::draw_rect(
                cel,
                w,
                h,
                nums[0] as u32,
                nums[1] as u32,
                nums[2] as u32,
                nums[3] as u32,
                idx,
                true,
            );
            let _ = layer;
            let _ = frame;
            Ok(())
        })?;
        globals.set("rectfill", rectfill)?;

        let ellipse = self.canvas_fn(|doc, layer, frame, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let nums: Vec<f64> = (0..4)
                .map(|_| num_arg(it.next()))
                .collect::<mlua::Result<_>>()?;
            let color = it.next().unwrap_or(Value::Nil);
            let filled = it.next().and_then(|v| v.as_boolean()).unwrap_or(false);
            let idx = resolve_color(doc, color)?;
            let (w, h) = (doc.width, doc.height);
            let cel = cel_mut(doc, layer, frame)?;
            super::ops::draw_ellipse(
                cel,
                w,
                h,
                nums[0] as u32,
                nums[1] as u32,
                nums[2] as u32,
                nums[3] as u32,
                idx,
                filled,
            );
            Ok(())
        })?;
        globals.set("ellipse", ellipse.clone())?;
        let ellipfill = self.canvas_fn(|doc, layer, frame, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let nums: Vec<f64> = (0..4)
                .map(|_| num_arg(it.next()))
                .collect::<mlua::Result<_>>()?;
            let color = it.next().unwrap_or(Value::Nil);
            let idx = resolve_color(doc, color)?;
            let (w, h) = (doc.width, doc.height);
            let cel = cel_mut(doc, layer, frame)?;
            super::ops::draw_ellipse(
                cel,
                w,
                h,
                nums[0] as u32,
                nums[1] as u32,
                nums[2] as u32,
                nums[3] as u32,
                idx,
                true,
            );
            Ok(())
        })?;
        // 提示词里写的是 ellipfill，这里却只注册了 ellipsefill：模型照着提示词写
        // canvas.ellipfill(...) 拿到的是 nil，一整个输出预算就这么烧掉的。两个名字
        // 都挂上，模型写哪个都算数。
        globals.set("ellipfill", ellipfill.clone())?;
        globals.set("ellipsefill", ellipfill)?;

        let circle = self.canvas_fn(|doc, layer, frame, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let cx = num_arg(it.next())?;
            let cy = num_arg(it.next())?;
            let r = num_arg(it.next())?;
            let color = it.next().unwrap_or(Value::Nil);
            let filled = it.next().and_then(|v| v.as_boolean()).unwrap_or(false);
            let idx = resolve_color(doc, color)?;
            let (w, h) = (doc.width, doc.height);
            let cel = cel_mut(doc, layer, frame)?;
            let box_ = circle_box(cx, cy, r, w, h);
            super::ops::draw_ellipse(cel, w, h, box_.0, box_.1, box_.2, box_.3, idx, filled);
            Ok(())
        })?;
        globals.set("circle", circle.clone())?;
        let circfill = self.canvas_fn(|doc, layer, frame, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let cx = num_arg(it.next())?;
            let cy = num_arg(it.next())?;
            let r = num_arg(it.next())?;
            let color = it.next().unwrap_or(Value::Nil);
            let idx = resolve_color(doc, color)?;
            let (w, h) = (doc.width, doc.height);
            let cel = cel_mut(doc, layer, frame)?;
            let box_ = circle_box(cx, cy, r, w, h);
            super::ops::draw_ellipse(cel, w, h, box_.0, box_.1, box_.2, box_.3, idx, true);
            Ok(())
        })?;
        // circfill 的别名：文档给的是 circfill，但模型更熟 circlefill，
        // 两种写法都收，别为一次命名出入白吃一个来回。
        globals.set("circfill", circfill.clone())?;
        globals.set("circlefill", circfill)?;

        // ---- 平滑绘制（抗锯齿）----
        //
        // 上面那组基本图形把几何四舍五入到整数格，画出来是硬的：一条 46° 的
        // 斜线碎成台阶，一个 r=5 的圆在 64px 画布上只剩六个方向可去。用户要
        // 的是「画细节的地方有平滑曲线可用」，所以这一组按格心到几何的连续
        // 距离算覆盖率，一格一格掺色。坐标写法和硬边那组完全一样。
        //
        // 颜色统一走 resolve_color_for_layer：层上着配色锁时，掺出来的中间色
        // 也得归到范围里去，不然一条柔边就能把文档调色板撑满。
        let aaline = self.canvas_fn(|doc, layer, frame, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let nums: Vec<f64> = (0..4)
                .map(|_| num_arg(it.next()))
                .collect::<mlua::Result<_>>()?;
            let color = it.next().unwrap_or(Value::Nil);
            let idx = resolve_color_for_layer(doc, layer, color)?;
            let mut buf = aa::Coverage::for_canvas(doc.width, doc.height);
            aa::stroke_segment(&mut buf, nums[0], nums[1], nums[2], nums[3]);
            aa::apply(doc, layer, frame, buf, idx).map_err(doc_err)
        })?;
        globals.set("aaline", aaline.clone())?;
        globals.set("aaseg", aaline)?;

        let aacurve = self.canvas_fn(|doc, layer, frame, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let nums: Vec<f64> = (0..4)
                .map(|_| num_arg(it.next()))
                .collect::<mlua::Result<_>>()?;
            let (cx, cy) = (num_arg(it.next())?, num_arg(it.next())?);
            let color = it.next().unwrap_or(Value::Nil);
            let idx = resolve_color_for_layer(doc, layer, color)?;
            let mut buf = aa::Coverage::for_canvas(doc.width, doc.height);
            aa::quad_curve(&mut buf, nums[0], nums[1], nums[2], nums[3], cx, cy);
            aa::apply(doc, layer, frame, buf, idx).map_err(doc_err)
        })?;
        globals.set("aacurve", aacurve.clone())?;
        globals.set("aaquad", aacurve)?;

        let aacubic = self.canvas_fn(|doc, layer, frame, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let nums: Vec<f64> = (0..4)
                .map(|_| num_arg(it.next()))
                .collect::<mlua::Result<_>>()?;
            let (c0x, c0y) = (num_arg(it.next())?, num_arg(it.next())?);
            let (c1x, c1y) = (num_arg(it.next())?, num_arg(it.next())?);
            let color = it.next().unwrap_or(Value::Nil);
            let idx = resolve_color_for_layer(doc, layer, color)?;
            let mut buf = aa::Coverage::for_canvas(doc.width, doc.height);
            aa::cubic_curve(
                &mut buf,
                nums[0],
                nums[1],
                nums[2],
                nums[3],
                (c0x, c0y),
                (c1x, c1y),
            );
            aa::apply(doc, layer, frame, buf, idx).map_err(doc_err)
        })?;
        globals.set("aacubic", aacubic.clone())?;
        globals.set("aabez", aacubic)?;

        let aapoly = self.canvas_fn(|doc, layer, frame, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let first = it.next().unwrap_or(Value::Nil);
            let points = points_from_lua(&first)?;
            let color = it.next().unwrap_or(Value::Nil);
            let closed = it.next().and_then(|v| v.as_boolean()).unwrap_or(false);
            if points.len() < 2 {
                return Ok(0usize);
            }
            let idx = resolve_color_for_layer(doc, layer, color)?;
            let mut buf = aa::Coverage::for_canvas(doc.width, doc.height);
            aa::stroke_polyline(&mut buf, &points, closed);
            aa::apply(doc, layer, frame, buf, idx).map_err(doc_err)
        })?;
        globals.set("aapoly", aapoly.clone())?;
        globals.set("aapath", aapoly)?;

        let aapolyfill = self.canvas_fn(|doc, layer, frame, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let first = it.next().unwrap_or(Value::Nil);
            let points = points_from_lua(&first)?;
            let color = it.next().unwrap_or(Value::Nil);
            if points.len() < 3 {
                return Ok(0usize);
            }
            let idx = resolve_color_for_layer(doc, layer, color)?;
            let mut buf = aa::Coverage::for_canvas(doc.width, doc.height);
            aa::fill_polygon(&mut buf, &points);
            aa::apply(doc, layer, frame, buf, idx).map_err(doc_err)
        })?;
        globals.set("aapolyfill", aapolyfill.clone())?;
        globals.set("aafill", aapolyfill)?;

        let aacircle = self.canvas_fn(|doc, layer, frame, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let (cx, cy, r) = (
                num_arg(it.next())?,
                num_arg(it.next())?,
                num_arg(it.next())?,
            );
            let color = it.next().unwrap_or(Value::Nil);
            let filled = it.next().and_then(|v| v.as_boolean()).unwrap_or(false);
            let idx = resolve_color_for_layer(doc, layer, color)?;
            let mut buf = aa::Coverage::for_canvas(doc.width, doc.height);
            if filled {
                aa::ellipse_fill(&mut buf, cx, cy, r, r);
            } else {
                aa::ellipse_ring(&mut buf, cx, cy, r, r);
            }
            aa::apply(doc, layer, frame, buf, idx).map_err(doc_err)
        })?;
        globals.set("aacircle", aacircle.clone())?;
        globals.set("aacirc", aacircle)?;

        let aaellipse = self.canvas_fn(|doc, layer, frame, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let nums: Vec<f64> = (0..4)
                .map(|_| num_arg(it.next()))
                .collect::<mlua::Result<_>>()?;
            let color = it.next().unwrap_or(Value::Nil);
            let filled = it.next().and_then(|v| v.as_boolean()).unwrap_or(false);
            let idx = resolve_color_for_layer(doc, layer, color)?;
            // 跟 ellipse 一样按「两个角都算」的盒子收，模型换写法不用改坐标。
            let cx = (nums[0] + nums[2]) / 2.0;
            let cy = (nums[1] + nums[3]) / 2.0;
            let rx = ((nums[2] - nums[0]) / 2.0).abs();
            let ry = ((nums[3] - nums[1]) / 2.0).abs();
            let mut buf = aa::Coverage::for_canvas(doc.width, doc.height);
            if filled {
                aa::ellipse_fill(&mut buf, cx, cy, rx, ry);
            } else {
                aa::ellipse_ring(&mut buf, cx, cy, rx, ry);
            }
            aa::apply(doc, layer, frame, buf, idx).map_err(doc_err)
        })?;
        globals.set("aaellipse", aaellipse)?;

        let aarect = self.canvas_fn(|doc, layer, frame, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let nums: Vec<f64> = (0..4)
                .map(|_| num_arg(it.next()))
                .collect::<mlua::Result<_>>()?;
            let color = it.next().unwrap_or(Value::Nil);
            let filled = it.next().and_then(|v| v.as_boolean()).unwrap_or(false);
            let idx = resolve_color_for_layer(doc, layer, color)?;
            let mut buf = aa::Coverage::for_canvas(doc.width, doc.height);
            if filled {
                aa::rect_fill(&mut buf, nums[0], nums[1], nums[2], nums[3]);
            } else {
                let closed = [
                    (nums[0], nums[1]),
                    (nums[2], nums[1]),
                    (nums[2], nums[3]),
                    (nums[0], nums[3]),
                ];
                aa::stroke_polyline(&mut buf, &closed, true);
            }
            aa::apply(doc, layer, frame, buf, idx).map_err(doc_err)
        })?;
        globals.set("aarect", aarect)?;

        // blend(x, y, color, a)：只动一格，按 a 掺。给「差半格就够」的场合用，
        // 不必为一次局部调整铺一条覆盖缓冲的流程。
        let blend = self.canvas_fn(|doc, layer, frame, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let x = num_arg(it.next())?;
            let y = num_arg(it.next())?;
            let color = it.next().unwrap_or(Value::Nil);
            let a = it
                .next()
                .and_then(num_arg_opt)
                .unwrap_or(1.0)
                .clamp(0.0, 1.0);
            let idx = resolve_color_for_layer(doc, layer, color)?;
            // 单格函数跟 pset 一样要自己夹：负坐标 as u32 会绕到画布另一头。
            let (x, y) = (coord(x, doc.width)?, coord(y, doc.height)?);
            let mut buf = aa::Coverage::for_canvas(doc.width, doc.height);
            buf.plot(x, y, a as f32);
            aa::apply(doc, layer, frame, buf, idx).map_err(doc_err)
        })?;
        globals.set("blend", blend.clone())?;
        globals.set("aablend", blend)?;

        // dither(x, y, color, a)：单格有序抖动。上着配色锁的层不许扩色板，
        // 但想留一点柔边——落下去的色全在原范围里，出来的是颗粒过渡。
        let dither = self.canvas_fn(|doc, layer, frame, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let x = num_arg(it.next())?;
            let y = num_arg(it.next())?;
            let color = it.next().unwrap_or(Value::Nil);
            let a = it.next().and_then(num_arg_opt).unwrap_or(1.0);
            let idx = resolve_color_for_layer(doc, layer, color)?;
            aa::dither_dot(doc, layer, frame, x, y, idx, a as f32).map_err(doc_err)
        })?;
        globals.set("dither", dither)?;

        let flood = self.canvas_fn(|doc, layer, frame, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let x = num_arg(it.next())?;
            let y = num_arg(it.next())?;
            let color = it.next().unwrap_or(Value::Nil);
            let (x, y) = (coord(x, doc.width)?, coord(y, doc.height)?);
            let target = cel(doc, layer, frame)?.get(doc.width, x, y).unwrap_or(0);
            let fill = resolve_color(doc, color)?;
            let (w, h) = (doc.width, doc.height);
            if target != fill {
                let cel = cel_mut(doc, layer, frame)?;
                super::ops::flood_fill(cel, w, h, x, y, target, fill);
            }
            Ok(())
        })?;
        globals.set("flood", flood)?;

        let replace = self.canvas_fn(|doc, layer, frame, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let from = it.next().unwrap_or(Value::Nil);
            let to = it.next().unwrap_or(Value::Nil);
            let from = resolve_color_readonly(doc, from)?;
            let to = resolve_color(doc, to)?;
            let cel = cel_mut(doc, layer, frame)?;
            for i in cel.indices.iter_mut() {
                if *i == from {
                    *i = to;
                }
            }
            Ok(())
        })?;
        globals.set("replace", replace)?;

        let outline = self.canvas_fn(|doc, layer, frame, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let color = it.next().unwrap_or(Value::Nil);
            let idx = resolve_color(doc, color)?;
            let w = doc.width;
            let h = doc.height;
            let cel = cel_mut(doc, layer, frame)?;
            let snapshot = cel.indices.clone();
            for y in 0..h {
                for x in 0..w {
                    let i = (y * w + x) as usize;
                    if snapshot[i] != 0 {
                        continue;
                    }
                    let neighbor = (x > 0 && snapshot[i - 1] != 0)
                        || (x + 1 < w && snapshot[i + 1] != 0)
                        || (y > 0 && snapshot[i - w as usize] != 0)
                        || (y + 1 < h && snapshot[i + w as usize] != 0);
                    if neighbor {
                        cel.indices[i] = idx;
                    }
                }
            }
            Ok(())
        })?;
        globals.set("outline", outline)?;

        let clear = self.canvas_fn(|doc, layer, frame, args: mlua::MultiValue| {
            // clear() / clear(nil) 擦掉整格；clear('#1b1f2e') 整格填底色。
            // 提示词里写着「erase the cel, or fill it with one color」，
            // 早先这里把参数整个丢掉，模型写 clear(夜色) 只换来一张空白画布。
            let color = args.into_iter().next().unwrap_or(Value::Nil);
            let idx = resolve_color(doc, color)?;
            let cel = cel_mut(doc, layer, frame)?;
            cel.indices.iter_mut().for_each(|i| *i = idx);
            Ok(())
        })?;
        globals.set("clear", clear)?;

        // stamp 里要用 Lua 句柄建字符串，先把 lua clone 出来，
        // 闭包要求 'static，不能借 &self。
        let lua = self.lua.clone();
        let stamp = self.canvas_fn(move |doc, layer, frame, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let rows = it.next().unwrap_or(Value::Nil);
            let legend = it.next().unwrap_or(Value::Nil);
            let ox = it.next().and_then(num_arg_opt).unwrap_or(0.0);
            let oy = it.next().and_then(num_arg_opt).unwrap_or(0.0);
            let rows = lua_table_rows(&lua, rows)?;
            let legend = lua_table_legend(&lua, legend)?;
            let (w, h) = (doc.width, doc.height);
            let (ox, oy) = (coord(ox, w)?, coord(oy, h)?);
            // 行宽一致性校验（错误信息命名具体行）
            let width = rows
                .first()
                .map(|r| r.chars().count())
                .ok_or(mlua::Error::RuntimeError("stamp rows empty".into()))?;
            for (i, r) in rows.iter().enumerate() {
                if r.chars().count() != width {
                    return Err(mlua::Error::RuntimeError(format!(
                        "stamp row {} has {} chars, expected {width}",
                        i + 1,
                        r.chars().count()
                    )));
                }
            }
            let mut legend_idx = Vec::new();
            for (sym, color) in &legend {
                let idx = resolve_color(doc, Value::String(lua.create_string(color.as_bytes())?))?;
                legend_idx.push((*sym, idx));
            }
            let cel = cel_mut(doc, layer, frame)?;
            for (dy, row) in rows.iter().enumerate() {
                let py = oy as usize + dy;
                if py >= h as usize {
                    return Err(mlua::Error::RuntimeError(format!(
                        "stamp row {} exceeds canvas height {}",
                        dy + 1,
                        h
                    )));
                }
                for (dx, ch) in row.chars().enumerate() {
                    let px = ox as usize + dx;
                    if px >= w as usize {
                        return Err(mlua::Error::RuntimeError(format!(
                            "stamp column {} exceeds canvas width {}",
                            dx + 1,
                            w
                        )));
                    }
                    let idx = if ch == '.' || ch == ' ' {
                        0
                    } else {
                        legend_idx
                            .iter()
                            .find(|(s, _)| *s == ch)
                            .map(|(_, i)| *i)
                            .ok_or_else(|| {
                                mlua::Error::RuntimeError(format!(
                                    "stamp symbol {ch} not in legend"
                                ))
                            })?
                    };
                    cel.set(w, px as u32, py as u32, idx);
                }
            }
            Ok(())
        })?;
        globals.set("stamp", stamp)?;
        Ok(())
    }

    /// 生成一个「读 registry 目标 + 取文档指针」的画布函数包装。
    fn canvas_fn<R, F>(&self, f: F) -> mlua::Result<Function>
    where
        R: mlua::IntoLuaMulti,
        F: Fn(&mut Document, &str, &str, mlua::MultiValue) -> mlua::Result<R> + 'static,
    {
        let doc_ptr = self.doc_ptr as usize;
        let layer = self.layer.clone();
        let frame = self.frame.clone();
        self.lua
            .create_function(move |_lua, args: mlua::MultiValue| {
                let doc = unsafe { &mut *(doc_ptr as *mut Document) };
                let layer = layer.borrow().clone();
                let frame = frame.borrow().clone();
                f(doc, &layer, &frame, args)
            })
    }

    fn install_instruction_guard(&self) {
        // Lua 侧软 guard：每次回调入口检查墙钟时间；指令硬上限由
        // mlua 的 hook 计数在 guard_trigger 中处理。
        let started = self.started;
        let max_secs = self.budget.max_seconds;
        let max_ins = self.budget.max_instructions;
        let used = Rc::new(Cell::new(0u64));
        let used_hook = used.clone();
        self.lua.set_hook(
            mlua::HookTriggers::new().every_nth_instruction(50_000),
            move |_lua, _debug| {
                used_hook.set(used_hook.get() + 50_000);
                if used_hook.get() > max_ins {
                    return Err(mlua::Error::RuntimeError(
                        "shader instruction budget exhausted".into(),
                    ));
                }
                if started.elapsed() > Duration::from_secs(max_secs) {
                    return Err(mlua::Error::RuntimeError(
                        "shader time budget exhausted".into(),
                    ));
                }
                Ok(mlua::VmState::Continue)
            },
        );
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        self.lua.remove_hook();
    }
}

// ---------- helpers ----------

/// Lua 值 -> 调色板下标，0 是透明。
///
/// 负数和超界都按「用户写下的那个数」报错。`as u16` 会把 -1 绕成 65535、
/// 把 70000 绕成 4464，报错里出现的下标和脚本里写的对不上，照着报错改脚本
/// 的人只会越改越糊涂。合法区间是 0..=palette.len()：0 透明，1 起是调色板。
fn index_from(v: &Value, palette_len: usize) -> mlua::Result<u16> {
    let raw: i64 = match v {
        Value::Integer(i) => *i,
        Value::Number(n) => n.round() as i64,
        Value::String(s) => {
            let text = s.to_str()?.to_string();
            return text.parse::<u16>().map_err(|_| {
                mlua::Error::RuntimeError(format!(
                    "palette index \"{text}\" is not a whole number - want 0..{palette_len}, \
                     0 is transparent"
                ))
            });
        }
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "palette index must be a number 0..{palette_len} (0 is transparent), got {other:?}"
            )));
        }
    };
    if raw < 0 || raw as usize > palette_len {
        return Err(mlua::Error::RuntimeError(format!(
            "palette index {raw} out of range (0..{palette_len}, 0 is transparent) - \
             add colors first with pixel_apply_operations add_palette_colors, \
             or use a \"#RRGGBB\" string, which every drawing call also takes"
        )));
    }
    Ok(raw as u16)
}

/// 颜色助手共用的入口：把 Lua 侧的颜色值解析成绝对颜色。
/// 「#hex」串与调色板索引两条都认——颜色在 Lua 侧是一种能到处传的值，
/// `mix(pal(1), hex('#000000'), 0.3)`、`alpha(1, 0.5)` 这样的串法都得成立。
/// 认不出来的报错要点明它收到了什么，模型才好改下一行。
fn value_to_rgba(v: &Value, doc: &Document) -> mlua::Result<Rgba> {
    match v {
        Value::String(s) => {
            let text = s.to_str()?.to_string();
            match text.as_str() {
                "transparent" | "nil" | "." | "" => Ok(Rgba::TRANSPARENT),
                _ => Rgba::parse_hex(&text).ok_or_else(|| {
                    mlua::Error::RuntimeError(format!(
                        "bad color \"{text}\" - want \"#RRGGBB\" / \"#RRGGBBAA\", \
                         a palette index, or transparent"
                    ))
                }),
            }
        }
        Value::Integer(i) => {
            if *i <= 0 {
                return Ok(Rgba::TRANSPARENT);
            }
            doc.color_of(*i as u16).ok_or_else(|| {
                mlua::Error::RuntimeError(format!(
                    "palette index {i} out of range (0..{}, 0 is transparent) - \
                     add colors first with pixel_apply_operations add_palette_colors, \
                     or use a \"#RRGGBB\" string, which every drawing call also takes",
                    doc.palette.len()
                ))
            })
        }
        Value::Number(n) => value_to_rgba(&Value::Integer(n.round() as i64), doc),
        Value::Nil => Ok(Rgba::TRANSPARENT),
        other => Err(mlua::Error::RuntimeError(format!(
            "color must be a \"#RRGGBB\" string or a palette index, got {other:?}"
        ))),
    }
}

fn num_arg(v: Option<Value>) -> mlua::Result<f64> {
    match v {
        Some(Value::Number(n)) => Ok(n),
        Some(Value::Integer(i)) => Ok(i as f64),
        Some(Value::String(s)) => {
            let t = s.to_str()?.to_string();
            t.parse::<f64>()
                .map_err(|_| mlua::Error::RuntimeError(format!("expected number, got {t}")))
        }
        _ => Err(mlua::Error::RuntimeError(
            "expected a number argument".into(),
        )),
    }
}

fn num_arg_opt(v: Value) -> Option<f64> {
    match v {
        Value::Number(n) => Some(n),
        Value::Integer(i) => Some(i as f64),
        _ => None,
    }
}

/// 解析 Lua 侧的点表。三种写法都收：`{{x=3,y=4},{x=20,y=6}}`、
/// `{{3,4},{20,6}}`、扁平 `{3,4,20,6}`。
///
/// 为什么要这么宽容：模型写点表的习惯差得离谱，而 aapoly 报一次错就白烧
/// 一整个请求。这里多认一种写法，比在提示词里多写三行规定便宜得多。
fn points_from_lua(value: &Value) -> mlua::Result<Vec<(f64, f64)>> {
    let table = value.as_table().ok_or_else(|| {
        mlua::Error::RuntimeError(
            "expected a table of points, e.g. {{x=3,y=4},{x=20,y=6}}".to_string(),
        )
    })?;
    let len = table.raw_len();
    // 单点表 {x=?,y=?} 也收：raw_len 对纯键表返回 0，按 1..=0 取什么都取不到。
    if len == 0 {
        let single = (|| -> mlua::Result<Option<(f64, f64)>> {
            let x = table.raw_get::<Value>(1)?;
            let y = table.raw_get::<Value>(2)?;
            if matches!(x, Value::Nil) || matches!(y, Value::Nil) {
                return Ok(None);
            }
            Ok(Some((num_arg(Some(x))?, num_arg(Some(y))?)))
        })()?;
        return match single {
            Some(point) => Ok(vec![point]),
            None => Err(mlua::Error::RuntimeError(
                "point table is empty - list points like {{x=3,y=4},{x=20,y=6}}".to_string(),
            )),
        };
    }
    let mut entries = Vec::with_capacity(len);
    let mut all_numbers = true;
    for i in 1..=len {
        let entry = table.raw_get::<Value>(i)?;
        if matches!(entry, Value::Nil) {
            return Err(mlua::Error::RuntimeError(format!(
                "point table has a hole at index {i} - list points without gaps"
            )));
        }
        if !matches!(entry, Value::Number(_) | Value::Integer(_)) {
            all_numbers = false;
        }
        entries.push(entry);
    }
    let mut out = Vec::with_capacity(entries.len());
    if all_numbers {
        // 扁平写法：坐标两两成对，落单一个说明中间少写了一个数。
        if entries.len() % 2 != 0 {
            return Err(mlua::Error::RuntimeError(
                "flat point table needs an even number of coordinates".to_string(),
            ));
        }
        for pair in entries.chunks(2) {
            out.push((
                num_arg(Some(pair[0].clone()))?,
                num_arg(Some(pair[1].clone()))?,
            ));
        }
    } else {
        for entry in &entries {
            let inner = entry.as_table().ok_or_else(|| {
                mlua::Error::RuntimeError(
                    "point entries must be {x=?,y=?} tables or plain numbers".to_string(),
                )
            })?;
            // 先按下标取，取不到再按键取：两种点表写法都放过。
            let (x, y) = match (inner.raw_get::<Value>(1), inner.raw_get::<Value>(2)) {
                (Ok(x), Ok(y)) if !matches!(x, Value::Nil) && !matches!(y, Value::Nil) => (x, y),
                _ => (inner.raw_get::<Value>("x")?, inner.raw_get::<Value>("y")?),
            };
            out.push((num_arg(Some(x))?, num_arg(Some(y))?));
        }
    }
    Ok(out)
}

/// 圆心 + 浮动半径换算成夹好界的盒子：`cx < r` 时下界为负，
/// 直接 `as u32` 会翻成 40 亿，把绘制循环拖死。
fn circle_box(cx: f64, cy: f64, r: f64, w: u32, h: u32) -> (u32, u32, u32, u32) {
    let lo = |v: f64, extent: u32| (v.round() as i64).clamp(0, extent.max(1) as i64 - 1) as u32;
    (lo(cx - r, w), lo(cy - r, h), lo(cx + r, w), lo(cy + r, h))
}

/// 把像素层的错误翻成 Lua 错误。沙箱里只认得一种错误类型，但参数错、
/// cel 缺失这类原文一个字都不能丢——模型改脚本全靠这句话定位。
fn doc_err<E: std::fmt::Display>(e: E) -> mlua::Error {
    mlua::Error::RuntimeError(e.to_string())
}

fn coord(v: f64, max: u32) -> mlua::Result<u32> {
    // 负坐标是模型最常犯的错（for i=-10,-2,4、一个减过头的偏移），而 mlua
    // 拿不到行号。报错不带可修的信息，模型就只能把同一份脚本原样重发：
    // 白白烧掉几轮请求，画布还是一片空白。所以把合法范围和常见成因塞进
    // 文案，等于替它把 bug 指出来。
    if !v.is_finite() || v < 0.0 {
        return Err(mlua::Error::RuntimeError(format!(
            "coordinate {v} is not inside the canvas (this axis is {max} wide, legal range 0..={};              a loop lower bound below zero or a negative offset is the usual cause)",
            max.saturating_sub(1)
        )));
    }
    let r = v.round() as u32;
    if r >= max {
        return Err(mlua::Error::RuntimeError(format!(
            "coordinate {v} rounds to {r}, outside canvas bound {max} (legal range 0..={})",
            max.saturating_sub(1)
        )));
    }
    Ok(r)
}

fn cel<'a>(doc: &'a Document, layer: &str, frame: &str) -> mlua::Result<&'a super::document::Cel> {
    doc.cel(layer, frame)
        .ok_or_else(|| mlua::Error::RuntimeError(format!("cel {layer}/{frame} missing")))
}

fn cel_mut<'a>(
    doc: &'a mut Document,
    layer: &str,
    frame: &str,
) -> mlua::Result<&'a mut super::document::Cel> {
    doc.cel_mut(layer, frame)
        .ok_or_else(|| mlua::Error::RuntimeError(format!("cel {layer}/{frame} missing")))
}

/// 解析颜色值：接受 `#hex` / `transparent` / 数字索引。
pub fn resolve_color(doc: &mut Document, value: Value) -> mlua::Result<u16> {
    resolve_color_for_layer(doc, "", value)
}

/// 带图层上下文的取色：锁着的层越界颜色就近归队，没锁的层原样放行。
/// `layer` 为空串（全局查询、没有当前层）时不做仲裁。
pub fn resolve_color_for_layer(doc: &mut Document, layer: &str, value: Value) -> mlua::Result<u16> {
    match value {
        Value::Integer(i) => {
            if i < 0 || i as usize >= doc.palette.len() {
                // 新文档的 palette 可能是空的：len()-1 会下溢，拿 usize::MAX 去
                // 报「范围 (0..18446744073709551615)」，读的人只会更糊涂。
                let top = doc.palette.len().saturating_sub(1);
                return Err(mlua::Error::RuntimeError(format!(
                    "color index {i} outside palette (0..{top}) - add colors first with \
                     pixel_apply_operations add_palette_colors, or pass a \"#RRGGBB\" string, \
                     which every drawing call also takes"
                )));
            }
            Ok(i as u16)
        }
        Value::Number(n) => resolve_color(doc, Value::Integer(n.round() as i64)),
        Value::String(s) => {
            let text = s.to_str()?.to_string();
            if text == "transparent" || text == "nil" || text == "." {
                return Ok(0);
            }
            let color = Rgba::parse_hex(&text)
                .ok_or_else(|| mlua::Error::RuntimeError(format!("bad color literal {text}")))?;
            let color = doc.color_for_layer(layer, color);
            doc.intern_color(color)
                .map_err(|e| mlua::Error::RuntimeError(e.to_string()))
        }
        Value::Nil => Ok(0),
        other => Err(mlua::Error::RuntimeError(format!(
            "color must be index or #hex string, got {other:?}"
        ))),
    }
}

/// 只读解析：同样接受索引与 #hex，但未知 hex 映射到 0（用于 replace 的 from）。
///
/// 下标负了或超界要报错，不能绕：`as u16` 把 -1 绕成 65535，而 65535 在 cel 里
/// 永远匹配不到，`replace` 就成了一个悄悄什么都不做的调用——模型以为自己改了
/// 色，画面一点没动，下一轮它照样照着那个错下标写。合法区间 0..=palette.len()，
/// 0（透明）在 replace 里是有意义的替换对象。
fn resolve_color_readonly(doc: &Document, value: Value) -> mlua::Result<u16> {
    match value {
        Value::Integer(i) => {
            if i < 0 || i as usize > doc.palette.len() {
                let top = doc.palette.len();
                return Err(mlua::Error::RuntimeError(format!(
                    "color index {i} outside palette (0..{top}, 0 is transparent) - \
                     pass a \"#RRGGBB\" string to match by color instead"
                )));
            }
            Ok(i as u16)
        }
        Value::Number(n) => resolve_color_readonly(doc, Value::Integer(n.round() as i64)),
        Value::String(s) => {
            let text = s.to_str()?.to_string();
            if text == "transparent" || text == "nil" || text == "." {
                return Ok(0);
            }
            Ok(Rgba::parse_hex(&text)
                .and_then(|c| doc.palette_index_of(c))
                .unwrap_or(0))
        }
        _ => Ok(0),
    }
}

fn lua_table_rows(_lua: &Lua, value: Value) -> mlua::Result<Vec<String>> {
    match value {
        Value::String(s) => Ok(s.to_str()?.lines().map(|l| l.to_string()).collect()),
        Value::Table(t) => {
            let mut rows = Vec::new();
            for entry in t.sequence_values::<String>() {
                rows.push(entry?);
            }
            Ok(rows)
        }
        _ => Err(mlua::Error::RuntimeError(
            "stamp rows must be a string or array of strings".into(),
        )),
    }
}

fn lua_table_legend(lua: &Lua, value: Value) -> mlua::Result<Vec<(char, String)>> {
    match value {
        Value::Table(t) => {
            let mut out = Vec::new();
            for pair in t.pairs::<String, String>() {
                let (k, v) = pair?;
                let sym = k.chars().next().ok_or_else(|| {
                    mlua::Error::RuntimeError("legend key must be one char".into())
                })?;
                out.push((sym, v));
            }
            let _ = lua;
            Ok(out)
        }
        _ => Err(mlua::Error::RuntimeError(
            "stamp legend must be a table of { [\"sym\"] = \"#hex\" }".into(),
        )),
    }
}

fn lerp(a: u8, b: u8, t: f64) -> u8 {
    (a as f64 + (b as f64 - a as f64) * t)
        .round()
        .clamp(0.0, 255.0) as u8
}

pub fn hsv_to_rgb(h: f64, s: f64, v: f64) -> Rgba {
    let h = ((h % 1.0) + 1.0) % 1.0;
    let s = s.clamp(0.0, 1.0);
    let v = v.clamp(0.0, 1.0);
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    let (r, g, b) = match i as i64 % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    Rgba::rgb(
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8,
    )
}

pub fn value_noise(x: f64, y: f64) -> f64 {
    let x0 = x.floor();
    let y0 = y.floor();
    let fx = smoothstep(x - x0);
    let fy = smoothstep(y - y0);
    let n00 = hash_noise(x0, y0);
    let n10 = hash_noise(x0 + 1.0, y0);
    let n01 = hash_noise(x0, y0 + 1.0);
    let n11 = hash_noise(x0 + 1.0, y0 + 1.0);
    let nx0 = n00 + (n10 - n00) * fx;
    let nx1 = n01 + (n11 - n01) * fx;
    (nx0 + (nx1 - nx0) * fy).clamp(0.0, 1.0)
}

fn smoothstep(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

fn hash_noise(x: f64, y: f64) -> f64 {
    let mut h = (x.to_bits()).wrapping_mul(0x9E3779B97F4A7C15)
        ^ (y.to_bits()).wrapping_mul(0xC2B2AE3D27D4EB4F);
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58476D1CE4E5B9);
    h ^= h >> 32;
    (h >> 11) as f64 / ((1u64 << 53) as f64)
}

/// 提示词里「照着抄」的那份上色配方：一条色相偏移的色阶，加一次按形体自身法线
/// 逐像素取带的重写法。抽成常量而不是只写进提示词字符串里，是为了它能被下面那个
/// 测试真跑一遍——提示词承诺的东西必须在沙箱里跑得通，并且真的把受光侧提亮、
/// 背光侧压暗。谁改了 `mix` / `pget` 的语义，这个测试第一个喊。
pub const SHADING_RECIPE: &str = r#"local function ramp(c, n) local d, l, o = mix(c, '#2a1f3d', 0.45), mix(mix(c, '#ffffff', 0.45), '#ffd9a8', 0.35), {}
  for i = 1, n do local t = (i - 1) / (n - 1)
    o[i] = t < 0.5 and mix(d, c, t / 0.5) or mix(c, l, (t - 0.5) / 0.5) end
  return o end
local R = ramp('#e8823a', 5)
circfill(32, 30, 11, R[3])
for y = 19, 41 do for x = 21, 43 do
  if pget(x, y) == R[3] then
    local nx, ny = (x - 32) / 11, (y - 30) / 11
    local nl = (nx * -0.7 + ny * -0.7) / math.sqrt(nx * nx + ny * ny)
    pset(x, y, nl > 0.62 and R[5] or nl > 0.25 and R[4] or nl > -0.25 and R[3] or nl > -0.6 and R[2] or R[1])
  end end end
outline(mix(R[1], '#e8823a', 0.35))"#;

/// 上色配方的任意轮廓版：法线不再从「圆心 + 半径」猜。
///
/// 老配方要模型自己报出主体中心和半径，这句话一离开球体就废：猫身子是扁的、
/// 尾巴是条矩形、树干上细下粗，按一个圆心算出来的法线在边缘上全错，画面还是平的。
/// 这份配方把法线改从像素自己推——第一遍扫出每一行基色的左右边界和整体的上下范围，
/// 第二遍把「行心的横向偏移」和「整体的纵向偏移」都除以主体的半高当法线点光向量。
/// 半高而不是半宽除法线：球体上这么算与老配方逐像素重合（短行不会被放大成法线），
/// 而宽身子读成一颗被水平拉长的球，正好是躯干该有的光。代价是每行多扫一遍，
/// 换回来是任意轮廓、任意多个连通块都吃得到层次，而且扫的范围只看基色，
/// 后来盖上去的条纹、眼睛、胡须不在扫描里，不会被重写掉。
///
/// 圆心边界情况：nx 与 ny 同时为 0（宽主体正中那一列）时老配方会算出 0/0，
/// 落进最暗一档，在矩形上就是一条从上到下的暗带；这里显式取中性档。
pub const FORM_SHADING_RECIPE: &str = r#"local function ramp(c, n) local d, l, o = mix(c, '#2a1f3d', 0.45), mix(mix(c, '#ffffff', 0.45), '#ffd9a8', 0.35), {}
  for i = 1, n do local t = (i - 1) / (n - 1)
    o[i] = t < 0.5 and mix(d, c, t / 0.5) or mix(c, l, (t - 0.5) / 0.5) end
  return o end
local R = ramp('#e8823a', 5)
local base, W, H = R[3], canvas.width, canvas.height
-- ONE flat base pass in the middle step, any contour you like; this is the line you swap
-- for your own subject. Everything drawn in another colour stays outside the scan.
circfill(32, 30, 11, base)
-- pass one: the left/right edge of every base-coloured row plus the body's top/bottom.
-- the normal is read off the pixels, so no centre and no radius has to be guessed.
local top, bot, rows = nil, nil, {}
for y = 0, H - 1 do
  local xa, xb = nil, nil
  for x = 0, W - 1 do
    if pget(x, y) == base then if not xa then xa = x end xb = x end
  end
  if xa then rows[y] = {xa, xb} if not top then top = y end bot = y end
end
-- pass two: band every base-coloured pixel by a normal read off the form itself
-- (row centre offset and body offset, both over the half height). The light direction is
-- baked in from the top-left: flip both signs for a light from the bottom-right.
-- Every loop-invariant is hoisted: a pget already costs a colour string and a 1024px
-- canvas has a million of them, so the fixed ones stay out of the inner loop.
local hh = bot and (bot - top) or 0
if top then
for y = top, bot do local r = rows[y] if r then
  local ra, rb = r[1], r[2]
  local ny = (hh > 0) and 2 * (y - top) / hh - 1 or 0
  for x = ra, rb do
    if pget(x, y) == base then
      local nx = (hh > 0 and rb > ra) and (2 * x - ra - rb) / hh or 0
      local n = math.sqrt(nx * nx + ny * ny)
      local nl = (n > 0) and (nx * -0.7 + ny * -0.7) / n or 0
      pset(x, y, nl > 0.62 and R[5] or nl > 0.25 and R[4] or nl > -0.25 and R[3] or nl > -0.6 and R[2] or R[1])
    end
  end
end end
end
outline(mix(R[1], '#e8823a', 0.35))"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use std::collections::BTreeSet;

    fn blank() -> Document {
        Document::new("test", 16, 16).expect("16x16 within limits")
    }

    /// 锁了配色范围的层跑 aa*：掺色前的解析必须先归队，柔边不许把用户的
    /// 配色表撑开。文档里那句「锁色板下 blend 会归回范围」靠的就是这条路径，
    /// aa* 三个坐标写法都走同一个 resolve_color_for_layer，测一条曲线就够。
    #[test]
    fn a_soft_curve_on_a_locked_layer_stays_inside_the_range() {
        let mut doc = blank();
        let layer = doc.layers[0].id.clone();
        let frame = doc.frames[0].id.clone();
        // 窄范围两色 + 上锁：#808080 不在范围内，归队只能落回这两色之一。
        doc.palettes = vec![crate::document::NamedPalette {
            id: "lock-test".to_string(),
            name: "lock test".to_string(),
            colors: vec![Rgba::rgb(255, 0, 0), Rgba::rgb(0, 0, 255)],
            builtin: false,
        }];
        doc.layers[0].palette_id = "lock-test".to_string();
        doc.layers[0].locked = true;
        run_shader(
            &mut doc,
            &layer,
            "aacurve(1, 8, 14, 8, 8, 2, '#808080')\n",
            false,
            &ShaderBudget::default(),
        )
        .expect("锁色板上的曲线不该报错");
        let cel = doc.cel(&layer, &frame).expect("cel");
        assert!(cel.indices.iter().any(|i| *i != 0), "曲线没有落到画布上");
        let stray = Rgba::rgb(0x80, 0x80, 0x80);
        assert!(
            !doc.palette.contains(&stray),
            "范围外的灰色被 intern 进了文档调色板：{:?}",
            doc.palette
        );
        for index in cel.indices.iter().filter(|i| **i != 0) {
            let color = doc.palette[(*index - 1) as usize];
            assert!(
                doc.palettes[0].colors.contains(&color),
                "画出去的颜色 {color:?} 不在锁定范围内"
            );
        }
    }

    /// 没锁的层照旧放行：归队只发生在上锁的层，否则「独立范围」会把自由层
    /// 也绑死，用户就没法在别的层上试色了。
    #[test]
    fn a_soft_curve_on_an_unlocked_layer_keeps_its_own_color() {
        let mut doc = blank();
        let layer = doc.layers[0].id.clone();
        doc.palettes = vec![crate::document::NamedPalette {
            id: "lock-test".to_string(),
            name: "lock test".to_string(),
            colors: vec![Rgba::rgb(255, 0, 0), Rgba::rgb(0, 0, 255)],
            builtin: false,
        }];
        doc.layers[0].palette_id = "lock-test".to_string();
        doc.layers[0].locked = false;
        run_shader(
            &mut doc,
            &layer,
            "aacurve(1, 8, 14, 8, 8, 2, '#808080')\n",
            false,
            &ShaderBudget::default(),
        )
        .expect("没锁的层不该报错");
        assert!(
            doc.palette.contains(&Rgba::rgb(0x80, 0x80, 0x80)),
            "没锁的层被归队了：{:?}",
            doc.palette
        );
    }

    /// 平滑落实的证据：同一条 45° 对角线，硬边只落一个色，柔边必须掺出多档
    /// 中间色。「基础图形太突兀」能拿数字说话的形式正是这个——突兀来自整格
    /// 要么全上要么全不上，所以中间档多出来才算柔边真的在起作用。
    /// 顺带钉住两姐妹的脚印：硬边那一档必须整个落在柔边铺开的格子里，
    /// 否则「aa* 和硬边孪生函数同一个坐标」这句承诺就是假的。
    #[test]
    fn a_soft_diagonal_lands_more_tones_than_the_hard_one() {
        fn tones_of(script: &str) -> BTreeSet<u16> {
            let mut doc = blank();
            let layer = doc.layers[0].id.clone();
            run_shader(&mut doc, &layer, script, false, &ShaderBudget::default())
                .expect("直线要在沙箱里跑通");
            let cel = doc.cel(&layer, "F0").expect("cel");
            cel.indices.iter().copied().filter(|i| *i != 0).collect()
        }
        let hard = tones_of("line(2, 2, 13, 13, '#808080')\n");
        let soft = tones_of("aaline(2, 2, 13, 13, '#808080')\n");
        assert_eq!(hard.len(), 1, "硬边直线只该有一档色：{hard:?}");
        assert!(
            soft.len() > hard.len(),
            "柔边直线该掺出多档中间色，实际 {} 档：{soft:?}",
            soft.len()
        );
        assert!(
            hard.is_subset(&soft),
            "硬边那一档没落在柔边范围内：{hard:?}"
        );
    }

    /// 画布说明文档承诺的每个画图函数名都必须在沙箱里真的注册着。
    ///
    /// 模型照文档调一个没注册的名字，只拿到一句 lua error，一轮输出预算
    /// 就废在瞎改名字上——文档与实现之间最贵的裂缝就是这种。所以逐个点名
    /// 验一次存在与类型，真参数调用由各函数的专项测试负责。
    /// 覆盖范围跟着 prompt.rs 里那段 SOFT EDGES AND SMOOTH CURVES 走：
    /// 硬边孪生函数、平滑家族全部别名、单格柔化，一个不漏。
    #[test]
    fn every_documented_drawing_helper_is_actually_registered() {
        const DOCUMENTED: &[&str] = &[
            // 硬边孪生函数：aa* 承诺「同样的坐标」，所以两套都得在。
            "pset",
            "pget",
            "line",
            "rect",
            "rectfill",
            "ellipse",
            "ellipfill",
            "ellipsefill",
            "circle",
            "circfill",
            "circlefill",
            "flood",
            "replace",
            "outline",
            "clear",
            "stamp",
            // 平滑家族与它在文档里点名的每个别名。
            "aaline",
            "aaseg",
            "aacurve",
            "aaquad",
            "aacubic",
            "aabez",
            "aapoly",
            "aapath",
            "aapolyfill",
            "aafill",
            "aacircle",
            "aacirc",
            "aaellipse",
            "aarect",
            // 单格柔化：差半格就够的场合用，不为它铺一条覆盖缓冲。
            "blend",
            "aablend",
            "dither",
        ];
        let mut probe = String::from("local missing = {}\n");
        for name in DOCUMENTED {
            probe.push_str(&format!(
                "if type(_G[{name:?}]) ~= 'function' then missing[#missing + 1] = {name:?} end\n"
            ));
        }
        probe.push_str(
            "if #missing > 0 then error('missing: ' .. table.concat(missing, ', ')) end\n",
        );
        let mut doc = blank();
        let layer = doc.layers[0].id.clone();
        run_shader(&mut doc, &layer, &probe, false, &ShaderBudget::default())
            .expect("文档承诺的画图函数都该注册在沙箱里");
    }

    /// 拿内置函数名当局部变量，报错里必须带上「改名」这句话。
    /// 模型没有这条提示只能把预算烧在瞎改上，一轮就没了。
    #[test]
    fn shadowing_a_builtin_gets_a_rename_hint() {
        let mut doc = blank();
        let layer = doc.layers[0].id.clone();
        let err = run_shader(
            &mut doc,
            &layer,
            "local line = \"x\" line(0, 0, 4, 4, \"#FF004D\")",
            false,
            &ShaderBudget::default(),
        )
        .expect_err("遮蔽内置函数必须报错");
        let text = err.to_string();
        assert!(text.contains("hint"), "错误里没有提示：{text}");
        assert!(
            text.contains("builtin") && text.contains("rename"),
            "提示要说清是内置名且要改名：{text}"
        );
    }

    /// 尺寸适配助手：装进沙盒的这六个数必须和提示词承诺的一模一样。
    /// 用长方形画布（120x48）是刻意的——正方形上 min == max == cx == cy，
    /// scale 少除了 64、cx 取了短边，全都会被对称性藏过去，专挑这种形状才炸得出来。
    /// 断言写在脚本里而不是逐个 get：模型拿不到 map 之外的键，assert 一失败
    /// run_shader 直接把消息带出来，比在 Rust 侧反推坐标直观。
    #[test]
    fn the_size_helpers_report_what_the_prompt_promises() {
        let mut doc = Document::new("sizes", 120, 48).expect("120x48 within limits");
        let layer = doc.layers[0].id.clone();
        let script = concat!(
            "assert(canvas.max == 120, 'max 要取长边: ' .. tostring(canvas.max))\n",
            "assert(canvas.min == 48, 'min 要取短边: ' .. tostring(canvas.min))\n",
            "assert(canvas.cx == 60, 'cx 要按长边取中: ' .. tostring(canvas.cx))\n",
            "assert(canvas.cy == 24, 'cy 要按短边取中: ' .. tostring(canvas.cy))\n",
            "assert(canvas.scale(32) == 60, 'scale 按长边 / 64 折算: ' .. tostring(canvas.scale(32)))\n",
            "local cw, ch = canvas.grid(4, 3)\n",
            "assert(cw == 30 and ch == 16, 'grid 要给出整数格宽高: ' .. tostring(cw) .. 'x' .. tostring(ch))\n",
            "local ow, oh = canvas.grid(0, 3)\n",
            "assert(ow == 120 and oh == 16, '格子数低于 1 按 1 算: ' .. tostring(ow))\n",
            "local zw, zh = canvas.grid(400, 3)\n",
            "assert(zw == 1 and zh == 16, '格子比画布还窄时至少留 1px: ' .. tostring(zw))\n",
            "pset(60, 24, '#ff004d')\n",
        );
        run_shader(&mut doc, &layer, script, false, &ShaderBudget::default())
            .expect("尺寸助手不该报错");
        let cel = doc.cel(&layer, &doc.frames[0].id.clone()).expect("cel");
        assert!(
            cel.indices.iter().any(|i| *i != 0),
            "断言全过了但画布是空的"
        );
    }

    /// 提示只在真撞名时出现：普通运行时错误不该被塞一句废话。
    #[test]
    fn ordinary_lua_errors_stay_clean() {
        let mut doc = blank();
        let layer = doc.layers[0].id.clone();
        let err = run_shader(
            &mut doc,
            &layer,
            "this_function_does_not_exist(1)",
            false,
            &ShaderBudget::default(),
        )
        .expect_err("调用不存在的函数必须报错");
        let text = err.to_string();
        assert!(!text.contains("hint"), "无关报错被加了提示：{text}");
        assert!(
            text.contains("this_function_does_not_exist"),
            "原文要留住：{text}"
        );
    }

    /// 脚本跑到一半报错：前半截已经画在 cel 上，报错就得把它撤干净。
    /// 留半截在画布上比什么都不留更糟——用户看不出哪儿是画坏的，模型读到
    /// 「画了多少个像素」也以为自己已经画上了，接着在错的底子上往下补。
    #[test]
    fn a_script_that_fails_half_way_paints_nothing() {
        let mut doc = blank();
        let layer = doc.layers[0].id.clone();
        let frame = doc.frames[0].id.clone();
        let err = run_shader(
            &mut doc,
            &layer,
            "pset(0, 0, hex('#ff004d'))\npset(-1, 0, hex('#00e436'))\n",
            false,
            &ShaderBudget::default(),
        )
        .expect_err("负坐标必须报错");
        let text = err.to_string();
        assert!(
            text.contains("legal range 0..=15"),
            "报错得把合法范围说清，模型才知道往哪儿改：{text}"
        );
        let painted = doc
            .cel(&layer, &frame)
            .unwrap()
            .indices
            .iter()
            .filter(|i| **i != 0)
            .count();
        assert_eq!(painted, 0, "报错的脚本不许在画布上留半截：{painted}");
    }

    /// 行号得从原文里抠出来：报错回给模型时要点名第几行，不然它只能整篇重读
    /// 自己的脚本，瘸着改。
    #[test]
    fn lua_errors_carry_their_line() {
        let mut doc = blank();
        let layer = doc.layers[0].id.clone();
        let err = run_shader(
            &mut doc,
            &layer,
            "local a = 1\nlocal b = 2\nnope(3)\n",
            false,
            &ShaderBudget::default(),
        )
        .expect_err("调用不存在的函数必须报错");
        let text = format!("{err:?}");
        match err {
            ShaderError::Lua { line, .. } => assert_eq!(line, Some(3), "第三行才调错的：{text}"),
            other => panic!("不该是别的变体：{other:?}"),
        }
    }

    /// 提示词写着 `hsv(h, s, v[, a])`。第四个参数曾经被 mlua 的元组解构静默
    /// 丢掉：半透明一整层凭空变成实色，不报错、不提示，模型甚至不知道自己少
    /// 画了一层。补上回归，别让这类「多给一个实参就哑掉」再发生。
    #[test]
    fn hsv_alpha_channel_actually_lands() {
        let mut doc = blank();
        let (layer, frame) = (doc.layers[0].id.clone(), doc.frames[0].id.clone());
        run_shader(
            &mut doc,
            &layer,
            "canvas.pset(1, 1, hsv(200, 0.9, 0.3, 0.5)) canvas.pset(2, 2, hsv(200, 0.9, 0.3))",
            false,
            &ShaderBudget::default(),
        )
        .expect("hsv 四参不该报错");
        let cel = doc.cel(&layer, &frame).unwrap();
        let half = doc.color_of(cel.get(doc.width, 1, 1).unwrap()).unwrap();
        let full = doc.color_of(cel.get(doc.width, 2, 2).unwrap()).unwrap();
        assert_eq!(half.a, 128, "0.5 的 alpha 要落成 128：{half:?}");
        assert_eq!(full.a, 255, "不传 alpha 仍是实色：{full:?}");
        // 带 alpha 的颜色必须走 rgba 表示，rgb 表示会把那 8 位丢掉。
        assert_eq!(
            (half.r, half.g, half.b),
            (full.r, full.g, full.b),
            "同一色相只有 alpha 不同"
        );
    }

    /// 提示词写着「erase the cel, or fill it with one color」。
    /// 底色填充曾经被整个丢掉，模型每次铺夜色都只拿回一张空白画布。
    #[test]
    fn clear_with_a_color_fills_the_cel_instead_of_erasing() {
        let mut doc = blank();
        let (layer, frame) = (doc.layers[0].id.clone(), doc.frames[0].id.clone());
        run_shader(
            &mut doc,
            &layer,
            "clear('#0B1026') canvas.pset(0, 0, '#FF004D')",
            false,
            &ShaderBudget::default(),
        )
        .expect("底色填充不该报错");
        let cel = doc.cel(&layer, &frame).unwrap();
        let bg = doc.palette_index_of(Rgba::rgb(0x0b, 0x10, 0x26)).unwrap();
        assert_eq!(cel.get(doc.width, 15, 15).unwrap(), bg, "整格都该是底色");
        assert_ne!(
            cel.get(doc.width, 0, 0).unwrap(),
            bg,
            "pset 之后左上角是前景"
        );
    }

    /// clear() 不带参数仍是擦除：宽度铺满色、下一句再 erase 要能回头。
    #[test]
    fn clear_without_a_color_still_erases() {
        let mut doc = blank();
        let (layer, frame) = (doc.layers[0].id.clone(), doc.frames[0].id.clone());
        run_shader(
            &mut doc,
            &layer,
            "clear('#FF004D') canvas.pset(1, 1, '#FF004D') canvas.clear(nil) canvas.pset(2, 2, '#FF004D')",
            false,
            &ShaderBudget::default(),
        )
        .expect("clear(nil) 是擦除，不该报错");
        let cel = doc.cel(&layer, &frame).unwrap();
        assert_eq!(cel.get(doc.width, 1, 1).unwrap(), 0, "擦除要真擦掉");
        assert_ne!(cel.get(doc.width, 2, 2).unwrap(), 0, "擦除后还能继续画");
        assert_eq!(cel.get(doc.width, 0, 0).unwrap(), 0);
    }

    /// 报错必须带脚本行号：提示词承诺「the error names the exact line」，
    /// 16 行的脚本在第 16 行炸，模型得知道改哪一行。
    #[test]
    fn script_errors_carry_the_line_number() {
        let mut doc = blank();
        let layer = doc.layers[0].id.clone();
        let mut script = String::new();
        for i in 0..15 {
            script.push_str(&format!("local v{i} = {i}\n"));
        }
        script.push_str("bad_literal(9999)\n");
        let err = run_shader(&mut doc, &layer, &script, false, &ShaderBudget::default())
            .expect_err("第 16 行调用不存在的函数必须报错");
        let text = err.to_string();
        assert!(text.contains(":16"), "行号没进报错：{text}");
    }

    /// `pal()` 的下标绕位：负数被 `as u16` 绕成 65535，报错里就成了一个用户
    /// 从没写过的数字。照那个数字改脚本，只会把对的写成错的。
    #[test]
    fn pal_reports_the_index_the_script_actually_wrote() {
        let mut doc = blank();
        doc.intern_color(crate::document::Rgba::rgb(255, 0, 77))
            .expect("one color");
        let layer = doc.layers[0].id.clone();
        let err = run_shader(
            &mut doc,
            &layer,
            "local c = pal(-1)",
            false,
            &ShaderBudget::default(),
        )
        .expect_err("负数下标必须报错");
        let text = err.to_string();
        assert!(text.contains("-1"), "报错要留住用户写的那个数：{text}");
        assert!(!text.contains("65535"), "下标被绕位了：{text}");
    }

    /// `replace` 的 from 绕位更坏：匹配永远落空，调用变成悄悄什么都不做。
    /// 模型以为自己改了色，画面一点没动，下一轮还照着错下标写。
    #[test]
    fn replace_refuses_an_out_of_range_index_instead_of_doing_nothing() {
        let mut doc = blank();
        doc.intern_color(crate::document::Rgba::rgb(255, 0, 77))
            .expect("one color");
        let layer = doc.layers[0].id.clone();
        let err = run_shader(
            &mut doc,
            &layer,
            "canvas.replace(-1, '#00E436')",
            false,
            &ShaderBudget::default(),
        )
        .expect_err("越界 from 必须报错");
        let text = err.to_string();
        assert!(text.contains("-1"), "报错要留住用户写的那个数：{text}");
        assert!(!text.contains("65535"), "下标被绕位了：{text}");
    }

    /// 提示词里那份上色配方必须真跑得通，而且方向不能反。配方跑不动，模型照着
    /// 抄完只拿到一个 lua error，一轮输出预算就没了；方向画反了，整张图的受光面
    /// 全部颠倒，比不画还糟。所以这里拿真实沙箱验一次：受光侧亮、背光侧暗、
    /// 中档只剩在明暗交界附近。
    #[test]
    fn the_documented_shading_recipe_lights_the_top_left() {
        let mut doc = Document::new("t", 64, 64).expect("64x64 within limits");
        let layer = doc.layers[0].id.clone();
        let out = run_shader(
            &mut doc,
            &layer,
            SHADING_RECIPE,
            false,
            &ShaderBudget::default(),
        )
        .expect("配方要在沙箱里跑通");
        assert!(out.opaque_pixels > 300, "球体该真的画上去了");

        let cel = doc.cel(&layer, "F0").expect("F0 cel");
        let lum = |x: u32, y: u32| -> f64 {
            let px = cel
                .get(doc.width, x, y)
                .and_then(|i| doc.color_of(i))
                .unwrap_or(crate::document::Rgba::TRANSPARENT);
            (0.299 * px.r as f64 + 0.587 * px.g as f64 + 0.114 * px.b as f64) / 255.0
        };
        // 球心 (32,30) 半径 11：左上 (26,24) 迎着 (-0.7,-0.7) 的光，
        // 右下 (38,36) 背着它。两点都在球内，且都在 outline 那一像素之外。
        let lit = lum(26, 24);
        let core = lum(38, 36);
        assert!(lit > core + 0.25, "受光侧 {lit} 该明显亮于背光侧 {core}");
        // 「整颗球被取过带」要能被数出来：球内至少出现四档颜色，
        // 而且没有任何一档占到一半以上——占了一半就说明还是平涂。
        let mut spread: std::collections::HashMap<u16, usize> = std::collections::HashMap::new();
        for y in 19..=41u32 {
            for x in 21..=43u32 {
                if let Some(i) = cel.get(doc.width, x, y) {
                    *spread.entry(i).or_default() += 1;
                }
            }
        }
        let solid: usize = spread.values().sum();
        let top = spread.values().copied().max().unwrap_or(0);
        assert!(
            spread.len() >= 4,
            "球内至少四档色阶，实际 {} 档：{spread:?}",
            spread.len()
        );
        assert!(
            top * 2 < solid,
            "任一档不该占到一半以上（{top}/{solid}），否则仍是平涂"
        );
    }

    /// 任意轮廓配方在球体上要和老配方逐像素一致。同一颗球、同一个 ramp，
    /// 只是法线的来路不同（半径归一化 vs 行归一化）——退化情形若对不上，
    /// 说明行归一化把曲面的方向算歪了，那它在猫身上也只会更歪。
    #[test]
    fn the_form_recipe_degenerates_to_the_round_one_on_a_sphere() {
        let mut doc = Document::new("t", 64, 64).expect("64x64 within limits");
        let layer = doc.layers[0].id.clone();
        let out = run_shader(
            &mut doc,
            &layer,
            FORM_SHADING_RECIPE,
            false,
            &ShaderBudget::default(),
        )
        .expect("任意轮廓配方要在沙箱里跑通");
        assert!(out.opaque_pixels > 300, "球体该真的画上去了");

        let mut old = Document::new("t", 64, 64).expect("64x64 within limits");
        let old_layer = old.layers[0].id.clone();
        run_shader(
            &mut old,
            &old_layer,
            SHADING_RECIPE,
            false,
            &ShaderBudget::default(),
        )
        .expect("老配方要能跑");
        let fresh = doc.cel(&layer, "F0").expect("新配方的 cel");
        let stale = old.cel(&old_layer, "F0").expect("老配方的 cel");
        // 比的是颜色不是下标：两次运行里 ramp 各档的 intern 顺序本来就不同，
        // 拿下标比等于拿记账顺序比画面。
        for y in 0..64u32 {
            for x in 0..64u32 {
                // 球心那一个像素是唯一的例外：老配方在圆心算出 0/0，NaN 比不过任何
                // 阈值，那一像素被推进最暗一档（单像素瑕疵）；新版显式取中性档。
                if x == 32 && y == 30 {
                    continue;
                }
                let here = fresh.get(doc.width, x, y).and_then(|i| doc.color_of(i));
                let there = stale.get(old.width, x, y).and_then(|i| old.color_of(i));
                assert_eq!(here, there, "球体同一像素 ({x},{y}) 两个配方给出的色阶不同");
            }
        }
    }

    /// 模型照抄的流程就是把配方里那一行 circfill 换成自己的平涂底。换成矩形之后
    /// 层次必须还在：老配方在矩形上按圆心取带，四角和边缘的阶全是错的，
    /// 这正是「换个身子就废」要修掉的东西。
    #[test]
    fn a_flat_rectangle_takes_layers_from_the_form_recipe() {
        let mut doc = Document::new("t", 64, 64).expect("64x64 within limits");
        let layer = doc.layers[0].id.clone();
        let script = FORM_SHADING_RECIPE.replace(
            "circfill(32, 30, 11, base)",
            "rectfill(14, 22, 50, 42, R[3])",
        );
        run_shader(&mut doc, &layer, &script, false, &ShaderBudget::default())
            .expect("矩形上任意轮廓配方要跑通");
        let cel = doc.cel(&layer, "F0").expect("F0 cel");
        let lum = |x: u32, y: u32| -> f64 {
            let px = cel
                .get(doc.width, x, y)
                .and_then(|i| doc.color_of(i))
                .unwrap_or(crate::document::Rgba::TRANSPARENT);
            (0.299 * px.r as f64 + 0.587 * px.g as f64 + 0.114 * px.b as f64) / 255.0
        };
        // 矩形左上 (16,24) 迎着左上光源，右下 (48,40) 背着它。
        let lit = lum(16, 24);
        let core = lum(48, 40);
        assert!(lit > core + 0.25, "受光侧 {lit} 该明显亮于背光侧 {core}");

        let mut spread: std::collections::HashMap<u16, usize> = std::collections::HashMap::new();
        for y in 22..=42u32 {
            for x in 14..=50u32 {
                if let Some(i) = cel.get(doc.width, x, y) {
                    *spread.entry(i).or_default() += 1;
                }
            }
        }
        let solid: usize = spread.values().sum();
        let top = spread.values().copied().max().unwrap_or(0);
        assert!(
            spread.len() >= 4,
            "矩形内至少四档色阶，实际 {} 档：{spread:?}",
            spread.len()
        );
        assert!(
            top * 2 < solid,
            "任一档不该占到一半以上（{top}/{solid}），否则仍是平涂"
        );
    }

    /// 复合主体：一次平涂画出椭圆身子和矩形尾巴（猫的样子）。第一遍扫到的是并集的
    /// 包围盒，每行按自己的左右边界归一化，所以身子的左上还是要亮、右下还是要暗，
    /// 尾巴作为同一个主体的一部分也一起吃到光。
    #[test]
    fn a_composite_body_takes_layers_from_the_form_recipe() {
        let mut doc = Document::new("t", 64, 64).expect("64x64 within limits");
        let layer = doc.layers[0].id.clone();
        let script = FORM_SHADING_RECIPE.replace(
            "circfill(32, 30, 11, base)",
            "ellipfill(18, 26, 46, 42, R[3]) rectfill(6, 30, 22, 34, R[3])",
        );
        run_shader(&mut doc, &layer, &script, false, &ShaderBudget::default())
            .expect("复合主体上任意轮廓配方要跑通");
        let cel = doc.cel(&layer, "F0").expect("F0 cel");
        let lum = |x: u32, y: u32| -> f64 {
            let px = cel
                .get(doc.width, x, y)
                .and_then(|i| doc.color_of(i))
                .unwrap_or(crate::document::Rgba::TRANSPARENT);
            (0.299 * px.r as f64 + 0.587 * px.g as f64 + 0.114 * px.b as f64) / 255.0
        };
        // 椭圆身子 bbox 18..46 x 26..42：左上取 (24,29)，右下取 (40,39)，
        // 两点都在椭圆内且在 outline 那一像素之外。
        let lit = lum(24, 29);
        let core = lum(40, 39);
        assert!(lit > core + 0.25, "身子左上 {lit} 该亮于右下 {core}");

        let mut spread: std::collections::HashMap<u16, usize> = std::collections::HashMap::new();
        for y in 26..=42u32 {
            for x in 6..=46u32 {
                if let Some(i) = cel.get(doc.width, x, y) {
                    *spread.entry(i).or_default() += 1;
                }
            }
        }
        assert!(
            spread.len() >= 3,
            "复合主体内至少三档色阶，实际 {} 档：{spread:?}",
            spread.len()
        );
    }
}
