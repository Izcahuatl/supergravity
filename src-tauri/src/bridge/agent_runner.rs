//! Agent runner — placeholder commands. Task U3 replaces these with the real
//! agent loop, event pump, and approval plumbing.

use crate::bridge::state::AppState;

#[tauri::command]
pub async fn send_message(
    _app: tauri::AppHandle,
    _state: tauri::State<'_, AppState>,
    _conversation_id: String,
    _text: String,
) -> Result<(), String> {
    Err("not implemented".into())
}

#[tauri::command]
pub async fn cancel_agent(_state: tauri::State<'_, AppState>, _conversation_id: String) -> Result<(), String> {
    Err("not implemented".into())
}

#[tauri::command]
pub async fn resolve_approval(
    _state: tauri::State<'_, AppState>,
    _conversation_id: String,
    _request_id: String,
    _allow: bool,
) -> Result<(), String> {
    Err("not implemented".into())
}
