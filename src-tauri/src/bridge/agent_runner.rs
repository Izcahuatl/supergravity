use crate::bridge::commands::{block, estr};
use crate::bridge::state::{AppState, RunningAgent};
use crate::core::agent::{self, AgentRequest, DEFAULT_MAX_ITERATIONS};
use crate::core::approvals::ApprovalBroker;
use crate::core::error::{Error, Result};
use crate::core::providers::{build_provider, Provider};
use crate::core::store::{ConversationRow, Store};
use crate::core::tools::default_tools;
use crate::core::types::{AgentEvent, ApprovalMode, Message, Role};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
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

/// Removes the agents-map entry on drop - including panic paths. Dropping the
/// entry also drops the last broker `Sender` clone, which lets the pump end.
struct AgentGuard {
    agents: Arc<Mutex<HashMap<String, RunningAgent>>>,
    cid: String,
}

impl Drop for AgentGuard {
    fn drop(&mut self) {
        if let Ok(mut g) = self.agents.lock() {
            g.remove(&self.cid);
        }
    }
}

/// Everything the spawned agent task needs (assembled after the atomic reservation).
pub struct AgentTaskParts {
    pub agents: Arc<Mutex<HashMap<String, RunningAgent>>>,
    pub store: Arc<Store>,
    pub workspace_root: PathBuf,
    pub model: String,
    pub history: Vec<Message>,
    pub provider: Arc<dyn Provider>,
    pub broker: Arc<ApprovalBroker>,
    pub cancel: CancellationToken,
    pub events_tx: mpsc::Sender<AgentEvent>,
    pub events_rx: mpsc::Receiver<AgentEvent>,
    pub conversation_id: String,
    /// Id of the user message that triggered this run (checkpoint boundary).
    pub after_message_id: i64,
    /// Provider answering this run (stamped onto produced messages).
    pub provider_id: String,
    /// Per-conversation scratch dir (created lazily; None in tests).
    pub workshop_root: Option<PathBuf>,
}

impl AgentTaskParts {
    /// Reserve the agents-map slot (atomic guard+insert), then load everything
    /// the run needs. On any failure the reservation is released.
    pub async fn new(
        state: &AppState,
        conv: ConversationRow,
        provider: Arc<dyn Provider>,
    ) -> Result<Self> {
        let cid = conv.id.clone();
        let (events_tx, events_rx) = mpsc::channel::<AgentEvent>(256);
        let cancel = CancellationToken::new();
        let broker = Arc::new(ApprovalBroker::new(ApprovalMode::Manual, events_tx.clone()));
        {
            let mut agents = state.agents.lock().unwrap();
            if agents.contains_key(&cid) {
                return Err(Error::Tool(
                    "an agent is already running in this conversation".into(),
                ));
            }
            agents.insert(
                cid.clone(),
                RunningAgent {
                    cancel: cancel.clone(),
                    broker: broker.clone(),
                },
            );
        }
        let result = Self::load(state, &conv, provider, broker, cancel, events_tx, events_rx).await;
        if result.is_err() {
            state.agents.lock().unwrap().remove(&cid);
        }
        result
    }

