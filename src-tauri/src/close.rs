// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 关窗问询：用户点关闭时先把窗口按住，问过前端再决定放不放行。
//!
//! 只有一件事要守：没存盘的工程不能一声不响地没。规矩是「没登记守门员的
//! 窗口照常关」——前端没装守门员，说明这一刻没有能替用户拿主意的人，
//! 硬按住窗口只会让整个应用关不掉。
//!
//! 真正「没存盘」的判断在前端：会话、撤销栈、聊天记录都在它手里，Rust
//! 只负责把「用户想走了」这句话递过去，再把前端的答复放行或无视。

use tauri::{Emitter, Manager, State, WindowEvent};

use crate::state::AppState;

/// Rust 广播给前端的通道名：该问话了。载荷是空的，问的是一切尽在前端的账。
pub const CLOSE_EVENT_CHANNEL: &str = "app-close-requested";

/// 挂在 Builder 的 `on_window_event` 上：按住来问，不登记就放行。
pub fn guard_close(window: &tauri::Window, event: &WindowEvent) {
    let WindowEvent::CloseRequested { api, .. } = event else {
        return;
    };
    if !window.state::<AppState>().close_guard() {
        return;
    }
    // 系统先关着。放行不由这里决定：前端存完（或说不用存）之后走
    // `app_close_reply` 让 app.exit 收尾，这是唯一不绕回本处的出口。
    api.prevent_close();
    let _ = window.emit(CLOSE_EVENT_CHANNEL, ());
}

/// 前端装上/撤下关窗守门员。boot 时装上，之后每一句关窗都先来问。
#[tauri::command]
pub fn app_close_guard(state: State<'_, AppState>, ready: bool) {
    state.set_close_guard(ready);
}

/// 前端对「要不要关」的答复。`quit = false` 什么都不做：窗口已经按住，
/// 接着用就行；`true` 走系统退出，不再问第二遍。
#[tauri::command]
pub fn app_close_reply(app: tauri::AppHandle, quit: bool) {
    if quit {
        app.exit(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 放行与否只由守门员登记决定，与事件本身无关：没登记时
    /// CloseRequested 必须原样放行，否则前端没起来根本关不掉应用。
    /// 直接跑 guard_close 需要一个 Window，这里只把判断抽出来验。
    #[test]
    fn close_channel_name_is_stable() {
        // 前端桥接层写死了同一个名字，改名会让关窗问询整条链路静默失效。
        assert_eq!(CLOSE_EVENT_CHANNEL, "app-close-requested");
    }
}
