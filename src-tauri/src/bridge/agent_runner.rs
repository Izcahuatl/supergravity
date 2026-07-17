use crate::bridge::commands::{block, estr};
use crate::bridge::state::AppState;
use crate::core::agent::{self, AgentRequest, DEFAULT_MAX_ITERATIONS};
use crate::core::approvals::ApprovalBroker;
use crate::core::error::{Error, Result};
use crate::core::providers::{build_provider, Provider};
use crate::core::store::ConversationRow;
use crate::core::tools::default_tools;
use crate::core::types::{AgentEvent, Message, Role};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Forward agent events to the webview as `{"conversation_id", "event"}` JSON.
pub async fn pump_events(
    conversation_id: String,
    mut rx: mpsc::Receiver<AgentEvent>,
    emit: impl Fn(serde_json::Value) + Send,
) {
    while let Some(ev) = rx.recv().await {
        emit(serde_json::json!({ "conversation_id": conversation_id, "event": ev }));
    }
}

/// Spawn the agent for a conversation with an explicit provider (test seam).
pub async fn spawn_agent(
    app: AppHandle,
    state: &AppState,
    conv: ConversationRow,
    provider: Arc<dyn Provider>,
) -> Result<()> {
    let cid = conv.id.clone();
    let ws = {
        let s = state.store.clone();
        let w = conv.workspace_id.clone();
        block(move || s.get_workspace(&w)).await?
    };
    let history = {
        let s = state.store.clone();
        let c = cid.clone();
        block(move || s.get_messages(&c)).await?
    };
    let (events_tx, events_rx) = mpsc::channel::<AgentEvent>(256);
    let broker = Arc::new(ApprovalBroker::new(conv.approval_mode, events_tx.clone()));
    let cancel = CancellationToken::new();
    state.agents.lock().unwrap().insert(
        cid.clone(),
        crate::bridge::state::RunningAgent { cancel: cancel.clone(), broker: broker.clone() },
    );
    let store = state.store.clone();
    let agents = state.agents.clone();
    let cid_pump = cid.clone();
    tauri::async_runtime::spawn(async move {
        let app2 = app.clone();
        let pump = tauri::async_runtime::spawn(pump_events(cid_pump, events_rx, move |v| {
            let _ = app2.emit("agent-event", v);
        }));
        let outcome = agent::run(AgentRequest {
            workspace_root: PathBuf::from(&ws.path),
            provider,
            model: conv.model.clone(),
            history,
            tools: default_tools(),
            approvals: broker,
            events: events_tx,
            cancel,
            max_iterations: DEFAULT_MAX_ITERATIONS,
        })
        .await;
        // Persist produced messages even on failure (partial runs stay resumable).
        for msg in &outcome.produced {
            let _ = store.append_message(&cid, msg);
        }
        // Broker/agent senders are dropped with `run`; the pump ends when the
        // channel closes.
        let _ = pump.await;
        agents.lock().unwrap().remove(&cid);
    });
    Ok(())
}

pub async fn send_message_impl(
    app: AppHandle,
    state: &AppState,
    conversation_id: String,
    text: String,
) -> Result<()> {
    if state.agents.lock().unwrap().contains_key(&conversation_id) {
        return Err(Error::Tool("an agent is already running in this conversation".into()));
    }
    let conv = {
        let s = state.store.clone();
        let c = conversation_id.clone();
        block(move || s.get_conversation(&c)).await?
    };
    let user_msg = Message::text(Role::User, &text);
    {
        let s = state.store.clone();
        let c = conversation_id.clone();
        block(move || s.append_message(&c, &user_msg)).await?;
    }
    // Auto-title placeholder conversations from the first message.
    if conv.title == "New Conversation" || conv.title.trim().is_empty() {
        let title: String = text.chars().take(40).collect();
        let s = state.store.clone();
        let c = conversation_id.clone();
        block(move || s.rename_conversation(&c, &title)).await?;
    }
    let pcfg = {
        let s = state.store.clone();
        let pid = conv.provider_id.clone();
        block(move || s.get_provider(&pid)).await?
    };
    let api_key = {
        let k = state.keys.clone();
        let pid = conv.provider_id.clone();
        block(move || k.get(&pid)).await?
    };
    let provider: Arc<dyn Provider> = Arc::from(build_provider(&pcfg, api_key)?);
    spawn_agent(app, state, conv, provider).await
}

pub async fn cancel_agent_impl(state: &AppState, conversation_id: String) -> Result<()> {
    match state.agents.lock().unwrap().get(&conversation_id) {
        Some(agent) => {
            agent.cancel.cancel();
            Ok(())
        }
        None => Err(Error::Tool("no agent running in this conversation".into())),
    }
}

pub async fn resolve_approval_impl(state: &AppState, conversation_id: String, request_id: String, allow: bool) -> Result<()> {
    match state.agents.lock().unwrap().get(&conversation_id) {
        Some(agent) => agent.broker.resolve(&request_id, allow),
        None => Err(Error::Tool("no agent running in this conversation".into())),
    }
}

// `Result` here is `core::error::Result` (1-parameter alias), so the command
// wrappers must spell out the two-parameter std form (same as commands.rs).
#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    text: String,
) -> std::result::Result<(), String> {
    send_message_impl(app, &state, conversation_id, text).await.map_err(estr)
}

#[tauri::command]
pub async fn cancel_agent(state: tauri::State<'_, AppState>, conversation_id: String) -> std::result::Result<(), String> {
    cancel_agent_impl(&state, conversation_id).await.map_err(estr)
}

#[tauri::command]
pub async fn resolve_approval(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    request_id: String,
    allow: bool,
) -> std::result::Result<(), String> {
    resolve_approval_impl(&state, conversation_id, request_id, allow).await.map_err(estr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::AgentEvent;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn pump_forwards_events_as_envelopes() {
        let (tx, rx) = mpsc::channel(8);
        let (out_tx, mut out_rx) = mpsc::channel(8);
        let pump = tokio::spawn(pump_events("conv-1".into(), rx, move |v| {
            let _ = out_tx.try_send(v);
        }));
        tx.send(AgentEvent::TextDelta("hi".into())).await.unwrap();
        tx.send(AgentEvent::MessageDone).await.unwrap();
        drop(tx);
        pump.await.unwrap();
        let first = out_rx.recv().await.unwrap();
        assert_eq!(
            first,
            serde_json::json!({"conversation_id": "conv-1", "event": {"kind": "text_delta", "data": "hi"}})
        );
        let second = out_rx.recv().await.unwrap();
        assert_eq!(
            second,
            serde_json::json!({"conversation_id": "conv-1", "event": {"kind": "message_done"}})
        );
        assert!(out_rx.try_recv().is_err(), "channel closes after senders drop");
    }
}