    async fn load(
        state: &AppState,
        conv: &ConversationRow,
        provider: Arc<dyn Provider>,
        broker: Arc<ApprovalBroker>,
        cancel: CancellationToken,
        events_tx: mpsc::Sender<AgentEvent>,
        events_rx: mpsc::Receiver<AgentEvent>,
    ) -> Result<Self> {
        broker.set_mode(conv.approval_mode);
        // Permission policy from app config (Permissions section in Settings).
        {
            let cfg = state.ui_config.lock().unwrap();
            broker.set_permissions(
                crate::core::approvals::ExternalPolicy::from_config(
                    cfg.external_policy.as_deref(),
                ),
                cfg.workshop_full_access.unwrap_or(true),
                cfg.project_files_no_ask.unwrap_or(false),
                cfg.project_shell_no_ask.unwrap_or(false),
            );
        }
        let ws = {
            let s = state.store.clone();
            let w = conv.workspace_id.clone();
            block(move || s.get_workspace(&w)).await?
        };
        let history = {
            let s = state.store.clone();
            let c = conv.id.clone();
            block(move || s.get_messages(&c)).await?
        };
        let after_message_id = {
            let s = state.store.clone();
            let c = conv.id.clone();
            block(move || s.last_message_id(&c)).await?
        };
        // Per-conversation scratch dir ("Workshop") under the app data dir.
        // Production only - tests use a relative config path and skip this.
        let workshop_root = if state.config_path.is_absolute() {
            state.config_path.parent().and_then(|p| {
                let d = p.join("workshops").join(&conv.id);
                match std::fs::create_dir_all(&d) {
                    Ok(()) => Some(d),
                    Err(e) => {
                        eprintln!("supergravity: cannot create workshop dir: {e}");
                        None
                    }
                }
            })
        } else {
            None
        };
        Ok(AgentTaskParts {
            agents: state.agents.clone(),
            store: state.store.clone(),
            workspace_root: PathBuf::from(&ws.path),
            model: conv.model.clone(),
            history,
            provider,
            broker,
            cancel,
            events_tx,
            events_rx,
            conversation_id: conv.id.clone(),
            after_message_id,
            provider_id: conv.provider_id.clone(),
            workshop_root,
        })
    }
}

/// The spawned task body: pump events, run the loop, persist produced messages
/// (even on failure - partial runs stay resumable), free the agents-map entry.
pub async fn run_agent_task(
    parts: AgentTaskParts,
    emit: impl Fn(serde_json::Value) + Send + 'static,
) {
    let guard = AgentGuard {
        agents: parts.agents.clone(),
        cid: parts.conversation_id.clone(),
    };
    let pump = tokio::spawn(pump_events(
        parts.conversation_id.clone(),
        parts.events_rx,
        emit,
    ));
    // Captured before AgentRequest moves the fields out (used for stamping).
    let stamp_pid = parts.provider_id.clone();
    let stamp_model = parts.model.clone();
    // Block policy: external tools are not even offered to the model.
    let tools: Vec<Box<dyn crate::core::tools::Tool>> =
        if parts.broker.external_policy() == crate::core::approvals::ExternalPolicy::Block {
            default_tools()
                .into_iter()
                .filter(|t| {
                    !["list_external_dir", "read_external_file", "write_external_file"]
                        .contains(&t.spec().name.as_str())
                })
                .collect()
        } else {
            default_tools()
        };
    let outcome = agent::run(AgentRequest {
        workspace_root: parts.workspace_root,
        provider: parts.provider,
        model: parts.model,
        history: parts.history,
        tools,
        approvals: parts.broker,
        events: parts.events_tx,
        cancel: parts.cancel,
        max_iterations: DEFAULT_MAX_ITERATIONS,
        backup: Some(agent::BackupCtx {
            store: parts.store.clone(),
            conversation_id: parts.conversation_id.clone(),
            after_message_id: parts.after_message_id,
        }),
        workshop_root: parts.workshop_root.clone(),
    })
    .await;
    {
        let store = parts.store.clone();
        let cid = parts.conversation_id.clone();
        let produced = outcome.produced;
        let _ = block(move || {
            for msg in &produced {
                if let Err(e) = store.append_message_with_provider(
                    &cid,
                    msg,
                    Some((&stamp_pid, &stamp_model)),
                ) {
                    eprintln!("supergravity: failed to persist produced message: {e}");
                }
            }
            Ok(())
        })
        .await;
    }
    // Drop the map entry (and with it the last broker Sender clone) BEFORE
    // awaiting the pump - otherwise the channel never closes and this task
    // deadlocks. (A new run may start while the old pump drains its last
    // buffered events; that window is acceptable.)
    drop(guard);
    let _ = pump.await;
}

