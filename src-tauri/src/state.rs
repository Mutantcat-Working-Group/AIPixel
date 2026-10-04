// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! Tauri 托管状态：会话表 + 用户自带模型配置。
//! 没有内置服务器、没有登录、没有计费；模型完全来自用户在本机填的 Provider。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_core::{
    AgentSession, Capabilities, LoopLimits, McpRegistry, ModelConfig, Protocol, RunnerConfig,
};
use pixel_core::document::Document;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::mcp::McpFile;
use crate::mcp_server::McpServerRuntime;

/// 模型配置文件，落盘在 app config 目录。api_key 只留在本机。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelsFile {
    #[serde(default)]
    pub active_id: String,
    #[serde(default)]
    pub entries: Vec<ModelConfig>,
}

impl ModelsFile {
    /// 激活的定义；None 就是还没配模型。这里刻意不造一个默认兜底：
    /// 「没配」是设置页要如实显示的状态，顶上来的假配置只会让人以为好了。
    fn active(&self) -> Option<&ModelConfig> {
        self.entries.iter().find(|m| m.id == self.active_id)
    }
}

/// 运行护栏配置文件。与模型配置分开存：改护栏不该牵动 api_key，
/// 用户也方便把护栏单独发给别人复现一次「跑偏了」的现场。
/// MCP 总开关也住在这里：它和护栏一样是「这一台机器怎么跑」的选择，
/// 都不随模型定义走，也不该因为换个模型就被重置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitsFile {
    #[serde(default)]
    pub limits: LoopLimits,
    /// MCP 总开关。默认开着：装了服务器的人不用多一步，
    /// 不想让模型碰外部工具的人在设置里一次关干净。
    #[serde(default = "mcp_on_by_default")]
    pub mcp_enabled: bool,
}

fn mcp_on_by_default() -> bool {
    true
}

impl Default for LimitsFile {
    fn default() -> Self {
        Self {
            limits: LoopLimits::DEFAULT,
            mcp_enabled: true,
        }
    }
}

/// 回给前端的模型视图：不把 api_key 送到 webview，只告诉它「配过没有」。
#[derive(Debug, Clone, Serialize)]
pub struct ModelView {
    pub id: String,
    pub label: String,
    pub protocol: Protocol,
    pub base_url: String,
    pub model: String,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    /// 要不要每轮都关掉思考。透出的是用户意愿本身：`null` = 不干预（保留
    /// runner 那次自动翻盘），勾上 = 始终关，取消 = 用户要留着思考看。
    pub disable_thinking: Option<bool>,
    /// 用户勾选的能力；只透出布尔，不涉及任何凭证。
    pub capabilities: Capabilities,
    pub has_api_key: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelsView {
    pub active_id: String,
    pub entries: Vec<ModelView>,
}

pub struct AppState {
    sessions: Mutex<HashMap<String, Arc<AgentSession>>>,
    /// 会话簿脏标记。true = 有东西变了，异步循环该写盘了。
    /// 异步循环每两秒看一眼，脏才序列化整份——一次改标题就写一遍全量画布，
    /// 在一秒钟拖十次图层的用户手上就是十次磁盘。
    sessions_dirty: AtomicBool,
    /// 上一次写盘时各会话的廉价指纹（id、排序位、标题、模型、revision、历史条数）。
    /// 用来判「什么都没变就不用再写」。刻意不含画布内容：那条太贵，
    /// 而 revision 已经把每一次真正的落笔都覆盖到了。
    sessions_fingerprint: Mutex<String>,
    models: Mutex<ModelsFile>,
    /// 运行护栏：续写、重试、纯思考的封顶值。所有会话共用一份。
    limits: Mutex<LoopLimits>,
    /// MCP 总开关。false 时新会话不挂注册表，活着的会话当场摘掉——
    /// 「关掉」必须当轮生效，否则用户以为关了就一定没在传。
    mcp_enabled: Mutex<bool>,
    /// MCP 服务器登记表：配置的唯一真相，mcp.json 只是它的落盘影子。
    mcp: Arc<McpRegistry>,
    /// 「本程序当服务端」的那一份生命周期：开关、端口、监听停机信号。
    /// 和上面的登记表是反方向的两件事，但共用同一个 AppState——外部进程
    /// 从这端口改画布，和用户在前端点同一支笔，走的是同一条会话路径。
    mcp_server: Mutex<crate::mcp_server::McpServerRuntime>,
    config_dir: Mutex<PathBuf>,
    /// 关窗守门员登记。false 时 CloseRequested 直接放行——那一刻没有能
    /// 替用户拿主意的人，按住窗口只会让应用关不掉。前端 boot 时登记。
    close_guard: Mutex<bool>,
    /// 落盘串行化。三份配置共用一套「写临时文件再 rename」的路子，
    /// 两条命令同时保存时得排队，否则两份内容会互相盖对方的临时文件。
    save_lock: Mutex<()>,
    counter: Mutex<u64>,
}

impl Default for AppState {
    fn default() -> Self {
        AppState {
            sessions: Mutex::new(HashMap::new()),
            sessions_dirty: AtomicBool::new(false),
            sessions_fingerprint: Mutex::new(String::new()),
            models: Mutex::new(ModelsFile::default()),
            limits: Mutex::new(LoopLimits::DEFAULT),
            mcp_enabled: Mutex::new(true),
            mcp: Arc::new(McpRegistry::new()),
            mcp_server: Mutex::new(McpServerRuntime::default()),
            // 还没 bootstrap 也要有个站得住的配置目录：空路径会让 save_models()
            // 落到「当前工作目录/models.json」，跑一遍单测就等于往仓库里写一份
            // 含 api_key 的模型配置。setup 里的 bootstrap 会立刻把它换成真目录。
            config_dir: Mutex::new(scratch_config_dir()),
            close_guard: Mutex::new(false),
            save_lock: Mutex::new(()),
            counter: Mutex::new(0),
        }
    }
}

impl AppState {
    /// 落一份配置：全程只许一条线程在里面走。
    /// rename 本身是原子的，但「写临时文件」和「rename」之间不设防的话，
    /// 两次相邻的保存会共用同一个 .tmp 路径，后写的那份可能把前一份盖掉再换名。
    pub(crate) fn write_config(&self, path: &Path, text: &str, label: &str) {
        let _guard = self
            .save_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Err(e) = write_json_atomic(path, text) {
            eprintln!("cannot save {label}: {e}");
        }
    }

