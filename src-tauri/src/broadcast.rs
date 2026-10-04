// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only

//! 画布广播的突发合并。
//!
//! 画布一改就要往前端推一份 `document_updated`。改的人有三类：用户点编辑器、
//! 主循环里的模型回合、以及外部经 MCP 连发工具的客户端。前两种天然稀疏
//! （一次点击、一次工具调用隔着好几秒），第三种会在一个回合里连发几十个工具：
//! 每个工具都广播一次，webview 就被逐个叫醒几十回。
//!
//! 每一回醒来都不是免费的：解析增量、`setState`、重画帧条和画布、再往后端要
//! 一张 PNG，而 macOS 的 WKWebView IPC 是串行的，队列一堵界面就一格一格跳。
//! 所以这里给广播加一道闸门：窗口内只放行第一发和最后一发，中间的全合并掉。
//!
//! 合并不丢数据：patch 是「相对上一次广播」的增量，而不是「第 N 版全量」，
//! 最后一发自然带着中间所有改动。见 `AgentSession::document_patch`。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// 合并窗口。取 48ms 是照着两次广播之间人眼能察觉的下限来的：再小合并不掉
/// 一个回合里的连发，再大用户点一下画笔就能感到黏手。
const WINDOW_MS: u64 = 48;

/// 进程启动时刻。拿单调时钟而不是 `SystemTime`：用户改系统时间会让窗口算成
/// 负数或者巨值，虽然 `saturating_sub` 兜得住，但墙上的钟本就不该管这种事。
static START: OnceLock<Instant> = OnceLock::new();

/// 从 1 起算，不是从 0：`last_sent_ms` 拿 0 当「从来没发过」的哨兵，而进程刚
/// 起来那一毫秒里 elapsed() 正好也是 0，两个含义撞在一起就会把连发当成首发。
fn now_ms() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

/// 一个会话的广播闸门。字段全是原子的：emit 可能从命令线程、tokio 任务、
/// MCP 连接任务里任意一个进来，用锁会把它们串成一条队。
struct Gate {
    /// 上一次真正发出去的时刻（进程启动以来的毫秒）。0 表示「从来没发过」。
    last_sent_ms: AtomicU64,
    /// 是否已经排定了窗口尾巴上的那一发补发。同一波突发只排一次。
    scheduled: AtomicBool,
}

impl Gate {
    fn new() -> Self {
        Gate {
            last_sent_ms: AtomicU64::new(0),
            scheduled: AtomicBool::new(false),
        }
    }
}

/// 会话 id -> 闸门。放静态表而不是塞进 `AgentSession`：那边住在 agent-core 里，
/// 不该知道 Tauri 的事件总线；而 emit 那一侧只拿得到会话 id。
static GATES: OnceLock<Mutex<HashMap<String, Arc<Gate>>>> = OnceLock::new();

fn gate_for(session_id: &str) -> Arc<Gate> {
    let table = GATES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut table = table
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    table
        .entry(session_id.to_string())
        .or_insert_with(|| Arc::new(Gate::new()))
        .clone()
}

/// 会话被删掉时收尸。闸门本身没有后台任务，漏了也只是 HashMap 里多一个空壳；
/// 但会话簿是用户随手增删的东西，攒着攒着就是一份不该有的常驻内存。
pub fn release(session_id: &str) {
    if let Some(table) = GATES.get() {
        table
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(session_id);
    }
}

/// 发一发广播，必要时先合并。
///
/// `send` 里做真正的那一次 emit。它必须自带「此刻该发什么」的全部信息——
/// 调用方在闭包里重新算一遍，而不是把提前算好的结果递进来：合并的意义就在于
/// 最后一发带着截至那一刻的全部改动，提前算好的东西到那时已经旧了。
pub fn coalesced<F>(session_id: &str, send: F)
where
    F: FnOnce() + Send + 'static,
{
    let gate = gate_for(session_id);
    // 冷手（用户点一下）或者上一发已经过去一整窗口：立刻发，低延迟优先。
    // 这一条同时保证了「用户操作永远不等」——人手两次点击之间远超一个窗口。
    //
    // 0 是「从来没发过」的哨兵（见 now_ms）：头一发永远立刻走，不因为刚开
    // 进程、窗口还没铺开就被压后一小会儿。用户点一下画笔就得马上看到响应。
    let last_sent = gate.last_sent_ms.load(Ordering::Relaxed);
    if last_sent == 0 || now_ms().saturating_sub(last_sent) >= WINDOW_MS {
        send_now(&gate, send);
        return;
    }
    // 已经排了一发尾巴：后来的都搭那一发的车，不再另起定时器。
    if gate.scheduled.swap(true, Ordering::SeqCst) {
        return;
    }
    let gate = Arc::clone(&gate);
    tauri::async_runtime::spawn(async move {
        // 睡到窗口尾巴。这一觉里到的改动全被最后一发带上，中间那几发连
        // patch 都不用算——省下的正是后端栅格化和前端重画。
        tokio::time::sleep(Duration::from_millis(WINDOW_MS)).await;
        // 先撤牌子再发：万一发的过程中又进来一发，它看到的是「没排过」，
        // 于是自己排下一个尾巴，不会和这一发叠在一起。
        gate.scheduled.store(false, Ordering::SeqCst);
        send_now(&gate, send);
    });
}

fn send_now<F: FnOnce()>(gate: &Gate, send: F) {
    gate.last_sent_ms.store(now_ms(), Ordering::Relaxed);
    send();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// 一波连发只该放行两发：第一发立刻走，剩下的合成窗口尾巴上那一发。
    #[test]
    fn a_burst_collapses_into_a_leading_and_a_trailing_send() {
        let hits = Arc::new(AtomicUsize::new(0));
        let id = "burst-session";
        release(id);
        for _ in 0..6 {
            let hits = Arc::clone(&hits);
            coalesced(id, move || {
                hits.fetch_add(1, Ordering::SeqCst);
            });
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1, "窗口内只放行第一发");
        // 等尾巴那一发落地：窗口 48ms，给足余量。
        std::thread::sleep(Duration::from_millis(220));
        assert_eq!(hits.load(Ordering::SeqCst), 2, "尾巴上补一发");
        release(id);
    }

    /// 稀疏的调用一发都不该被合并：用户点编辑器、模型隔几秒调一次工具，
    /// 这些场景要的是低延迟，合并它们纯属帮倒忙。
    #[test]
    fn sparse_calls_are_never_merged() {
        let hits = Arc::new(AtomicUsize::new(0));
        let id = "sparse-session";
        release(id);
        for _ in 0..3 {
            let hits = Arc::clone(&hits);
            coalesced(id, move || {
                hits.fetch_add(1, Ordering::SeqCst);
            });
            std::thread::sleep(Duration::from_millis(90));
        }
        assert_eq!(hits.load(Ordering::SeqCst), 3);
        release(id);
    }

    /// 会话删掉之后闸门跟着消失：下一个同名会话从冷手开始，不继承上一个的窗口。
    #[test]
    fn releasing_forgets_the_gate() {
        let hits = Arc::new(AtomicUsize::new(0));
        let id = "released-session";
        release(id);
        let first = Arc::clone(&hits);
        coalesced(id, move || {
            first.fetch_add(1, Ordering::SeqCst);
        });
        release(id);
        let second = Arc::clone(&hits);
        coalesced(id, move || {
            second.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(hits.load(Ordering::SeqCst), 2, "撤了闸门就不该再合并");
        release(id);
    }
}