/// Everything send_message does BEFORE spawning the agent task: resolve the
/// conversation/provider/key, build the provider, persist the user message,
/// and auto-title placeholder conversations. Split out so it can be tested
/// without an AppHandle.
pub async fn prepare_run(
    state: &AppState,
    conversation_id: String,
    text: String,
) -> Result<(ConversationRow, Arc<dyn Provider>)> {
    let conv = {
        let s = state.store.clone();
        let c = conversation_id.clone();
        block(move || s.get_conversation(&c)).await?
    };
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

    // Expand @path mentions into <attached> blocks before persisting, so the
    // file context stays available for the rest of the conversation.
    let text = {
        let s = state.store.clone();
        let w = conv.workspace_id.clone();
        let ws = block(move || s.get_workspace(&w)).await?;
        crate::core::mentions::expand(&text, std::path::Path::new(&ws.path))
    };
    let user_msg = Message::text(Role::User, &text);
    {
        let s = state.store.clone();
        let c = conversation_id.clone();
        block(move || s.append_message(&c, &user_msg)).await?;
    }
    // Auto-title placeholder conversations from the first message.
    if conv.title == "New Conversation" || conv.title.trim().is_empty() {
        let title: String = text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(40)
            .collect();
        let s = state.store.clone();
        let c = conversation_id.clone();
        block(move || s.rename_conversation(&c, &title)).await?;
    }
    Ok((conv, provider))
}

pub async fn send_message_impl(
    app: AppHandle,
    state: &AppState,
    conversation_id: String,
    text: String,
) -> Result<()> {
    let (conv, provider) = prepare_run(state, conversation_id, text).await?;
    let parts = AgentTaskParts::new(state, conv, provider).await?;
    tauri::async_runtime::spawn(async move {
        let emit = move |v: serde_json::Value| {
            let _ = app.emit("agent-event", v);
        };
        run_agent_task(parts, emit).await;
    });
    Ok(())
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

pub async fn resolve_approval_impl(
    state: &AppState,
    conversation_id: String,
    request_id: String,
    allow: bool,
) -> Result<()> {
    match state.agents.lock().unwrap().get(&conversation_id) {
        Some(agent) => agent.broker.resolve(&request_id, allow),
        None => Err(Error::Tool("no agent running in this conversation".into())),
    }
}

// Wrappers use fully-qualified Result: `core::error::Result` (1-param alias) is
// imported in this file, so unqualified `Result<(), String>` would not compile.
#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    text: String,
) -> std::result::Result<(), String> {
    send_message_impl(app, &state, conversation_id, text)
        .await
        .map_err(estr)
}