    /// 配置目录。测试里要能指到别处，所以不私有。
    pub(crate) fn config_dir(&self) -> PathBuf {
        self.config_dir
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// 会话簿落盘位置。和另三份配置待在同一个目录里。
    pub(crate) fn sessions_path(&self) -> PathBuf {
        self.config_dir().join("sessions.json")
    }

    /// 会话表的守卫。会话持久化要在锁内整批插入，所以不能只给只读接口。
    pub(crate) fn sessions_guard(&self) -> MutexGuard<'_, HashMap<String, Arc<AgentSession>>> {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 所有会话的 Arc 快照。刻意不把守卫交出去：序列化一份画布要花时间，
    /// 这把锁不能陪着挂在那里，期间「列会话」「改标题」都得能进。
    pub(crate) fn session_arcs(&self) -> Vec<Arc<AgentSession>> {
        self.sessions_guard().values().cloned().collect()
    }

    /// 建号计数器的当前值，写盘用：恢复时要靠它把计数器顶回已用过的最大值之上。
    pub(crate) fn counter_value(&self) -> u64 {
        *self
            .counter
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 把建号计数器顶到至少 n。只向上，不向下：计数器倒退会让新会话抢到旧号。
    pub(crate) fn bump_counter(&self, n: u64) {
        let mut counter = self
            .counter
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *counter < n {
            *counter = n;
        }
    }

    /// 标记会话簿该写盘了。各条改会话的命令都要调一次。
    pub fn note_sessions_dirty(&self) {
        self.sessions_dirty.store(true, Ordering::SeqCst);
    }

    /// 各会话的廉价指纹，用来判「什么都没变」。不含画布内容：
    /// 序列化一份画布正是要省掉的那笔开销，而 revision 已覆盖每一次真正的落笔。
    fn sessions_signature(&self) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        for session in self.session_arcs() {
            let _ = write!(
                out,
                "{}|{}|{}|{}|{}|{}",
                session.id(),
                session.order(),
                session.title().as_deref().unwrap_or(""),
                session.model_config().id,
                session.revision(),
                session.history_len(),
            );
            out.push('\n');
        }
        out
    }

    /// 记下「现在这份就等于盘上那份」。开机恢复完立刻调一次：
    /// 不记得的话，头两秒的轮询会把刚读回来的簿子原样再写一遍。
    fn remember_sessions_signature(&self) {
        let signature = self.sessions_signature();
        *self
            .sessions_fingerprint
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = signature;
    }

    /// 把会话簿写盘。什么都没变就什么都不做。
    ///
    /// 关机前也会被强叫一次（`RunEvent::Exit`）：异步循环有间隔，
    /// 用户改完名字紧接着关窗，那一下不能等两秒。
    pub fn flush_sessions(&self) {
        // 先把脏标记取走：就算这次写失败，也不该下一轮又从头来一遍。
        let forced = self.sessions_dirty.swap(false, Ordering::SeqCst);
        let signature = self.sessions_signature();
        if !forced
            && signature
                == *self
                    .sessions_fingerprint
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            return;
        }
        let file = crate::sessions::build(self);
        if let Ok(text) = serde_json::to_string_pretty(&file) {
            let path = self.sessions_path();
            self.write_config(&path, &text, "sessions.json");
            *self
                .sessions_fingerprint
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = signature;
        }
    }

