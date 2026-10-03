// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! AIPixel 的 Tauri 外壳：应用状态托管、命令注册与事件桥。
//! 主循环在 agent-core，文档模型在 pixel-core，这里只做桌面端装配。
//!
//! 下面的命令清单按前端 `src/lib/bridge.ts` 的分组对齐，顺序即分组：
//! 主循环、文档与导出、模型与能力、MCP、会话、工作流、编辑器、批量。
//! 新增命令时归进既有分组，别往末尾一挂了事——清单本身就是索引。

mod close;
mod commands;
mod editor;
mod mcp;
mod state;
mod workflow;

mod batch;

use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(state::AppState::default())
        .on_window_event(close::guard_close)
        .setup(|app| {
            let handle = app.handle().clone();
            let app_state = handle.state::<state::AppState>();
            app_state.bootstrap(&handle)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // ---- 主循环：一轮对话的读写、打断、审批、激活上下文 ----
            commands::agent_document,
            commands::agent_history,
            commands::agent_interrupt,
            commands::agent_resolve_approval,
            commands::agent_list_models,
            commands::agent_loop_limits,
            commands::agent_mcp_enabled,
            commands::agent_set_mcp_enabled,
            commands::agent_send_message,
            commands::agent_set_loop_limits,
            commands::agent_set_active,
            commands::agent_set_permission,
            commands::agent_sync_document,
            // ---- 文档与导出：.aip 读写、画布快照、各格式编码 ----
            commands::aip_load,
            commands::aip_save,
            commands::aip_text,
            commands::document_png_url,
            commands::document_export,
            // ---- 模型与能力：BYOM 配置、拉清单、生图能力探测、读图 ----
            commands::model_remove,
            commands::model_set_active,
            commands::model_upsert,
            commands::model_fetch_models,
            commands::model_probe_image,
            commands::read_image_context,
            // ---- MCP：外部工具服务器的登记、连接、清单 ----
            mcp::mcp_connect,
            mcp::mcp_disconnect,
            mcp::mcp_list,
            mcp::mcp_remove,
            mcp::mcp_upsert,
            // ---- 会话：建、列、删、改名、排序、绑模型 ----
            commands::session_bind_model,
            commands::session_bind_role,
            commands::session_clear_role,
            commands::session_create,
            commands::session_drop,
            commands::session_list,
            commands::session_rename,
            commands::session_reorder,
            // ---- 工作流：目录与七条流程 + 纯本机量化 ----
            workflow::prompt_refine,
            workflow::video_probe,
            workflow::video_brief,
            workflow::vision_brief,
            workflow::workflow_catalog,
            workflow::workflow_image_gen,
            workflow::workflow_pixelize,
            workflow::workflow_tween,
            workflow::workflow_video_frames,
            // ---- 编辑器：画笔、填充、结构操作、改画布 ----
            editor::editor_apply_ops,
            editor::editor_fill,
            editor::editor_paint_stroke,
            editor::editor_resize_canvas,
            // ---- 窗口：关窗前问一声，答复由前端递回来 ----
            close::app_close_guard,
            close::app_close_reply,
            // ---- 批量：文件夹进文件夹出 + 配方簿 ----
            batch::batch_scan,
            batch::batch_run,
            batch::batch_recipe_delete,
            batch::batch_recipe_export,
            batch::batch_recipe_import,
            batch::batch_recipes_list,
            batch::batch_recipe_save,
        ])
        .run(tauri::generate_context!())
        .expect("error while running AIPixel");
}
