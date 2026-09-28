//! AIPixel 的 Tauri 外壳：应用状态托管、命令注册与事件桥。
//! 主循环在 agent-core，文档模型在 pixel-core，这里只做桌面端装配。

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
        .setup(|app| {
            let handle = app.handle().clone();
            let app_state = handle.state::<state::AppState>();
            app_state.bootstrap(&handle)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::agent_document,
            commands::agent_history,
            commands::agent_interrupt,
            commands::agent_resolve_approval,
            commands::agent_list_models,
            commands::agent_send_message,
            commands::agent_set_active,
            commands::agent_set_permission,
            commands::agent_sync_document,
            commands::aip_load,
            commands::aip_save,
            commands::aip_text,
            commands::document_png_url,
            commands::document_export,
            commands::model_remove,
            commands::model_set_active,
            commands::model_upsert,
            commands::read_image_context,
            mcp::mcp_connect,
            mcp::mcp_disconnect,
            mcp::mcp_list,
            mcp::mcp_remove,
            mcp::mcp_upsert,
            commands::session_bind_model,
            commands::session_create,
            commands::session_drop,
            commands::session_list,
            workflow::prompt_refine,
            workflow::video_probe,
            workflow::vision_brief,
            workflow::workflow_catalog,
            workflow::workflow_image_gen,
            workflow::workflow_pixelize,
            workflow::workflow_tween,
            workflow::workflow_video_frames,
            workflow::video_brief,
            editor::editor_apply_ops,
            editor::editor_fill,
            editor::editor_paint_stroke,
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