    /// 开机把会话簿读回来。返回恢复了几条。
    pub fn restore_sessions(&self) -> usize {
        let count = crate::sessions::restore(self);
        self.remember_sessions_signature();
        count
    }
}

/// 给每个实例一个临时目录，免得并行单测互相盖同一份文件。
pub(crate) fn scratch_config_dir() -> PathBuf {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "aipixel-scratch-{}-{nanos}-{n}",
        std::process::id()
    ))
}

/// 原子落盘：先写同目录的临时文件，再 rename 盖上去。
///
/// `std::fs::write` 是「开、写、关」三步。中途断电、磁盘写满、或者进程正好
/// 在这一步被 kill，用户看到的就是一个被截断的 JSON。而 `load_models` 解析
/// 失败就退回默认值，下一次保存又把默认值写回去——用户整份模型配置连 API Key
/// 就这么没了，而且找不回来。同一个文件系统上的 rename 是原子的：要么旧内容
/// 完好，要么新内容完整，不会停在中间。
pub(crate) fn write_json_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    let atomic = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, path));
    if atomic.is_ok() {
        return Ok(());
    }
    // rename 在 Windows 上会输给占用：杀软扫一下、或者用户自己拿编辑器开着
    // 这份配置，rename 就失败，新内容独自留在 .tmp 里，而原地那份还是旧的——
    // 用户刚才改的设置看着"存过了"，其实一个字都没进去。
    // 退回直写目标：直写有截断风险，可那比让用户整份模型配置连 API Key
    // 一起蒸发要好得多。
    std::fs::write(path, text).map_err(|direct| {
        // 直写也失败：内容只剩 .tmp 这一份，留着让人能自己捞回来。
        eprintln!(
            "atomic save of {} failed ({atomic:?}); direct write failed too ({direct}); the content is still in {}",
            path.display(),
            tmp.display()
        );
        direct
    })?;
    let _ = std::fs::remove_file(&tmp);
    eprintln!(
        "atomic save of {} failed ({atomic:?}); wrote the file directly",
        path.display()
    );
    Ok(())
}

/// 读一份 JSON 配置；解析失败时把坏文件挪成 `.bak` 再说话。
///
/// 悄悄忽略坏文件是有代价的：内存里是默认值，而写回时会把它当成新内容覆盖，
/// 那一份含 api_key 的配置就真没了。挪去 `.bak` 至少留一份，用户自己能捞。
pub(crate) fn load_json_or_quarantine<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str::<T>(&text) {
        Ok(parsed) => Some(parsed),
        Err(e) => {
            let backup = path.with_extension("bak");
            let _ = std::fs::rename(path, &backup);
            eprintln!(
                "{} was not readable JSON ({e}); moved it to {}",
                path.display(),
                backup.display()
            );
            None
        }
    }
}

impl AppState {
    /// 在 Tauri setup 阶段调用：定位配置目录并读回模型配置。
    pub fn bootstrap(&self, app: &AppHandle) -> Result<(), String> {
        let dir = app
            .path()
            .app_config_dir()
            .map_err(|e| format!("cannot resolve config dir: {e}"))?;
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create config dir: {e}"))?;
        *self
            .config_dir
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = dir;
        self.load_models();
        self.load_limits();
        // 会话簿要在模型配置之后读：恢复时要按 id 找回会话原本绑的那个模型定义，
        // 读倒了只会让每个会话都落到「当前生效模型」上——画布还在，人却换了一批。
        let restored = self.restore_sessions();
        if restored > 0 {
            eprintln!("restored {restored} session(s)");
        }
        self.schedule_mcp_recovery();
        Ok(())
    }

    fn mcp_path(&self) -> PathBuf {
        self.config_dir
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .join("mcp.json")
    }

    pub fn mcp_registry(&self) -> Arc<McpRegistry> {
        self.mcp.clone()
    }

