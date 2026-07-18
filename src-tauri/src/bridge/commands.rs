use crate::bridge::state::AppState;
use crate::core::config::AppConfig;
use crate::core::error::{Error, Result};
use crate::core::store::{ConversationRow, WorkspaceRow};
use crate::core::types::{Message, ProviderConfig};
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
    let s = state.store.clone();
    block(move || {
        s.create_conversation(
            &workspace_id,
            &title,
            &provider_id,
            &model,
            crate::core::types::ApprovalMode::Manual,
        )
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
    let s = state.store.clone();
    block(move || s.delete_conversation(&id)).await
}

pub async fn get_messages_impl(state: &AppState, conversation_id: String) -> Result<Vec<Message>> {
    let s = state.store.clone();
    block(move || s.get_messages(&conversation_id)).await
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
}

pub async fn get_initial_state_impl(state: &AppState) -> Result<InitialState> {
    let workspaces = list_workspaces_impl(state).await?;
    let providers = list_providers_impl(state).await?;
    let config = state.ui_config.lock().unwrap().clone();
    Ok(InitialState {
        config,
        workspaces,
        providers,
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
) -> std::result::Result<Vec<Message>, String> {
    get_messages_impl(&state, conversation_id)
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
        assert_eq!(msgs, vec![msg]);
    }
}
