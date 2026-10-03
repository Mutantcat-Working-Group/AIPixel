// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 会话持久化：关掉再开回来，会话、画布、聊天记录都还得在。
//!
//! 会话原先只活在内存里：用户画到一半关掉软件，回来侧栏空空如也，上一个工程
//! 连 .aip 都没来得及导出就没了。这里把每个会话存成一份快照，落在 config 目录的
//! `sessions.json`；画布直接用既有的 .aip v2 文本，不另造一种序列化。
//!
//! 与 models.json / limits.json / mcp.json 共用同一套「写临时文件再 rename」的
//! 落盘路子（`AppState::write_config` / `write_json_atomic`），改一处行为三处一起受益。
//!
//! 快照带魔数与版本号：将来字段变了要能认得出来。认不出的那份宁可整份不要，
//! 也不能把半解的会话灌回内存——那比不恢复更糟，用户会以为会话都还在。

use std::sync::Arc;

use agent_core::{AgentSession, Message, ModelRole, RunnerConfig};
use serde::{Deserialize, Serialize};

use crate::state::{load_json_or_quarantine, AppState};

/// 文件头魔数。换格式时改它，旧版本的文件就会被当成不认识的东西挡掉。
pub const SESSIONS_MAGIC: &str = "aipixel-sessions";
/// 快照格式版本。字段含义变了才动它。
pub const SESSIONS_VERSION: u32 = 1;

/// 异步落盘的间隔。两件事都在这儿权衡：间隔太短，改一次标题就写一遍全量画布，
/// 一秒钟拖十次图层就是十次磁盘；拖太久，用户刚画完就断电，最近的几笔白画。
pub const AUTOSAVE_INTERVAL_MS: u64 = 2000;

/// 落盘的会话簿。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionsFile {
    pub format: String,
    pub version: u32,
    /// 建号计数器的读表值。恢复时要把它顶回已用过的最大值之上，
    /// 否则新会话会拿到一个已存在的 id，把恢复回来的那个顶掉。
    #[serde(default)]
    pub counter: u64,
    #[serde(default)]
    pub entries: Vec<SessionSnapshot>,
}

impl SessionsFile {
    /// 空簿。落一份"一条会话都没有"的状态也要有正经的文件头。
    #[cfg(test)]
    fn blank(counter: u64) -> Self {
        Self {
            format: SESSIONS_MAGIC.to_string(),
            version: SESSIONS_VERSION,
            counter,
            entries: Vec::new(),
        }
    }

    /// 这份文件是不是我们这个程序写的、版本对不对得上。
    fn is_ours(&self) -> bool {
        self.format == SESSIONS_MAGIC && self.version == SESSIONS_VERSION
    }
}

/// 一个会话的快照。id 是恢复的钥匙，order 是侧边栏的排序位。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub id: String,
    /// None 就是用户没起名，侧栏显示编号。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub order: u64,
    /// 主模型 id。读回来发现定义已经没了，就落到当前生效模型上：
    /// 用户删掉的模型不该让整个会话起不来。
    #[serde(default)]
    pub model_id: String,
    /// 另绑过模型的角色。没另绑的那些回落主模型，不必记。
    #[serde(default)]
    pub roles: Vec<RoleSnapshot>,
    /// 画布的 .aip v2 文本。
    pub document: String,
    #[serde(default)]
    pub messages: Vec<Message>,
}

/// 一条角色绑定只留「谁在干」：标签、能力都能从模型定义现推，
/// 存下来只会和用户的下一处修改对不上。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleSnapshot {
    pub role: ModelRole,
    pub model_id: String,
}

/// 把一个活着的会话抄成快照。
pub(crate) fn snapshot(session: &AgentSession) -> SessionSnapshot {
    SessionSnapshot {
        id: session.id().to_string(),
        title: session.title(),
        order: session.order(),
        model_id: session.model_config().id,
        roles: session
            .role_bindings()
            .into_iter()
            .filter(|binding| binding.detached)
            .map(|binding| RoleSnapshot {
                role: binding.role,
                model_id: binding.model_id,
            })
            .collect(),
        // 序列化失败时给空串，恢复那一侧认得出空串并跳过这条会话。
        // 兜一个坏文档，比让整份簿子存不下去要好。
        document: session.aip_text().unwrap_or_default(),
        messages: session.history(),
    }
}

/// 从会话 id 里取编号。认不出就不是我们发的号，恢复时直接跳过——
/// 放任一个来路不明的 id 进来，会和建号计数器对着干。
fn id_number(id: &str) -> Option<u64> {
    id.strip_prefix('s')?.parse::<u64>().ok()
}

/// 拿快照重建一个会话。读不回来就 `None`，由调用方跳过这条。
///
/// 模型定义可能已经不在表里（用户删过）：那就落回当前生效模型，
/// 用户回来至少还有画布和历史，不至于整个工程消失。
pub(crate) fn rebuild(app: &AppState, snap: &SessionSnapshot) -> Option<Arc<AgentSession>> {
    if id_number(&snap.id).is_none() || snap.document.trim().is_empty() {
        return None;
    }
    let doc = pixel_core::aip::import_any(&snap.document).ok()?;
    let doc = doc.validated().ok()?;
    let config = app
        .model_config(&snap.model_id)
        .unwrap_or_else(|_| app.active_config());
    let session = Arc::new(
        AgentSession::new(snap.id.clone(), config, doc)
            .with_mcp_registry(app.mcp_registry())
            .with_runner_config(RunnerConfig {
                // 恢复出来的会话第一轮就得带上当前护栏：用户上次调高的续写次数
                // 不该因为一次重启就悄悄退回默认值。
                loop_limits: app.limits(),
                ..RunnerConfig::default()
            })
            .with_title(snap.title.clone())
            .with_order(snap.order),
    );
    // MCP 总开关关着就别挂：刚启动那一下它是关的，恢复不能把这条规矩绕过去。
    if !app.mcp_enabled() {
        session.set_mcp_registry(None);
    }
    if !snap.messages.is_empty() {
        session.load_history(snap.messages.clone());
    }
    for role in &snap.roles {
        if let Ok(config) = app.model_config(&role.model_id) {
            session.rebind_role(role.role, config);
        }
    }
    Some(session)
}