    /// 服务端运行时的守卫。刻意不返回 Arc：持锁的代码全都很短，而且
    /// 「拿设置 -> 改 -> 落盘」这三步之间不容许别人插进来改端口。
    ///
    /// 锁序警告：`save_mcp_file` 自己也要拿这把锁，所以任何持着本守卫的
    /// 代码都不要再调 `save_mcp_file`，否则同一线程二次上锁直接死掉。
    /// mcp_server.rs 里都是先用 `{}` 块把守卫 drop 掉再落盘的。
    pub fn mcp_server(&self) -> std::sync::MutexGuard<'_, crate::mcp_server::McpServerRuntime> {
        self.mcp_server
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn save_mcp_file(&self) {
        let file = McpFile {
            server: self.mcp_server().settings(),
            entries: self.mcp.configs(),
        };
        if let Ok(text) = serde_json::to_string_pretty(&file) {
            self.write_config(&self.mcp_path(), &text, "mcp.json");
        }
    }

    /// 开机恢复：逐条登记（校验不过的跳过），auto_connect 的再排队连。
    /// 一个坏服务器不该挡住启动，失败只留状态，不冒泡。
    fn schedule_mcp_recovery(&self) {
        // 一次读进来分两路用：entries 给外部服务器登记，server 给本程序
        // 自己当服务端的那一份设置。分两次读会让一个跑到一半被改坏的文件
        // 读出两份不一样的答案。
        let Some(file) = load_json_or_quarantine::<McpFile>(&self.mcp_path()) else {
            return;
        };
        // 「开着」必须先搬回内存再进 lib.rs 的 start_if_enabled：
        // 那个函数只认内存里的设置，读不到用户的开关就会让外部接不进来。
        self.mcp_server().set_settings(file.server);
        let entries = file.entries;
        let registry = self.mcp.clone();
        tauri::async_runtime::spawn(async move {
            for entry in entries {
                if registry.register(entry.clone()).await.is_err() {
                    continue;
                }
                if entry.auto_connect {
                    if let Err(e) = registry.connect(&entry.name).await {
                        eprintln!("auto-connect {} failed: {e}", entry.name);
                    }
                }
            }
        });
    }

    fn models_path(&self) -> PathBuf {
        self.config_dir
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .join("models.json")
    }

    fn limits_path(&self) -> PathBuf {
        self.config_dir
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .join("limits.json")
    }

    fn load_limits(&self) {
        let path = self.limits_path();
        // 读不出来（不存在、字段错位、被手改坏）都退回默认值：护栏是兜底，
        // 不该因为一个坏文件让整个工具起不来。坏的那份会挪成 .bak 留着，
        // 下一次保存不许把它当成新内容覆盖掉。
        let Some(file) = load_json_or_quarantine::<LimitsFile>(&path) else {
            return;
        };
        *self
            .limits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = file.limits;
        *self
            .mcp_enabled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = file.mcp_enabled;
    }

    fn save_limits(&self) {
        let snapshot = *self
            .limits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mcp_enabled = *self
            .mcp_enabled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Ok(text) = serde_json::to_string_pretty(&LimitsFile {
            limits: snapshot,
            mcp_enabled,
        }) {
            self.write_config(&self.limits_path(), &text, "limits.json");
        }
    }

    fn load_models(&self) {
        if let Some(file) = load_json_or_quarantine::<ModelsFile>(&self.models_path()) {
            *self
                .models
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = file;
        }
    }

    fn save_models(&self) {
        let snapshot = self
            .models
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Ok(text) = serde_json::to_string_pretty(&snapshot) {
            self.write_config(&self.models_path(), &text, "models.json");
        }
    }

    pub fn models_view(&self) -> ModelsView {
        let file = self
            .models
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        ModelsView {
            active_id: file.active_id.clone(),
            entries: file
                .entries
                .iter()
                .map(|m| ModelView {
                    id: m.id.clone(),
                    label: m.label.clone(),
                    protocol: m.protocol,
                    base_url: m.base_url.clone(),
                    model: m.model.clone(),
                    max_tokens: m.max_tokens,
                    temperature: m.temperature,
                    disable_thinking: m.disable_thinking,
                    capabilities: m.capabilities,
                    has_api_key: !m.api_key.trim().is_empty(),
                })
                .collect(),
        }
    }

    /// 当前生效的模型配置；没配过就返回空壳，让 provider 给出可读错误。
    pub fn active_config(&self) -> ModelConfig {
        let file = self
            .models
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(active) = file.active() {
            return active.clone();
        }
        ModelConfig {
            id: "unset".into(),
            label: "unset".into(),
            protocol: Protocol::Anthropic,
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
            max_tokens: None,
            temperature: None,
            disable_thinking: None,
            capabilities: Capabilities::default(),
        }
    }

