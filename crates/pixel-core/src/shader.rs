//! 沙箱 Lua 绘制：LLM 的生图主通道。
//! 「不让模型手写矩阵」——脚本体量与画布尺寸无关，循环/噪声/插值由 runtime 执行。
//!
//! 预算（前作实测值）：20M 指令 / 5 秒 / 64KB 脚本。

use super::document::{Document, Rgba};
use mlua::{Function, Lua, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

pub const MAX_SCRIPT_BYTES: usize = 64 * 1024;
pub const DEFAULT_MAX_INSTRUCTIONS: u64 = 20_000_000;
pub const DEFAULT_MAX_SECONDS: u64 = 5;

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
        ShaderError::Lua {
            message: e.to_string(),
            line: None,
        }
    }
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
        sandbox.exec(script)?;
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
        let _ = g.set("canvas_w", self.doc().width);
        let _ = g.set("canvas_h", self.doc().height);
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
                Err(ShaderError::Lua {
                    message: msg,
                    line: None,
                })
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
        self.install_instruction_guard();
        Ok(())
    }

    fn install_palette_functions(&self) -> mlua::Result<()> {
        let doc_ptr = self.doc_ptr as usize;
        let globals = self.lua.globals();

        // pal(i) -> "#rrggbb"
        let pal = self.lua.create_function(move |lua, idx: Value| {
            let doc = unsafe { &*(doc_ptr as *const Document) };
            let i = index_from(&idx);
            match doc.color_of(i) {
                // 索引 0 在 Lua 侧表达为 transparent，避免被当成黑色
                Some(c) if c == Rgba::TRANSPARENT => {
                    Ok(Value::String(lua.create_string(b"transparent")?))
                }
                Some(c) => Ok(Value::String(lua.create_string(c.to_hex().as_bytes())?)),
                None => Err(mlua::Error::RuntimeError(format!(
                    "palette index {i} out of range"
                ))),
            }
        })?;
        globals.set("pal", pal)?;

        // hex(value) -> index（#hex 先 intern；transparent -> 0）
        let hex_fn = self.lua.create_function(move |_lua, v: Value| {
            let doc = unsafe { &mut *(doc_ptr as *mut Document) };
            resolve_color(doc, v)
        })?;
        globals.set("hex", hex_fn)?;

        // mix(a, b, t) -> "#hex"
        let mix_fn = self
            .lua
            .create_function(|lua, (a, b, t): (String, String, f64)| {
                let ca = Rgba::parse_hex(&a)
                    .ok_or_else(|| mlua::Error::RuntimeError(format!("bad color {a}")))?;
                let cb = Rgba::parse_hex(&b)
                    .ok_or_else(|| mlua::Error::RuntimeError(format!("bad color {b}")))?;
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

        // hsv(h, s, v) -> "#hex"（h 取 0..360）
        let hsv_fn = self
            .lua
            .create_function(|lua, (h, s, v): (f64, f64, f64)| {
                let color = hsv_to_rgb(h / 360.0, s, v);
                Ok(Value::String(lua.create_string(color.to_hex().as_bytes())?))
            })?;
        globals.set("hsv", hsv_fn)?;

        // alpha(color, a) -> "#rrggbbaa"（a 接受 0..1 或 0..255）
        let alpha_fn = self.lua.create_function(|lua, (color, a): (String, f64)| {
            let base = Rgba::parse_hex(&color)
                .ok_or_else(|| mlua::Error::RuntimeError(format!("bad color {color}")))?;
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

        // rand() -> 0..1 确定性
        let rng = self.rng.clone();
        let rand_fn = self.lua.create_function(move |_lua, ()| {
            let mut x = rng.get();
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            rng.set(x);
            Ok(((x >> 11) as f64) / ((1u64 << 53) as f64))
        })?;
        globals.set("rand", rand_fn)?;

        // noise(x, y) -> 0..1 确定性
        let noise_fn = self
            .lua
            .create_function(|_lua, (x, y): (f64, f64)| Ok(value_noise(x, y)))?;
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
            super::ops::draw_line(cel, w, h, x0 as u32, y0 as u32, x1 as u32, y1 as u32, idx);
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
            super::ops::draw_ellipse(
                cel,
                w,
                h,
                (cx - r) as u32,
                (cy - r) as u32,
                (cx + r) as u32,
                (cy + r) as u32,
                idx,
                filled,
            );
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
            super::ops::draw_ellipse(
                cel,
                w,
                h,
                (cx - r) as u32,
                (cy - r) as u32,
                (cx + r) as u32,
                (cy + r) as u32,
                idx,
                true,
            );
            Ok(())
        })?;
        globals.set("circfill", circfill)?;

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

        let clear = self.canvas_fn(|doc, layer, frame, _args: mlua::MultiValue| {
            let cel = cel_mut(doc, layer, frame)?;
            cel.indices.iter_mut().for_each(|i| *i = 0);
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

fn index_from(v: &Value) -> u16 {
    match v {
        Value::Integer(i) => *i as u16,
        Value::Number(n) => n.round() as u16,
        Value::String(s) => s.to_str().map(|t| t.parse().unwrap_or(0)).unwrap_or(0),
        _ => 0,
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

fn coord(v: f64, max: u32) -> mlua::Result<u32> {
    if !v.is_finite() || v < 0.0 {
        return Err(mlua::Error::RuntimeError(format!(
            "coordinate {v} is not inside the canvas"
        )));
    }
    let r = v.round() as u32;
    if r >= max {
        return Err(mlua::Error::RuntimeError(format!(
            "coordinate {r} is outside canvas bound {max}"
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
    match value {
        Value::Integer(i) => {
            if i < 0 || i as usize >= doc.palette.len() {
                return Err(mlua::Error::RuntimeError(format!(
                    "color index {i} outside palette (0..{})",
                    doc.palette.len() - 1
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
fn resolve_color_readonly(doc: &Document, value: Value) -> mlua::Result<u16> {
    match value {
        Value::Integer(i) => Ok(i as u16),
        Value::Number(n) => Ok(n.round() as u16),
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
