// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! Tauri 托管状态：会话表 + 用户自带模型配置。
//! 没有内置服务器、没有登录、没有计费；模型完全来自用户在本机填的 Provider。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_core::{
    AgentSession, Capabilities, LoopLimits, McpRegistry, McpServerConfig, ModelConfig, Protocol,
    RunnerConfig,
};
use pixel_core::document::Document;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::mcp::McpFile;

/// 模型配置文件，落盘在 app config 目录。api_key 只留在本机。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelsFile {
    #[serde(default)]
    pub active_id: String,
    #[serde(default)]
    pub entries: Vec<ModelConfig>,
}

impl ModelsFile {
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
    models: Mutex<ModelsFile>,
    /// 运行护栏：续写、重试、纯思考的封顶值。所有会话共用一份。
    limits: Mutex<LoopLimits>,
    /// MCP 总开关。false 时新会话不挂注册表，活着的会话当场摘掉——
    /// 「关掉」必须当轮生效，否则用户以为关了就一定没在传。
    mcp_enabled: Mutex<bool>,
    /// MCP 服务器登记表：配置的唯一真相，mcp.json 只是它的落盘影子。
    mcp: Arc<McpRegistry>,
    config_dir: Mutex<PathBuf>,
    counter: Mutex<u64>,
}

impl Default for AppState {
    fn default() -> Self {
        AppState {
            sessions: Mutex::new(HashMap::new()),
            models: Mutex::new(ModelsFile::default()),
            limits: Mutex::new(LoopLimits::DEFAULT),
            mcp_enabled: Mutex::new(true),
            mcp: Arc::new(McpRegistry::new()),
            // 还没 bootstrap 也要有个站得住的配置目录：空路径会让 save_models()
            // 落到「当前工作目录/models.json」，跑一遍单测就等于往仓库里写一份
            // 含 api_key 的模型配置。setup 里的 bootstrap 会立刻把它换成真目录。
            config_dir: Mutex::new(scratch_config_dir()),
            counter: Mutex::new(0),
        }
    }
}

/// 给每个实例一个临时目录，免得并行单测互相盖同一份文件。
fn scratch_config_dir() -> PathBuf {
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

    pub fn save_mcp_file(&self) {
        let file = McpFile {
            entries: self.mcp.configs(),
        };
        if let Ok(text) = serde_json::to_string_pretty(&file) {
            let _ = std::fs::write(self.mcp_path(), text);
        }
    }

    fn load_mcp_file(&self) -> Vec<McpServerConfig> {
        let Ok(text) = std::fs::read_to_string(self.mcp_path()) else {
            return Vec::new();
        };
        match serde_json::from_str::<McpFile>(&text) {
            Ok(file) => file.entries,
            Err(e) => {
                eprintln!("mcp.json is broken, starting with no servers: {e}");
                Vec::new()
            }
        }
    }

    /// 开机恢复：逐条登记（校验不过的跳过），auto_connect 的再排队连。
    /// 一个坏服务器不该挡住启动，失败只留状态，不冒泡。
    fn schedule_mcp_recovery(&self) {
        let entries = self.load_mcp_file();
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
        // 不该因为一个坏文件让整个工具起不来。
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        let Ok(file) = serde_json::from_str::<LimitsFile>(&text) else {
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
            let _ = std::fs::write(self.limits_path(), text);
        }
    }

    fn load_models(&self) {
        let path = self.models_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        if let Ok(file) = serde_json::from_str::<ModelsFile>(&text) {
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
            let _ = std::fs::write(self.models_path(), text);
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
    pub fn create_session(&self, document: Document) -> Arc<AgentSession> {
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
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(id);
    }
}

/// 默认画布：64x64 单图层单帧。
pub fn default_document() -> Document {
    Document::new("untitled", 64, 64).expect("64x64 stays within document limits")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mcp_switch_governs_both_new_and_living_sessions() {
        // 默认开着：装了服务器的人不该再多一步。
        let state = AppState::default();
        assert!(state.mcp_enabled());
        let first = state.create_session(default_document());
        assert!(first.mcp_attached(), "默认状态下新会话要接外部工具");

        // 关掉：活着的那个当场摘掉，新来的也不再挂。
        state.set_mcp_enabled(false);
        assert!(
            !first.mcp_attached(),
            "关掉必须当轮生效，不是下个会话才生效"
        );
        assert!(!state.create_session(default_document()).mcp_attached());

        // 再开回来：双向都要通，否则开关只能关不能开。
        state.set_mcp_enabled(true);
        assert!(first.mcp_attached(), "重新打开要装回同一个共享注册表");
        assert!(state.create_session(default_document()).mcp_attached());
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
        let session = state.create_session(default_document());

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

    /// 会话正绑着的定义被删掉：落到当前生效模型上，而不是赖在「未绑定」。
    #[test]
    fn dropping_a_model_falls_its_sessions_back_to_the_active_one() {
        let state = AppState::default();
        state.upsert_model(model_def("m1", "一号"), None);
        state.upsert_model(model_def("m2", "二号"), None);
        state.set_active_model("m2").unwrap();
        let session = state.create_session(default_document());
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
}