    /// 按 id 取一份完整配置（含密钥），供会话改绑模型用。
    pub fn model_config(&self, id: &str) -> Result<ModelConfig, String> {
        let file = self
            .models
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        file.entries
            .iter()
            .find(|m| m.id == id)
            .cloned()
            .ok_or_else(|| format!("unknown model: {id}"))
    }

    /// 只把本机已存密钥交给 upsert 合并，前端始终看不到它。
    pub fn stored_api_key(&self, id: &str) -> Option<String> {
        let file = self
            .models
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        file.entries
            .iter()
            .find(|m| m.id == id)
            .map(|m| m.api_key.clone())
    }

    pub fn upsert_model(&self, incoming: ModelConfig, previous_key: Option<String>) {
        let mut file = self
            .models
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut entry = incoming;
        let changed_id = entry.id.clone();
        // 「会思考」不再是用户勾的：按模型名自己推。勾选那一栏撤掉之后，
        // 这一位仍然要有个准数——前端拿它决定占位气泡画成思考节点还是
        // 「在处理」。判定走 `echoes_reasoning`：和 runner 判定「要不要
        // 回传 reasoning_content」是同一张表，认得出 deepseek 简名与各家
        // 变体，认不出的当非推理模型（最坏只是占位气泡的样式降级）。
        entry.capabilities.reasoning = agent_core::providers::echoes_reasoning(&entry.model);
        if let Some(existing) = file.entries.iter_mut().find(|m| m.id == entry.id) {
            // 前端不一定是密钥的来源：留空表示沿用旧 key，避免密钥在 webview 里反复流转。
            if entry.api_key.trim().is_empty() {
                if let Some(key) = previous_key {
                    entry.api_key = key;
                }
            }
            existing.label = entry.label.clone();
            existing.protocol = entry.protocol;
            existing.base_url = entry.base_url.clone();
            existing.model = entry.model.clone();
            existing.max_tokens = entry.max_tokens;
            existing.temperature = entry.temperature;
            existing.api_key = entry.api_key.clone();
            // 「关不关思考」也得跟着保存。漏了它，用户在弹窗里关掉思考，
            // 之后只改一个 Base URL 再存，这一项就悄悄回到了「不干预」。
            existing.disable_thinking = entry.disable_thinking;
            // 能力必须跟着保存：漏了它，用户在弹窗里勾的勾选一关一开就丢了，
            // 而工作流可用性完全由它决定。
            existing.capabilities = entry.capabilities;
        } else {
            file.entries.push(entry);
        }
        if file.active_id.trim().is_empty() {
            file.active_id = file
                .entries
                .first()
                .map(|m| m.id.clone())
                .unwrap_or_default();
        }
        drop(file);
        self.save_models();
        // 改完要把还挂在这个定义上的会话重刷一遍：会话里的配置是创建时的一份快照，
        // 只写文件的话侧栏还显示旧名字，下一轮请求也照样用旧的 label / max_tokens。
        self.rebind_sessions_for(&changed_id);
    }

    pub fn remove_model(&self, id: &str) {
        let mut file = self
            .models
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        file.entries.retain(|m| m.id != id);
        if file.active_id == id {
            file.active_id = file
                .entries
                .first()
                .map(|m| m.id.clone())
                .unwrap_or_default();
        }
        drop(file);
        self.save_models();
        self.rebind_sessions_for(id);
    }

    pub fn set_active_model(&self, id: &str) -> Result<(), String> {
        let mut file = self
            .models
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !file.entries.iter().any(|m| m.id == id) {
            return Err(format!("unknown model: {id}"));
        }
        file.active_id = id.to_string();
        drop(file);
        self.save_models();
        self.rebind_sessions_for(id);
        Ok(())
    }

    /// 模型定义被改动之后，把还牵在上面的会话重绑到新配置上。
    ///
    /// 会话拿的是创建时的配置快照：用户在设置里改了名字、换了 max_tokens、改了能力，
    /// 不刷这一下的后果是侧栏显示旧名字（用户以为自己白改了），而下一轮请求用的还是
    /// 旧参数。provider 一起重建，所以不用重启、也不丢消息历史。
    ///
    /// 顺带收编两路「无主」会话：绑在一个已经不在表里的模型上（定义被删了），或者从
    /// 来没绑过（app 首次启动、一个模型都还没配时开出来的会话）。它们跟着当前生效的
    /// 模型走，不然侧栏永远挂着「未绑定」，用户存的第一个模型谁也用不上。
    fn rebind_sessions_for(&self, target: &str) {
        // 目标被删了就跟着当前生效模型走；一个都不剩时 active_config() 会给空壳，
        // 由 provider 去解释「还没配模型」，这里不自己编错误。
        let fresh = self
            .model_config(target)
            .unwrap_or_else(|_| self.active_config());
        let sessions: Vec<Arc<AgentSession>> = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        for session in sessions {
            let current = session.model_config();
            if current.id == target || self.model_config(&current.id).is_err() {
                session.rebind_provider(fresh.clone());
                continue;
            }
            // 主模型没动，但某个角色散绑在这个刚改过的定义上（生图、识图、读视频
            // 各自另绑的模型）：替身不跟着刷，那一格能力用的还是旧参数。
            for binding in session.role_bindings() {
                if binding.detached && binding.model_id == target {
                    session.rebind_role(binding.role, fresh.clone());
                }
            }
        }
    }

