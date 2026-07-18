pub mod agent_runner;
pub mod commands;
pub mod state;

use crate::core::config::{data_dir, AppConfig};
use crate::core::store::Store;
use tauri::Manager;

pub fn run() {
    let dir = data_dir().expect("cannot determine app data dir");
    std::fs::create_dir_all(&dir).expect("cannot create app data dir");
    let store = Store::open(&dir.join("supergravity.db")).expect("cannot open store");
    let config_path = dir.join("config.toml");
    let ui_config = AppConfig::load(&config_path).unwrap_or_default();
    let state = state::AppState::production(store, config_path, ui_config);

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::list_workspaces,
            commands::add_workspace,
            commands::remove_workspace,
            commands::list_conversations,
            commands::create_conversation,
            commands::rename_conversation,
            commands::delete_conversation,
            commands::get_messages,
            commands::set_approval_mode,
            commands::update_conversation_model,
            commands::list_providers,
            commands::upsert_provider,
            commands::delete_provider,
            commands::set_api_key,
            commands::delete_api_key,
            commands::get_initial_state,
            commands::set_ui_state,
            agent_runner::send_message,
            agent_runner::cancel_agent,
            agent_runner::resolve_approval,
        ])
        .setup(|app| {
            let state = app.state::<state::AppState>();
            let handle = tauri::async_runtime::handle();
            let inner = state.inner();
            // Preset seeding needs an async context; block briefly at startup.
            handle.block_on(async {
                commands::seed_presets_if_empty_impl(inner)
                    .await
                    .expect("seed presets");
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running supergravity");
}
