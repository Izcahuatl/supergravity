use crate::bridge::state::AppState;
use crate::core::config::AppConfig;
use crate::core::error::{Error, Result};
use crate::core::store::{ConversationRow, WorkspaceRow};
use crate::core::types::ProviderConfig;
use serde::Serialize;
use std::path::Path;

/// Run a sync core call off the async runtime (Store/KeyStore contract).
pub async fn block<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| crate::core::error::Error::Tool(format!("task join: {e}")))?
}

/// Commands return String errors to the frontend.
pub fn estr(e: crate::core::error::Error) -> String {
    e.to_string()
}

pub async fn list_workspaces_impl(state: &AppState) -> Result<Vec<WorkspaceRow>> {
    let s = state.store.clone();
    block(move || s.list_workspaces()).await
}

pub async fn add_workspace_impl(state: &AppState, name: String, path: String) -> Result<String> {
    let p = Path::new(&path);
    if !p.is_absolute() {
        return Err(Error::Tool("workspace path must be absolute".into()));
    }
    if !p.is_dir() {
        return Err(Error::Tool(format!("not a directory: {path}")));
    }
    let s = state.store.clone();
    block(move || s.add_workspace(&name, &path)).await
}

pub async fn remove_workspace_impl(state: &AppState, id: String) -> Result<()> {
    let convs = {
        let s = state.store.clone();
        let wid = id.clone();
        block(move || s.list_conversations(&wid)).await?
    };
    {
        let agents = state.agents.lock().unwrap();
        for c in &convs {
            if let Some(agent) = agents.get(&c.id) {
                agent.cancel.cancel();
            }
        }
    }
    let s = state.store.clone();
    block(move || s.remove_workspace(&id)).await
}

pub async fn list_conversations_impl(
    state: &AppState,
    workspace_id: String,
) -> Result<Vec<ConversationRow>> {
    let s = state.store.clone();
    block(move || s.list_conversations(&workspace_id)).await
}

pub async fn create_conversation_impl(
    state: &AppState,
    workspace_id: String,
    title: String,
    provider_id: String,
    model: String,
) -> Result<String> {
    let mode = match state
        .ui_config
        .lock()
        .unwrap()
        .default_approval_mode
        .as_deref()
    {
        Some("auto") => crate::core::types::ApprovalMode::Auto,
        _ => crate::core::types::ApprovalMode::Manual,
    };
    let s = state.store.clone();
    block(move || {
        s.create_conversation(&workspace_id, &title, &provider_id, &model, mode)
    })
    .await
}

pub async fn rename_conversation_impl(state: &AppState, id: String, title: String) -> Result<()> {
    let s = state.store.clone();
    block(move || s.rename_conversation(&id, &title)).await
}

pub async fn delete_conversation_impl(state: &AppState, id: String) -> Result<()> {
    if let Some(agent) = state.agents.lock().unwrap().get(&id) {
        agent.cancel.cancel();
    }
    // Remove the conversation's scratch dir (best-effort).
    if state.config_path.is_absolute() {
        if let Some(parent) = state.config_path.parent() {
            let _ = std::fs::remove_dir_all(parent.join("workshops").join(&id));
        }
    }
    let s = state.store.clone();
    block(move || s.delete_conversation(&id)).await
}

pub async fn get_messages_impl(state: &AppState, conversation_id: String) -> Result<Vec<crate::core::store::MessageRow>> {
    let s = state.store.clone();
    block(move || s.get_message_rows(&conversation_id)).await
}