#[tauri::command]
pub async fn cancel_agent(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> std::result::Result<(), String> {
    cancel_agent_impl(&state, conversation_id)
        .await
        .map_err(estr)
}

#[tauri::command]
pub async fn resolve_approval(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    request_id: String,
    allow: bool,
) -> std::result::Result<(), String> {
    resolve_approval_impl(&state, conversation_id, request_id, allow)
        .await
        .map_err(estr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::state::AppState;
    use crate::core::providers::mock::MockProvider;
    use crate::core::types::{AgentEvent, ChatEvent, Message, Role};
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
        assert!(
            out_rx.try_recv().is_err(),
            "channel closes after senders drop"
        );
    }

    /// Full lifecycle: scripted run persists produced messages, emits events,
    /// and ALWAYS frees the agents-map entry (the deadlock regression test).
    #[tokio::test]
    async fn agent_task_lifecycle_completes_and_cleans_up() {
        let state = AppState::test();
        let dir = tempfile::tempdir().unwrap();
        let ws = state
            .store
            .add_workspace("proj", &dir.path().to_string_lossy())
            .unwrap();
        let cid = state
            .store
            .create_conversation(
                &ws,
                "c",
                "mock",
                "m",
                crate::core::types::ApprovalMode::Auto,
            )
            .unwrap();
        state
            .store
            .append_message(&cid, &Message::text(Role::User, "go"))
            .unwrap();

        let provider = std::sync::Arc::new(MockProvider::new(vec![vec![
            Ok(ChatEvent::TextDelta("hello".into())),
            Ok(ChatEvent::Done),
        ]]));
        let conv = state.store.get_conversation(&cid).unwrap();
        let parts = AgentTaskParts::new(&state, conv, provider).await.unwrap();
        let (out_tx, mut out_rx) = mpsc::channel::<serde_json::Value>(16);
        let events_seen = tokio::spawn(async move {
            let mut kinds = vec![];
            while let Some(v) = out_rx.recv().await {
                kinds.push(v["event"]["kind"].as_str().unwrap().to_string());
            }
            kinds
        });

        // Must COMPLETE (the original pump-order bug deadlocked here forever).
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            run_agent_task(parts, move |v| {
                let _ = out_tx.try_send(v);
            }),
        )
        .await
        .expect("agent task must not deadlock");

        // Map entry freed.
        assert!(
            state.agents.lock().unwrap().is_empty(),
            "agents map must be empty after run"
        );
        // Produced assistant message persisted.
        let msgs = state.store.get_messages(&cid).unwrap();
        assert_eq!(msgs.len(), 2, "user + assistant");
        assert_eq!(msgs[1].role, Role::Assistant);
        // Events flowed through the emit seam.
        let kinds = events_seen.await.unwrap();
        assert!(kinds.contains(&"text_delta".to_string()), "{kinds:?}");
        assert!(kinds.contains(&"message_done".to_string()), "{kinds:?}");
    }

    #[tokio::test]
    async fn reservation_guard_rejects_duplicate() {
        let state = AppState::test();
        let dir = tempfile::tempdir().unwrap();
        let ws = state
            .store
            .add_workspace("proj", &dir.path().to_string_lossy())
            .unwrap();
        let cid = state
            .store
            .create_conversation(
                &ws,
                "c",
                "mock",
                "m",
                crate::core::types::ApprovalMode::Auto,
            )
            .unwrap();
        let conv = state.store.get_conversation(&cid).unwrap();
        let provider = std::sync::Arc::new(MockProvider::new(vec![]));
        let _parts = AgentTaskParts::new(&state, conv.clone(), provider)
            .await
            .unwrap();
        let provider2 = std::sync::Arc::new(MockProvider::new(vec![]));
        // `.err().unwrap()` instead of `.unwrap_err()`: the latter needs
        // `AgentTaskParts: Debug`, which it cannot derive (`Arc<dyn Provider>`).
        let err = AgentTaskParts::new(&state, conv, provider2)
            .await
            .err()
            .unwrap();
        assert!(err.to_string().contains("already running"), "{err}");
    }

    /// Reproduces the user's "message shows, then nothing" report: full
    /// prepare_run → agent task path against a LIVE Ollama server.
    /// Run with: cargo test --lib bridge::agent_runner -- --ignored
    #[tokio::test]
    #[ignore = "needs a local Ollama server with qwen3:0.6b"]
    async fn live_send_message_produces_assistant_reply() {
        let state = AppState::test();
        let dir = tempfile::tempdir().unwrap();
        let ws = state
            .store
            .add_workspace("proj", &dir.path().to_string_lossy())
            .unwrap();
        // Seed an ollama provider config matching production.
        state
            .store
            .upsert_provider(&crate::core::types::ProviderConfig {
                id: "ollama".into(),
                label: "Ollama (local)".into(),
                kind: crate::core::types::ProviderKind::Ollama,
                base_url: None,
                has_key: false,
                models: vec!["qwen3:0.6b".into()],
                disabled_models: vec![],
            extra_headers: vec![],
            })
            .unwrap();
        let cid = state
            .store
            .create_conversation(&ws, "New Conversation", "ollama", "qwen3:0.6b", crate::core::types::ApprovalMode::Auto)
            .unwrap();

        let (conv, provider) = prepare_run(&state, cid.clone(), "say hi".into()).await.unwrap();
        let parts = AgentTaskParts::new(&state, conv, provider).await.unwrap();
        let (out_tx, mut out_rx) = mpsc::channel::<serde_json::Value>(4096);
        tokio::time::timeout(
            std::time::Duration::from_secs(120),
            run_agent_task(parts, move |v| {
                let _ = out_tx.try_send(v);
            }),
        )
        .await
        .expect("agent task must complete");

        let mut kinds = vec![];
        while let Ok(v) = out_rx.try_recv() {
            kinds.push(v["event"]["kind"].as_str().unwrap().to_string());
        }
        assert!(kinds.contains(&"text_delta".to_string()), "{kinds:?}");
        assert!(kinds.contains(&"message_done".to_string()), "{kinds:?}");

        let msgs = state.store.get_messages(&cid).unwrap();
        assert!(msgs.iter().any(|m| m.role == Role::Assistant), "assistant reply must be persisted: {msgs:?}");
        // Auto-title fired.
        assert_ne!(state.store.get_conversation(&cid).unwrap().title, "New Conversation");
    }
}
