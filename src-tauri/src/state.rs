//! Tauri 托管状态：会话表 + 用户自带模型配置。
//! 没有内置服务器、没有登录、没有计费；模型完全来自用户在本机填的 Provider。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agent_core::{AgentSession, Capabilities, ModelConfig, Protocol};
use pixel_core::document::Document;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

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
    config_dir: Mutex<PathBuf>,
    counter: Mutex<u64>,
}

impl Default for AppState {
    fn default() -> Self {
        AppState {
            sessions: Mutex::new(HashMap::new()),
            models: Mutex::new(ModelsFile::default()),
            config_dir: Mutex::new(PathBuf::new()),
            counter: Mutex::new(0),
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
        *self.config_dir.lock().unwrap() = dir;
        self.load_models();
        Ok(())
    }

    fn models_path(&self) -> PathBuf {
        self.config_dir.lock().unwrap().join("models.json")
    }

    fn load_models(&self) {
        let path = self.models_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        if let Ok(file) = serde_json::from_str::<ModelsFile>(&text) {
            *self.models.lock().unwrap() = file;
        }
    }

    fn save_models(&self) {
        let snapshot = self.models.lock().unwrap().clone();
        if let Ok(text) = serde_json::to_string_pretty(&snapshot) {
            let _ = std::fs::write(self.models_path(), text);
        }
    }

    pub fn models_view(&self) -> ModelsView {
        let file = self.models.lock().unwrap().clone();
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
                    capabilities: m.capabilities,
                    has_api_key: !m.api_key.trim().is_empty(),
                })
                .collect(),
        }
    }

    /// 当前生效的模型配置；没配过就返回空壳，让 provider 给出可读错误。
    pub fn active_config(&self) -> ModelConfig {
        let file = self.models.lock().unwrap();
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
            capabilities: Capabilities::default(),
        }
    }

    /// 按 id 取一份完整配置（含密钥），供会话改绑模型用。
    pub fn model_config(&self, id: &str) -> Result<ModelConfig, String> {
        let file = self.models.lock().unwrap();
        file.entries
            .iter()
            .find(|m| m.id == id)
            .cloned()
            .ok_or_else(|| format!("unknown model: {id}"))
    }

    /// 只把本机已存密钥交给 upsert 合并，前端始终看不到它。
    pub fn stored_api_key(&self, id: &str) -> Option<String> {
        let file = self.models.lock().unwrap();
        file.entries
            .iter()
            .find(|m| m.id == id)
            .map(|m| m.api_key.clone())
    }

    pub fn upsert_model(&self, incoming: ModelConfig, previous_key: Option<String>) {
        let mut file = self.models.lock().unwrap();
        let mut entry = incoming;
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
    }

    pub fn remove_model(&self, id: &str) {
        let mut file = self.models.lock().unwrap();
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
    }

    pub fn set_active_model(&self, id: &str) -> Result<(), String> {
        let mut file = self.models.lock().unwrap();
        if !file.entries.iter().any(|m| m.id == id) {
            return Err(format!("unknown model: {id}"));
        }
        file.active_id = id.to_string();
        drop(file);
        self.save_models();
        Ok(())
    }

    pub fn session(&self, id: &str) -> Result<Arc<AgentSession>, String> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| format!("unknown session: {id}"))
    }

    pub fn session_ids(&self) -> Vec<String> {
        self.sessions.lock().unwrap().keys().cloned().collect()
    }

    /// 建会话并按当前生效模型绑定 provider。
    pub fn create_session(&self, document: Document) -> Arc<AgentSession> {
        let mut counter = self.counter.lock().unwrap();
        *counter += 1;
        let id = format!("s{}", *counter);
        drop(counter);
        let session = Arc::new(AgentSession::new(
            id.clone(),
            self.active_config(),
            document,
        ));
        self.sessions
            .lock()
            .unwrap()
            .insert(id.clone(), session.clone());
        session
    }

    pub fn drop_session(&self, id: &str) {
        self.sessions.lock().unwrap().remove(id);
    }
}

/// 默认画布：64x64 单图层单帧。
pub fn default_document() -> Document {
    Document::new("untitled", 64, 64).expect("64x64 stays within document limits")
}