/// 开机恢复会话簿。返回恢复了几条。
///
/// 读不到文件是常态（第一次跑），那不是错误。文件在但读不懂也不报错：
/// 会话是用户资产，报错只会拦住启动。
pub(crate) fn restore(app: &AppState) -> usize {
    let Some(file) = load_json_or_quarantine::<SessionsFile>(&app.sessions_path()) else {
        return 0;
    };
    if !file.is_ours() {
        return 0;
    }
    // 先重建再进簿：rebuild 要读模型表、护栏、MCP 开关，那些都不碰 sessions 锁，
    // 但把重建挪到锁外，将来加什么也不会一不小心就自锁。
    let mut rebuilt = Vec::new();
    for snap in &file.entries {
        if let Some(session) = rebuild(app, snap) {
            rebuilt.push(session);
        }
    }
    let highest = rebuilt
        .iter()
        .filter_map(|s| id_number(s.id()))
        .max()
        .unwrap_or(0)
        .max(file.counter);
    let count = rebuilt.len();
    {
        let mut guard = app.sessions_guard();
        for session in rebuilt {
            guard.insert(session.id().to_string(), session);
        }
    }
    // 建号计数器顶到已用过的最大值之上：不然新会话拿到 s1、s2 这些旧号，
    // 往 HashMap 里一插就把刚恢复的会话顶没了。
    app.bump_counter(highest);
    count
}

/// 把当前会话簿序列化成一份文件结构。空簿也照常产出：
/// 用户删光会话之后，「一条都没有」这个状态本身就得记住，否则重启会凭空多出会话。
pub(crate) fn build(app: &AppState) -> SessionsFile {
    let mut entries: Vec<SessionSnapshot> = app
        .session_arcs()
        .iter()
        .map(|session| snapshot(session))
        .collect();
    // 按侧边栏的次序落盘，不按 HashMap 的遍历次序：恢复后的顺序要和人眼的顺序一致。
    // 同排序位时用 id 兜底，和 session_list 的排法一模一样。
    entries.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.id.cmp(&b.id)));
    SessionsFile {
        format: SESSIONS_MAGIC.to_string(),
        version: SESSIONS_VERSION,
        counter: app.counter_value(),
        entries,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::default_document;

    /// 建号计数器必须顶到已用过的最大值之上。漏了这一步，恢复后第一个新会话
    /// 拿到的是 s1，而 s1 正是刚从盘上读回来的那个——一插就把它顶没了。
    #[test]
    fn a_restored_session_keeps_its_id_against_new_ones() {
        let dir = crate::state::scratch_config_dir();
        let state = AppState::default();
        state.point_config_dir_at(dir.clone());
        let session = state.create_session(default_document(), Some("橘猫".into()));
        let id = session.id().to_string();
        state.note_sessions_dirty();
        state.flush_sessions();
        state.drop_session(&id);

        // 同一个配置目录再起一份状态读回来：恢复必须拿到同一条 id，而不是重新编号。
        let again = AppState::default();
        again.point_config_dir_at(dir.clone());
        assert_eq!(again.restore_sessions(), 1, "要恢复出一条会话");
        let back = again.session(&id).expect("原来的 id 必须原样回来");
        assert_eq!(back.title().as_deref(), Some("橘猫"), "名字也要跟着回来");

        let fresh = again.create_session(default_document(), None);
        assert_ne!(fresh.id(), id, "新会话不能抢恢复回来这个 id");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 快照里那个画布文本丢了（序列化写不出去）的会话不能被恢复成一张空画布——
    /// 用户看到的会是「会话还在，画布没了」，比整条不见更难懂。
    #[test]
    fn a_snapshot_without_a_document_is_skipped() {
        let state = AppState::default();
        let snap = SessionSnapshot {
            id: "s1".into(),
            title: None,
            order: 1,
            model_id: "unset".into(),
            roles: Vec::new(),
            document: String::new(),
            messages: Vec::new(),
        };
        assert!(rebuild(&state, &snap).is_none(), "没有画布的会话不能恢复");
    }

    /// 文件头对不上就整份不要：硬解一个不知道长什么样的格式，
    /// 只会把一堆半截会话灌回内存，用户还以为全都还在。
    #[test]
    fn a_foreign_file_is_refused_whole() {
        let mut file = SessionsFile::blank(3);
        file.format = "someone-elses-format".into();
        assert!(!file.is_ours(), "不认识的文件头必须整份挡掉");

        file.format = SESSIONS_MAGIC.into();
        file.version = SESSIONS_VERSION + 1;
        assert!(!file.is_ours(), "同族但版本对不上，也不能当自己人硬解");
    }
}