/// Fuzzy file search for the composer's @ autocomplete. Returns up to 50
/// workspace-relative paths (forward slashes), skipping noise directories.
pub async fn search_workspace_files_impl(
    state: &AppState,
    workspace_id: String,
    query: String,
) -> Result<Vec<String>> {
    const IGNORED: [&str; 4] = [".git", "target", "node_modules", ".idea"];
    const MAX_VISITED: usize = 5000;
    const MAX_RESULTS: usize = 50;
    let s = state.store.clone();
    block(move || {
        let ws = s.get_workspace(&workspace_id)?;
        let root = std::path::PathBuf::from(&ws.path);
        let mut paths: Vec<String> = Vec::new();
        let mut stack = vec![root.clone()];
        let mut visited = 0;
        while let Some(dir) = stack.pop() {
            if visited >= MAX_VISITED {
                break;
            }
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for entry in rd.flatten() {
                visited += 1;
                if visited >= MAX_VISITED {
                    break;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                if is_dir {
                    if !IGNORED.contains(&name.as_str()) && !name.starts_with('.') {
                        stack.push(entry.path());
                    }
                    continue;
                }
                if let Ok(rel) = entry.path().strip_prefix(&root) {
                    paths.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        let q = query.to_lowercase();
        let mut matches: Vec<(u8, String)> = paths
            .into_iter()
            .filter_map(|p| {
                let lower = p.to_lowercase();
                if q.is_empty() {
                    Some((1, p))
                } else if lower.starts_with(&q) || lower.rsplit('/').next().unwrap_or("").starts_with(&q) {
                    Some((0, p))
                } else if lower.contains(&q) {
                    Some((1, p))
                } else {
                    None
                }
            })
            .collect();
        matches.sort();
        matches.truncate(MAX_RESULTS);
        Ok(matches.into_iter().map(|(_, p)| p).collect())
    })
    .await
}

/// Per-file revert: restore one file to its checkpoint from the given turn,
/// consuming that file's backups for the turn. Error when no checkpoint exists.
pub async fn revert_file_impl(
    state: &AppState,
    conversation_id: String,
    path: String,
    after_message_id: i64,
) -> Result<()> {
    let s = state.store.clone();
    block(move || {
        let conv = s.get_conversation(&conversation_id)?;
        let ws = s.get_workspace(&conv.workspace_id)?;
        let root = std::path::PathBuf::from(&ws.path);
        let backup = s.file_backup_for(&conversation_id, &path, after_message_id)?;
        let Some(content) = backup else {
            return Err(Error::Tool(format!(
                "no checkpoint for {path} at this turn (already reverted or never changed)"
            )));
        };
        let abs = crate::core::tools::resolve_in_workspace(&root, &path)?;
        match content {
            Some(bytes) => {
                if let Some(parent) = abs.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&abs, bytes)?;
            }
            None => {
                if abs.exists() {
                    std::fs::remove_file(&abs)?;
                }
            }
        }
        s.delete_file_backups_for(&conversation_id, &path, after_message_id)
    })
    .await
}

/// Simulated result of a mutating tool call, for the approval card's diff
/// preview. Never touches disk. Returns None for non-mutating tools or args
/// that don't parse; tool-level errors (missing file, old_string not found)
/// come back as Err so the card can show them inline.
#[derive(Serialize)]
pub struct ToolDiffPreview {
    pub path: String,
    pub old: String,
    #[serde(rename = "new")]
    pub new_: String,
}

pub async fn preview_tool_diff_impl(    state: &AppState,
    conversation_id: String,
    name: String,
    args_json: String,
) -> Result<Option<ToolDiffPreview>> {
    if !matches!(name.as_str(), "write_file" | "edit_file") {
        return Ok(None);
    }
    let s = state.store.clone();
    block(move || {
        let conv = s.get_conversation(&conversation_id)?;
        let ws = s.get_workspace(&conv.workspace_id)?;
        let root = std::path::PathBuf::from(&ws.path);
        let args: serde_json::Value = serde_json::from_str(&args_json)
            .map_err(|e| Error::Tool(format!("bad args: {e}")))?;
        let path = args
            .get("path")
            .and_then(|p| p.as_str())
            .ok_or_else(|| Error::Tool("missing path".into()))?;
        let abs = crate::core::tools::resolve_in_workspace(&root, path)?;
        let old = std::fs::read_to_string(&abs).unwrap_or_default();
        let new = match name.as_str() {
            "write_file" => {
                let content = args
                    .get("content")
                    .and_then(|c| c.as_str())
                    .ok_or_else(|| Error::Tool("missing content".into()))?;
                match args.get("mode").and_then(|m| m.as_str()).unwrap_or("overwrite") {
                    "append" => format!("{old}{content}"),
                    _ => content.to_string(),
                }
            }
            _ => {
                // edit_file: mirror EditFileTool's replacement semantics.
                let old_string = args
                    .get("old_string")
                    .and_then(|c| c.as_str())
                    .ok_or_else(|| Error::Tool("missing old_string".into()))?;
                let new_string = args
                    .get("new_string")
                    .and_then(|c| c.as_str())
                    .unwrap_or("");
                if old_string.is_empty() {
                    return Err(Error::Tool("old_string must not be empty".into()));
                }
                if abs.metadata().is_err() {
                    return Err(Error::Tool(format!("file does not exist: {path}")));
                }
                let count = old.matches(old_string).count();
                let expected = args
                    .get("expected_replacements")
                    .and_then(|e| e.as_u64())
                    .unwrap_or(1) as usize;
                if count == 0 {
                    return Err(Error::Tool("old_string not found".into()));
                }
                if count != expected {
                    return Err(Error::Tool(format!(
                        "old_string occurs {count} times, expected {expected}"
                    )));
                }
                old.replacen(old_string, new_string, expected)
            }
        };
        Ok(Some(ToolDiffPreview {
            path: path.to_string(),
            old,
            new_: new,
        }))
    })
    .await
}

pub async fn rewind_conversation_impl(
    state: &AppState,
    conversation_id: String,
    message_id: i64,
) -> Result<()> {    // Never rewrite history under a live agent.
    if let Some(agent) = state.agents.lock().unwrap().get(&conversation_id) {
        agent.cancel.cancel();
    }
    let s = state.store.clone();
    block(move || {
        // Restore checkpointed files first (newest change last-write-wins, so
        // iterate newest→oldest and let the oldest original land last).
        let conv = s.get_conversation(&conversation_id)?;
        let ws = s.get_workspace(&conv.workspace_id)?;
        let root = std::path::PathBuf::from(&ws.path);
        for (path, content) in s.file_backups_from(&conversation_id, message_id)? {
            let abs = crate::core::tools::resolve_in_workspace(&root, &path)?;
            match content {
                Some(bytes) => {
                    if let Some(parent) = abs.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&abs, bytes)?;
                }
                // The file did not exist before this turn — remove it.
                None => {
                    if abs.exists() {
                        std::fs::remove_file(&abs)?;
                    }
                }
            }
        }
        s.delete_file_backups_from(&conversation_id, message_id)?;
        s.rewind_messages(&conversation_id, message_id)
    })
    .await
}

pub async fn set_approval_mode_impl(
    state: &AppState,
    conversation_id: String,
    mode: String,
) -> Result<()> {
    let mode = match mode.as_str() {
        "auto" => crate::core::types::ApprovalMode::Auto,
        "manual" => crate::core::types::ApprovalMode::Manual,
        other => return Err(Error::Tool(format!("unknown approval mode: {other}"))),
    };
    if let Some(agent) = state.agents.lock().unwrap().get(&conversation_id) {
        agent.broker.set_mode(mode);
    }
    let s = state.store.clone();
    block(move || s.set_approval_mode(&conversation_id, mode)).await
}

pub async fn update_conversation_model_impl(
    state: &AppState,
    conversation_id: String,
    provider_id: String,
    model: String,
) -> Result<()> {
    let s = state.store.clone();
    block(move || s.update_conversation_model(&conversation_id, &provider_id, &model)).await
}

pub async fn list_providers_impl(state: &AppState) -> Result<Vec<ProviderConfig>> {
    let s = state.store.clone();
    block(move || s.list_providers()).await
}

pub async fn upsert_provider_impl(state: &AppState, cfg: ProviderConfig) -> Result<()> {
    let s = state.store.clone();
    block(move || s.upsert_provider(&cfg)).await
}

pub async fn delete_provider_impl(state: &AppState, id: String) -> Result<()> {
    let k = state.keys.clone();
    let kid = id.clone();
    let _ = block(move || k.delete(&kid)).await;
    let s = state.store.clone();
    block(move || s.delete_provider(&id)).await
}

pub async fn set_api_key_impl(state: &AppState, provider_id: String, key: String) -> Result<()> {
    // Validate the provider exists BEFORE writing the keychain (no orphan entries).
    let mut cfg = {
        let s = state.store.clone();
        let pid = provider_id.clone();
        block(move || s.get_provider(&pid)).await?
    };
    {
        let k = state.keys.clone();
        let pid = provider_id.clone();
        block(move || k.set(&pid, &key)).await?;
    }
    cfg.has_key = true;
    let s = state.store.clone();
    block(move || s.upsert_provider(&cfg)).await
}

pub async fn delete_api_key_impl(state: &AppState, provider_id: String) -> Result<()> {
    let mut cfg = {
        let s = state.store.clone();
        let pid = provider_id.clone();
        block(move || s.get_provider(&pid)).await?
    };
    {
        let k = state.keys.clone();
        let pid = provider_id.clone();
        block(move || k.delete(&pid)).await?;
    }
    cfg.has_key = false;
    let s = state.store.clone();
    block(move || s.upsert_provider(&cfg)).await
}

/// Model names from a local Ollama server (`GET {base}/api/tags`).
pub async fn list_local_models_impl(state: &AppState, provider_id: String) -> Result<Vec<String>> {
    let cfg = {
        let s = state.store.clone();
        block(move || s.get_provider(&provider_id)).await?
    };
    if cfg.kind != crate::core::types::ProviderKind::Ollama {
        return Err(Error::Tool(
            "model listing is only supported for Ollama".into(),
        ));
    }
    let base = cfg
        .base_url
        .clone()
        .unwrap_or_else(|| "http://localhost:11434".into());
    let url = format!("{}/api/tags", base.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(Error::Http)?;
    let resp = client.get(&url).send().await.map_err(Error::Http)?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(Error::Provider {
            status,
            body: body.chars().take(200).collect(),
        });
    }
    let body: serde_json::Value = resp.json().await.map_err(Error::Http)?;
    Ok(parse_tags(&body))
}

fn parse_tags(body: &serde_json::Value) -> Vec<String> {
    body["models"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m["name"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Insert the four provider presets on first run (providers table empty).
pub async fn seed_presets_if_empty_impl(state: &AppState) -> Result<()> {
    let existing = {
        let s = state.store.clone();
        block(move || s.list_providers()).await?
    };
    if !existing.is_empty() {
        return Ok(());
    }
    for preset in crate::core::providers::presets() {
        let s = state.store.clone();
        block(move || s.upsert_provider(&preset)).await?;
    }
    Ok(())
}

/// Everything the UI needs at boot.
#[derive(Serialize)]
pub struct InitialState {
    pub config: AppConfig,
    pub workspaces: Vec<WorkspaceRow>,
    pub providers: Vec<ProviderConfig>,
    /// Root of per-conversation Workshop dirs (for workshop-aware tool labels).
    pub workshops_dir: String,
}

pub async fn get_initial_state_impl(state: &AppState) -> Result<InitialState> {
    let workspaces = list_workspaces_impl(state).await?;
    let providers = list_providers_impl(state).await?;
    let config = state.ui_config.lock().unwrap().clone();
    let workshops_dir = state
        .config_path
        .parent()
        .map(|p| p.join("workshops").to_string_lossy().to_string())
        .unwrap_or_default();
    Ok(InitialState {
        config,
        workspaces,
        providers,
        workshops_dir,
    })
}

pub async fn set_ui_state_impl(
    state: &AppState,
    last_workspace_id: Option<String>,
    last_conversation_id: Option<String>,
) -> Result<()> {
    let cfg = {
        let mut guard = state.ui_config.lock().unwrap();
        guard.last_workspace_id = last_workspace_id;
        guard.last_conversation_id = last_conversation_id;
        guard.clone()
    };
    let path = state.config_path.clone();
    tauri::async_runtime::spawn_blocking(move || cfg.save(&path))
        .await
        .map_err(|e| Error::Tool(format!("task join: {e}")))?
}

/// Merge-only update of app preferences (None leaves the field unchanged).
pub async fn set_app_prefs_impl(
    state: &AppState,
    default_approval_mode: Option<String>,
    notifications_enabled: Option<bool>,
    external_policy: Option<String>,
    workshop_full_access: Option<bool>,
    project_files_no_ask: Option<bool>,
    project_shell_no_ask: Option<bool>,
) -> Result<()> {
    let cfg = {
        let mut guard = state.ui_config.lock().unwrap();
        if let Some(m) = default_approval_mode {
            guard.default_approval_mode = Some(m);
        }
        if let Some(n) = notifications_enabled {
            guard.notifications_enabled = Some(n);
        }
        if let Some(p) = external_policy {
            guard.external_policy = Some(p);
        }
        if let Some(w) = workshop_full_access {
            guard.workshop_full_access = Some(w);
        }
        if let Some(f) = project_files_no_ask {
            guard.project_files_no_ask = Some(f);
        }
        if let Some(s) = project_shell_no_ask {
            guard.project_shell_no_ask = Some(s);
        }
        guard.clone()
    };
    let path = state.config_path.clone();
    tauri::async_runtime::spawn_blocking(move || cfg.save(&path))
        .await
        .map_err(|e| Error::Tool(format!("task join: {e}")))?
}

#[tauri::command]
pub async fn list_workspaces(
    state: tauri::State<'_, AppState>,
) -> std::result::Result<Vec<WorkspaceRow>, String> {
    list_workspaces_impl(&state).await.map_err(estr)
}

#[tauri::command]
pub async fn add_workspace(
    state: tauri::State<'_, AppState>,
    name: String,
    path: String,
) -> std::result::Result<String, String> {
    add_workspace_impl(&state, name, path).await.map_err(estr)
}

#[tauri::command]
pub async fn remove_workspace(
    state: tauri::State<'_, AppState>,
    id: String,
) -> std::result::Result<(), String> {
    remove_workspace_impl(&state, id).await.map_err(estr)
}

#[tauri::command]
pub async fn list_conversations(
    state: tauri::State<'_, AppState>,
    workspace_id: String,
) -> std::result::Result<Vec<ConversationRow>, String> {
    list_conversations_impl(&state, workspace_id)
        .await
        .map_err(estr)
}

#[tauri::command]
pub async fn create_conversation(
    state: tauri::State<'_, AppState>,
    workspace_id: String,
    title: String,
    provider_id: String,
    model: String,
) -> std::result::Result<String, String> {
    create_conversation_impl(&state, workspace_id, title, provider_id, model)
        .await
        .map_err(estr)
}

#[tauri::command]
pub async fn rename_conversation(
    state: tauri::State<'_, AppState>,
    id: String,
    title: String,
) -> std::result::Result<(), String> {
    rename_conversation_impl(&state, id, title)
        .await
        .map_err(estr)
}

#[tauri::command]
pub async fn delete_conversation(
    state: tauri::State<'_, AppState>,
    id: String,
) -> std::result::Result<(), String> {
    delete_conversation_impl(&state, id).await.map_err(estr)
}

#[tauri::command]
pub async fn get_messages(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> std::result::Result<Vec<crate::core::store::MessageRow>, String> {
    get_messages_impl(&state, conversation_id)
        .await
        .map_err(estr)
}

#[tauri::command]
pub async fn rewind_conversation(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    message_id: i64,
) -> std::result::Result<(), String> {
    rewind_conversation_impl(&state, conversation_id, message_id)
        .await
        .map_err(estr)
}

#[tauri::command]
pub async fn search_workspace_files(
    state: tauri::State<'_, AppState>,
    workspace_id: String,
    query: String,
) -> std::result::Result<Vec<String>, String> {
    search_workspace_files_impl(&state, workspace_id, query)
        .await
        .map_err(estr)
}

#[tauri::command]
pub async fn preview_tool_diff(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    name: String,
    args_json: String,
) -> std::result::Result<Option<ToolDiffPreview>, String> {
    preview_tool_diff_impl(&state, conversation_id, name, args_json)
        .await
        .map_err(estr)
}

#[tauri::command]
pub async fn revert_file(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    path: String,
    after_message_id: i64,
) -> std::result::Result<(), String> {
    revert_file_impl(&state, conversation_id, path, after_message_id)
        .await
        .map_err(estr)
}

#[tauri::command]
pub async fn set_approval_mode(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    mode: String,
) -> std::result::Result<(), String> {
    set_approval_mode_impl(&state, conversation_id, mode)
        .await
        .map_err(estr)
}

#[tauri::command]
pub async fn update_conversation_model(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    provider_id: String,
    model: String,
) -> std::result::Result<(), String> {
    update_conversation_model_impl(&state, conversation_id, provider_id, model)
        .await
        .map_err(estr)
}

#[tauri::command]
pub async fn list_providers(
    state: tauri::State<'_, AppState>,
) -> std::result::Result<Vec<ProviderConfig>, String> {
    list_providers_impl(&state).await.map_err(estr)
}

#[tauri::command]
pub async fn upsert_provider(
    state: tauri::State<'_, AppState>,
    cfg: ProviderConfig,
) -> std::result::Result<(), String> {
    upsert_provider_impl(&state, cfg).await.map_err(estr)
}

#[tauri::command]
pub async fn delete_provider(
    state: tauri::State<'_, AppState>,
    id: String,
) -> std::result::Result<(), String> {
    delete_provider_impl(&state, id).await.map_err(estr)
}

#[tauri::command]
pub async fn set_api_key(
    state: tauri::State<'_, AppState>,
    provider_id: String,
    key: String,
) -> std::result::Result<(), String> {
    set_api_key_impl(&state, provider_id, key)
        .await
        .map_err(estr)
}

#[tauri::command]
pub async fn delete_api_key(
    state: tauri::State<'_, AppState>,
    provider_id: String,
) -> std::result::Result<(), String> {
    delete_api_key_impl(&state, provider_id).await.map_err(estr)
}

#[tauri::command]
pub async fn list_local_models(
    state: tauri::State<'_, AppState>,
    provider_id: String,
) -> std::result::Result<Vec<String>, String> {
    list_local_models_impl(&state, provider_id)
        .await
        .map_err(estr)
}

#[tauri::command]
pub async fn get_initial_state(
    state: tauri::State<'_, AppState>,
) -> std::result::Result<InitialState, String> {
    get_initial_state_impl(&state).await.map_err(estr)
}

#[tauri::command]
pub async fn set_ui_state(
    state: tauri::State<'_, AppState>,
    last_workspace_id: Option<String>,
    last_conversation_id: Option<String>,
) -> std::result::Result<(), String> {
    set_ui_state_impl(&state, last_workspace_id, last_conversation_id)
        .await
        .map_err(estr)
}

#[tauri::command]
pub async fn set_app_prefs(
    state: tauri::State<'_, AppState>,
    default_approval_mode: Option<String>,
    notifications_enabled: Option<bool>,
    external_policy: Option<String>,
    workshop_full_access: Option<bool>,
    project_files_no_ask: Option<bool>,
    project_shell_no_ask: Option<bool>,
) -> std::result::Result<(), String> {
    set_app_prefs_impl(
        &state,
        default_approval_mode,
        notifications_enabled,
        external_policy,
        workshop_full_access,
        project_files_no_ask,
        project_shell_no_ask,
    )
    .await
    .map_err(estr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::state::AppState;
    use crate::core::types::{ApprovalMode, Message, ProviderKind, Role};

    #[tokio::test]
    async fn workspace_and_conversation_crud() {
        let state = AppState::test();
        let dir = tempfile::tempdir().unwrap();
        let ws = add_workspace_impl(
            &state,
            "proj".into(),
            dir.path().to_string_lossy().to_string(),
        )
        .await
        .unwrap();
        assert_eq!(list_workspaces_impl(&state).await.unwrap().len(), 1);
        let cid = create_conversation_impl(
            &state,
            ws.clone(),
            "New Conversation".into(),
            "ollama".into(),
            "qwen3".into(),
        )
        .await
        .unwrap();
        rename_conversation_impl(&state, cid.clone(), "Fix bug".into())
            .await
            .unwrap();
        set_approval_mode_impl(&state, cid.clone(), "auto".into())
            .await
            .unwrap();
        let convs = list_conversations_impl(&state, ws.clone()).await.unwrap();
        assert_eq!(convs.len(), 1);
        assert_eq!(convs[0].title, "Fix bug");
        assert_eq!(convs[0].approval_mode, ApprovalMode::Auto);
        delete_conversation_impl(&state, cid).await.unwrap();
        assert!(list_conversations_impl(&state, ws)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn search_workspace_files_filters_and_caps() {
        let state = AppState::test();
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::create_dir_all(dir.path().join("target/debug")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main(){}").unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "// lib").unwrap();
        std::fs::write(dir.path().join("notes.md"), "hi").unwrap();
        std::fs::write(dir.path().join("target/debug/build.o"), "obj").unwrap();
        let ws = state
            .store
            .add_workspace("proj", &dir.path().to_string_lossy())
            .unwrap();
        let cid = state
            .store
            .create_conversation(&ws, "c", "mock", "m", ApprovalMode::Auto)
            .unwrap();
        let all = search_workspace_files_impl(&state, ws.clone(), String::new())
            .await
            .unwrap();
        assert!(all.contains(&"src/main.rs".to_string()), "{all:?}");
        assert!(!all.iter().any(|p| p.starts_with("target/")), "{all:?}");
        let filtered = search_workspace_files_impl(&state, ws.clone(), "lib".into())
            .await
            .unwrap();
        assert_eq!(filtered, vec!["src/lib.rs".to_string()]);
        let _ = cid; // conversation unused by search (workspace-scoped)
    }

    #[tokio::test]
    async fn preview_tool_diff_write_and_edit() {
        let state = AppState::test();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
        let ws = state
            .store
            .add_workspace("proj", &dir.path().to_string_lossy())
            .unwrap();
        let cid = state
            .store
            .create_conversation(&ws, "c", "mock", "m", ApprovalMode::Auto)
            .unwrap();
        // write_file overwrite
        let p = preview_tool_diff_impl(
            &state,
            cid.clone(),
            "write_file".into(),
            r#"{"path":"a.txt","content":"three"}"#.into(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(p.old, "one\ntwo\n");
        assert_eq!(p.new_, "three");
        // write_file append
        let p = preview_tool_diff_impl(
            &state,
            cid.clone(),
            "write_file".into(),
            r#"{"path":"a.txt","content":"X","mode":"append"}"#.into(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(p.new_, "one\ntwo\nX");
        // edit_file ok — and nothing written to disk
        let p = preview_tool_diff_impl(
            &state,
            cid.clone(),
            "edit_file".into(),
            r#"{"path":"a.txt","old_string":"two","new_string":"TWO"}"#.into(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(p.new_, "one\nTWO\n");
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "one\ntwo\n");
        // edit_file missing old_string → error
        assert!(preview_tool_diff_impl(
            &state,
            cid.clone(),
            "edit_file".into(),
            r#"{"path":"a.txt","old_string":"nope","new_string":"x"}"#.into(),
        )
        .await
        .is_err());
        // read_file → None
        assert!(preview_tool_diff_impl(
            &state,
            cid,
            "read_file".into(),
            r#"{"path":"a.txt"}"#.into(),
        )
        .await
        .unwrap()
        .is_none());
    }

    #[tokio::test]
    async fn rewind_restores_files_and_history() {
        let state = AppState::test();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("keep.txt"), "v1").unwrap();
        let ws = state
            .store
            .add_workspace("proj", &dir.path().to_string_lossy())
            .unwrap();
        let cid = state
            .store
            .create_conversation(&ws, "c", "mock", "m", ApprovalMode::Auto)
            .unwrap();
        state
            .store
            .append_message(&cid, &Message::text(Role::User, "turn1"))
            .unwrap();
        let m1 = state.store.last_message_id(&cid).unwrap();
        state
            .store
            .append_message(&cid, &Message::text(Role::Assistant, "a1"))
            .unwrap();
        // Turn-1 run: keep.txt overwritten, made.txt created (backups recorded).
        state
            .store
            .add_file_backup(&cid, m1, "keep.txt", Some(b"v1"))
            .unwrap();
        state.store.add_file_backup(&cid, m1, "made.txt", None).unwrap();
        std::fs::write(dir.path().join("keep.txt"), "v2").unwrap();
        std::fs::write(dir.path().join("made.txt"), "new").unwrap();
        state
            .store
            .append_message(&cid, &Message::text(Role::User, "turn2"))
            .unwrap();
        let m2 = state.store.last_message_id(&cid).unwrap();
        state
            .store
            .append_message(&cid, &Message::text(Role::Assistant, "a2"))
            .unwrap();

        // Rewind to turn2: nothing to restore, history trimmed to 2 messages.
        rewind_conversation_impl(&state, cid.clone(), m2)
            .await
            .unwrap();
        assert_eq!(state.store.get_message_rows(&cid).unwrap().len(), 2);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("keep.txt")).unwrap(),
            "v2"
        );

        // Rewind to turn1: keep.txt → v1, made.txt removed, history empty,
        // backups consumed.
        rewind_conversation_impl(&state, cid.clone(), m1)
            .await
            .unwrap();
        assert_eq!(state.store.get_message_rows(&cid).unwrap().len(), 0);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("keep.txt")).unwrap(),
            "v1"
        );
        assert!(!dir.path().join("made.txt").exists());
        assert!(state.store.file_backups_from(&cid, 0).unwrap().is_empty());
    }

    #[tokio::test]
    async fn add_workspace_rejects_nonexistent_or_relative() {
        let state = AppState::test();
        assert!(
            add_workspace_impl(&state, "x".into(), "relative/path".into())
                .await
                .is_err()
        );
        assert!(add_workspace_impl(
            &state,
            "x".into(),
            "C:\\definitely\\not\\here\\12345".into()
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn provider_lifecycle_with_keys() {
        let state = AppState::test();
        seed_presets_if_empty_impl(&state).await.unwrap();
        let providers = list_providers_impl(&state).await.unwrap();
        assert_eq!(providers.len(), 4);
        // second seed is a no-op
        seed_presets_if_empty_impl(&state).await.unwrap();
        assert_eq!(list_providers_impl(&state).await.unwrap().len(), 4);
        assert!(providers.iter().all(|p| !p.has_key));
        set_api_key_impl(&state, "openai".into(), "sk-test".into())
            .await
            .unwrap();
        let providers = list_providers_impl(&state).await.unwrap();
        let openai = providers.iter().find(|p| p.id == "openai").unwrap();
        assert!(openai.has_key);
        assert_eq!(
            state.keys.get("openai").unwrap().as_deref(),
            Some("sk-test")
        );
        delete_api_key_impl(&state, "openai".into()).await.unwrap();
        let openai = list_providers_impl(&state)
            .await
            .unwrap()
            .into_iter()
            .find(|p| p.id == "openai")
            .unwrap();
        assert!(!openai.has_key);
        assert_eq!(state.keys.get("openai").unwrap(), None);
        // custom provider
        let custom = crate::core::types::ProviderConfig {
            id: "groq".into(),
            label: "Groq".into(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: Some("https://api.groq.com/openai/v1".into()),
            has_key: false,
            models: vec!["llama-3.3-70b".into()],
            disabled_models: vec![],
            extra_headers: vec![],
        };
        upsert_provider_impl(&state, custom).await.unwrap();
        assert_eq!(list_providers_impl(&state).await.unwrap().len(), 5);
        delete_provider_impl(&state, "groq".into()).await.unwrap();
        assert_eq!(list_providers_impl(&state).await.unwrap().len(), 4);
    }

    #[tokio::test]
    async fn ui_state_roundtrip() {
        let state = AppState::test();
        let initial = get_initial_state_impl(&state).await.unwrap();
        assert!(initial.config.last_conversation_id.is_none());
        set_ui_state_impl(&state, Some("w1".into()), Some("c1".into()))
            .await
            .unwrap();
        let after = get_initial_state_impl(&state).await.unwrap();
        assert_eq!(after.config.last_workspace_id.as_deref(), Some("w1"));
        assert_eq!(after.config.last_conversation_id.as_deref(), Some("c1"));
    }

    #[test]
    fn parse_tags_extracts_names() {
        let body = serde_json::json!({
            "models": [
                {"name": "qwen3:0.6b", "model": "qwen3:0.6b", "size": 522_000_000_u64},
                {"name": "llama3.2:latest"},
                {"size": 123}
            ]
        });
        assert_eq!(
            parse_tags(&body),
            vec!["qwen3:0.6b".to_string(), "llama3.2:latest".to_string()]
        );
        assert!(parse_tags(&serde_json::json!({})).is_empty());
        assert!(parse_tags(&serde_json::json!({"models": "nope"})).is_empty());
    }

    #[tokio::test]
    async fn messages_roundtrip() {
        let state = AppState::test();
        let dir = tempfile::tempdir().unwrap();
        let ws = add_workspace_impl(
            &state,
            "proj".into(),
            dir.path().to_string_lossy().to_string(),
        )
        .await
        .unwrap();
        let cid = create_conversation_impl(&state, ws, "c".into(), "ollama".into(), "m".into())
            .await
            .unwrap();
        let msg = Message::text(Role::User, "hello");
        state.store.append_message(&cid, &msg).unwrap();
        let msgs = get_messages_impl(&state, cid).await.unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, msg.role);
        assert_eq!(msgs[0].parts, msg.parts);
        assert!(msgs[0].created_at > 0);
    }
}