    /// 取一个会话的 Arc。刻意把克隆交出去而把 Guard 留在函数里：
    /// 一轮对话能跑几十秒，期间「列会话」「改标题」都得能进，
    /// 这把锁不能陪一整轮挂在那里。
    pub fn session(&self, id: &str) -> Result<Arc<AgentSession>, String> {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .cloned()
            .ok_or_else(|| format!("unknown session: {id}"))
    }

    pub fn session_ids(&self) -> Vec<String> {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }

    /// 当前护栏值。启动时是默认值，用户在设置里改过就一直是改过的那份。
    pub fn limits(&self) -> LoopLimits {
        *self
            .limits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 存护栏并推到所有活着的会话上。下一轮请求才会读到新值，
    /// 已经在飞的那一轮按老规矩收摊——打断一次请求只会更难看。
    pub fn set_limits(&self, limits: LoopLimits) {
        let limits = limits.clamped();
        *self
            .limits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = limits;
        let sessions: Vec<Arc<AgentSession>> = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        for session in sessions {
            let mut cfg = session.runner_config();
            cfg.loop_limits = limits;
            session.set_runner_config(cfg);
        }
        self.save_limits();
    }

    /// 建会话并按当前生效模型绑定 provider。
    pub fn create_session(&self, document: Document, title: Option<String>) -> Arc<AgentSession> {
        let mut counter = self
            .counter
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *counter += 1;
        let id = format!("s{}", *counter);
        // 排序位直接取创建序号：新会话天然排最后，侧边栏顺序与创建顺序一致。
        let order = *counter;
        drop(counter);
        // 新会话第一轮就得带上当前护栏：在这里漏掉，用户改完设置还得开个新会话才生效。
        let runner_config = RunnerConfig {
            loop_limits: self.limits(),
            ..RunnerConfig::default()
        };
        let session = Arc::new(
            AgentSession::new(id.clone(), self.active_config(), document)
                .with_mcp_registry(self.mcp.clone())
                .with_runner_config(runner_config)
                .with_title(title)
                .with_order(order),
        );
        // 总开关关着就别挂：新会话从第一轮起就看不见外部工具。
        if !*self
            .mcp_enabled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            session.set_mcp_registry(None);
        }
        // 新建的会话要落盘：不标记的话，用户建完会话就关窗，那一条是空的。
        self.note_sessions_dirty();
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id.clone(), session.clone());
        session
    }

    /// MCP 总开关的当前值。
    pub fn mcp_enabled(&self) -> bool {
        *self
            .mcp_enabled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 关窗守门员当前登记状态。
    pub fn close_guard(&self) -> bool {
        *self
            .close_guard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 装上/撤下关窗守门员。
    pub fn set_close_guard(&self, ready: bool) {
        *self
            .close_guard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = ready;
    }

    /// 开/关 MCP，并当场作用到所有活着的会话。
    /// 关的时候摘掉注册表而不是留着不读：模型下一轮看到的工具清单就是真实能力，
    /// 悬着一份「看得见但调不动」的注册表只会让模型反复撞墙。
    pub fn set_mcp_enabled(&self, enabled: bool) {
        *self
            .mcp_enabled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = enabled;
        let sessions: Vec<Arc<AgentSession>> = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        for session in sessions {
            session.set_mcp_registry(if enabled {
                Some(self.mcp.clone())
            } else {
                None
            });
        }
        self.save_limits();
    }

    pub fn drop_session(&self, id: &str) {
        // 删掉的那一笔也得落盘：光改内存的话，重启后这条会话又回来了，
        // 用户以为自己删过，白白再删一遍。
        self.note_sessions_dirty();
        let removed = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(id);
        // 在跑的那一笔必须先叫停：会话从簿里消失不等于 turn 会自己收手，
        // 少了这一步，删掉的会话还在继续烧额度，而中断按钮已经无从按起。
        if let Some(session) = removed {
            session.interrupt();
        }
        // 广播闸门跟着会话一起收尸：留着的话，同名会话重建后会继承上一个的
        // 合并窗口，第一发广播被无端压后一小会儿。
        crate::broadcast::release(id);
    }
}

/// 默认画布：64x64 单图层单帧。
pub fn default_document() -> Document {
    Document::new("untitled", 64, 64).expect("64x64 stays within document limits")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 把配置目录指到一个指定位置。单测里「存一份再读回来」这条路要两个实例
    /// 看同一个目录，默认那个每个实例都不相同的临时目录就无从写起。
    impl AppState {
        pub(crate) fn point_config_dir_at(&self, dir: PathBuf) {
            std::fs::create_dir_all(&dir).expect("scratch dir");
            *self
                .config_dir
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = dir;
        }
    }

    #[test]
    fn the_mcp_switch_governs_both_new_and_living_sessions() {
        // 默认开着：装了服务器的人不该再多一步。
        let state = AppState::default();
        assert!(state.mcp_enabled());
        let first = state.create_session(default_document(), None);
        assert!(first.mcp_attached(), "默认状态下新会话要接外部工具");

        // 关掉：活着的那个当场摘掉，新来的也不再挂。
        state.set_mcp_enabled(false);
        assert!(
            !first.mcp_attached(),
            "关掉必须当轮生效，不是下个会话才生效"
        );
        assert!(!state
            .create_session(default_document(), None)
            .mcp_attached());

        // 再开回来：双向都要通，否则开关只能关不能开。
        state.set_mcp_enabled(true);
        assert!(first.mcp_attached(), "重新打开要装回同一个共享注册表");
        assert!(state
            .create_session(default_document(), None)
            .mcp_attached());
    }

    /// 一条最小可用的模型定义：字段要全填，命令层就是这么校验的。
    fn model_def(id: &str, label: &str) -> ModelConfig {
        ModelConfig {
            id: id.into(),
            label: label.into(),
            protocol: Protocol::OpenAiCompat,
            base_url: "https://example.invalid/v1".into(),
            api_key: "test-key".into(),
            model: "test-model".into(),
            max_tokens: None,
            temperature: None,
            disable_thinking: None,
            capabilities: Capabilities::default(),
        }
    }

    /// 侧栏显示的名字来自会话里那份配置快照。不跟着设置刷新的后果是用户改了名
    /// 却看不出变化，下一轮请求也还在用旧的 label 与 max_tokens。
    #[test]
    fn renaming_a_model_refreshes_the_sessions_bound_to_it() {
        let state = AppState::default();
        let session = state.create_session(default_document(), None);

        // 一个模型都还没配时开出来的会话：存下第一个定义就该被收编，
        // 不然侧栏一直挂着「未绑定」，用户存的模型谁也用不上。
        state.upsert_model(model_def("m1", "旧名字"), None);
        assert_eq!(session.model_config().label, "旧名字");

        let mut renamed = model_def("m1", "新名字");
        renamed.max_tokens = Some(4096);
        state.upsert_model(renamed, Some("test-key".into()));

        let live = session.model_config();
        assert_eq!(live.label, "新名字", "侧栏显示的名字要跟着设置走");
        assert_eq!(live.max_tokens, Some(4096), "参数也要换成新存的那份");
    }

    /// 建会话时给的名字要真的落到侧栏显示上：不填回落到编号，填了原样留着。
    #[test]
    fn a_session_keeps_the_name_it_was_created_with() {
        let state = AppState::default();

        // 不填：侧栏回落到编号 s1，不能塞个空名进去。
        assert_eq!(state.create_session(default_document(), None).title(), None);

        // 填了：原样留着；前后空白要去掉，不然侧栏看着像多了一截空格。
        let named = state.create_session(default_document(), Some("  橘猫  ".into()));
        assert_eq!(named.title().as_deref(), Some("橘猫"));

        // 空白名等同没填：用户在输入框里删空了不该得到一个空名字会话。
        assert_eq!(
            state
                .create_session(default_document(), Some("   ".into()))
                .title(),
            None,
            "纯空白名等于没填，不是给个空标题"
        );
    }

    /// 会话正绑着的定义被删掉：落到当前生效模型上，而不是赖在「未绑定」。
    #[test]
    fn dropping_a_model_falls_its_sessions_back_to_the_active_one() {
        let state = AppState::default();
        state.upsert_model(model_def("m1", "一号"), None);
        state.upsert_model(model_def("m2", "二号"), None);
        state.set_active_model("m2").unwrap();
        let session = state.create_session(default_document(), None);
        session.rebind_provider(state.model_config("m1").unwrap());
        assert_eq!(session.model_config().id, "m1");

        state.remove_model("m1");

        assert_eq!(
            session.model_config().id,
            "m2",
            "被删的定义不能再挂在会话上"
        );
    }

    /// 「关不关思考」要跟着改：用户在弹窗里关掉它，之后只动一个 Base URL
    /// 再存盘，这一项不该悄悄回到「不干预」——那正是无限思考的病根。
    #[test]
    fn editing_other_fields_keeps_the_thinking_choice() {
        let state = AppState::default();
        state.upsert_model(model_def("m1", "一号"), None);

        let mut off = model_def("m1", "一号");
        off.disable_thinking = Some(true);
        state.upsert_model(off, Some("test-key".into()));
        assert_eq!(
            state.model_config("m1").unwrap().disable_thinking,
            Some(true)
        );

        // 之后只改地址再存一次：思考的意愿不能丢。
        let mut moved = model_def("m1", "一号");
        moved.disable_thinking = Some(true);
        moved.base_url = "https://example.invalid/v2".into();
        state.upsert_model(moved, Some("test-key".into()));
        let live = state.model_config("m1").unwrap();
        assert_eq!(
            live.disable_thinking,
            Some(true),
            "别处一改就不该把思考的意愿冲掉"
        );
        assert_eq!(live.base_url, "https://example.invalid/v2");
    }

    /// 保存要留下完整的一份：写坏的 models.json 会被读成默认值，
    /// 下一次保存就把用户整份配置（含 api_key）覆写成空的。
    #[test]
    fn a_corrupt_config_file_is_quarantined_not_silently_ignored() {
        let dir = scratch_config_dir();
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("models.json");
        std::fs::write(&path, "{ not json at all").expect("seed a broken file");

        let loaded: Option<serde_json::Value> = load_json_or_quarantine(&path);
        assert!(loaded.is_none(), "坏文件不能当成有效配置读进来");
        assert!(
            dir.join("models.bak").exists(),
            "坏文件必须留一份 .bak，否则内容连同 api_key 一起消失"
        );
        assert!(!path.exists(), "原位不能再留着那份坏内容");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_write_leaves_no_temporary_file_behind() {
        let dir = scratch_config_dir();
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("models.json");
        write_json_atomic(&path, "{\"a\":1}").expect("write");

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"a\":1}");
        assert!(
            !dir.join("models.tmp").exists(),
            "临时文件必须被 rename 带走，留在目录里只会越攒越多"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// rename 输给占用（Windows 上杀软扫一下、或者用户拿编辑器开着这份配置）
    /// 时，新内容独自留在 .tmp 里，原地那份还是旧的——用户看着"存过了"。
    /// 这里把 .tmp 占成一个目录，让原子落盘这一步必败，验证会退回直写。
    #[test]
    fn a_locked_target_falls_back_to_writing_the_file_directly() {
        let dir = scratch_config_dir();
        std::fs::create_dir_all(&dir).expect("scratch dir");
        std::fs::create_dir_all(dir.join("models.tmp")).expect("把 tmp 占住");
        let path = dir.join("models.json");

        write_json_atomic(&path, "{\"b\":2}").expect("退回直写后必须成功");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\"b\":2}",
            "配置一个字节都不许丢，那是用户的 api_key"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 两条路都走不通（目标路径本身不是文件）时，报错的同时必须把内容
    /// 留在 .tmp 里：那只是这次保存失败，删了 tmp 就是用户永远找不回来。
    #[test]
    fn when_both_paths_fail_the_content_is_kept_in_the_temporary_file() {
        let dir = scratch_config_dir();
        std::fs::create_dir_all(&dir).expect("scratch dir");
        std::fs::create_dir_all(dir.join("models.json")).expect("把目标占成目录");

        let err = write_json_atomic(&dir.join("models.json"), "{\"c\":3}");
        assert!(err.is_err(), "两条路都失败时必须报错，不能假装存好了");
        assert_eq!(
            std::fs::read_to_string(dir.join("models.tmp")).unwrap(),
            "{\"c\":3}",
            "内容必须留在 .tmp 里等人来捞"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 文档从外部灌进来（前端回灌、别的进程发 JSON）时必须自洽：
    /// 尺寸放行 u32::MAX 的话，后面一次 width*height 分配就能打死进程。
    #[test]
    fn an_outsized_document_from_the_frontend_is_refused() {
        let good = Document::new("ok", 8, 8).expect("8x8 is valid");
        assert!(good.clone().validated().is_ok());

        let absurd = Document {
            width: u32::MAX,
            height: u32::MAX,
            ..good
        };
        assert!(absurd.validated().is_err(), "u32::MAX 的宽高必须被挡在门外");
    }
}
