use crate::core::approvals::ApprovalBroker;
use crate::core::error::{Error, Result};
use crate::core::tools::{Tool, ToolContext};
use crate::core::types::{AgentEvent, ChatEvent, ContentPart, Message, Role};
use futures::StreamExt;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub const DEFAULT_MAX_ITERATIONS: usize = 50;

pub struct AgentRequest {
    pub workspace_root: PathBuf,
    pub provider: Arc<dyn crate::core::providers::Provider>,
    pub model: String,
    /// Conversation history including the new user message at the end.
    pub history: Vec<Message>,
    pub tools: Vec<Box<dyn Tool>>,
    pub approvals: Arc<ApprovalBroker>,
    pub events: mpsc::Sender<AgentEvent>,
    pub cancel: CancellationToken,
    pub max_iterations: usize,
}

/// Run the tool-call loop until the model stops calling tools.
/// Returns the messages produced during this run (assistant + tool messages),
/// which the caller persists to the store.
pub async fn run(req: AgentRequest) -> Result<Vec<Message>> {
    let mut messages = req.history.clone();
    let mut produced: Vec<Message> = Vec::new();

    for _ in 0..req.max_iterations {
        if req.cancel.is_cancelled() {
            let _ = req.events.send(AgentEvent::Cancelled).await;
            return Err(Error::Cancelled);
        }

        let tool_specs: Vec<crate::core::types::ToolSpec> = req.tools.iter().map(|t| t.spec()).collect();
        let mut stream = req.provider.stream_chat(&req.model, &messages, &tool_specs).await?;

        let mut text = String::new();
        let mut calls: Vec<(String, String, String)> = Vec::new(); // (id, name, args_json)
        let mut stream_err: Option<Error> = None;

        while let Some(item) = stream.next().await {
            if req.cancel.is_cancelled() {
                let _ = req.events.send(AgentEvent::Cancelled).await;
                return Err(Error::Cancelled);
            }
            match item {
                Ok(ChatEvent::TextDelta(d)) => {
                    text.push_str(&d);
                    let _ = req.events.send(AgentEvent::TextDelta(d)).await;
                }
                Ok(ChatEvent::ToolCall { id, name, args_json }) => calls.push((id, name, args_json)),
                Ok(ChatEvent::Usage { .. }) => {}
                Ok(ChatEvent::Error(msg)) => {
                    stream_err = Some(Error::Provider { status: 0, body: msg });
                    break;
                }
                Ok(ChatEvent::Done) => break,
                Err(e) => {
                    stream_err = Some(e);
                    break;
                }
            }
        }

        if let Some(e) = stream_err {
            let _ = req.events.send(AgentEvent::Error(e.to_string())).await;
            return Err(e);
        }

        let mut parts: Vec<ContentPart> = Vec::new();
        if !text.is_empty() {
            parts.push(ContentPart::Text { text });
        }
        for (id, name, args_json) in &calls {
            parts.push(ContentPart::ToolCall { id: id.clone(), name: name.clone(), args_json: args_json.clone() });
        }
        let assistant = Message { role: Role::Assistant, parts };
        messages.push(assistant.clone());
        produced.push(assistant);

        if calls.is_empty() {
            let _ = req.events.send(AgentEvent::MessageDone).await;
            return Ok(produced);
        }

        let ctx = ToolContext { workspace_root: req.workspace_root.clone() };
        let mut results: Vec<ContentPart> = Vec::new();
        for (id, name, args_json) in calls {
            if req.cancel.is_cancelled() {
                let _ = req.events.send(AgentEvent::Cancelled).await;
                return Err(Error::Cancelled);
            }
            let _ = req
                .events
                .send(AgentEvent::ToolCallProposed { id: id.clone(), name: name.clone(), args_json: args_json.clone() })
                .await;

            let tool = req.tools.iter().find(|t| t.spec().name == name);
            let result = match tool {
                None => {
                    let _ = req
                        .events
                        .send(AgentEvent::ToolCallFinished {
                            tool_call_id: id.clone(),
                            ok: false,
                            summary: format!("unknown tool: {name}"),
                        })
                        .await;
                    ContentPart::ToolResult { tool_call_id: id, content: format!("unknown tool: {name}"), is_error: true }
                }
                Some(t) => {
                    if t.needs_approval() {
                        // Cancel must interrupt the approval wait, not just iterations.
                        let decision = tokio::select! {
                            _ = req.cancel.cancelled() => {
                                let _ = req.events.send(AgentEvent::Cancelled).await;
                                return Err(Error::Cancelled);
                            }
                            res = req.approvals.check(&id, &name, &args_json) => res,
                        };
                        match decision {
                            Ok(true) => {}
                            Ok(false) => {
                                let _ = req
                                    .events
                                    .send(AgentEvent::ToolCallFinished {
                                        tool_call_id: id.clone(),
                                        ok: false,
                                        summary: "denied by user".into(),
                                    })
                                    .await;
                                results.push(ContentPart::ToolResult {
                                    tool_call_id: id,
                                    content: "user denied this action".into(),
                                    is_error: true,
                                });
                                continue;
                            }
                            Err(e) => {
                                results.push(ContentPart::ToolResult {
                                    tool_call_id: id,
                                    content: format!("approval error: {e}"),
                                    is_error: true,
                                });
                                continue;
                            }
                        }
                    }
                    match t.execute(&ctx, &args_json).await {
                        Ok(output) => {
                            let summary: String = output.chars().take(80).collect();
                            let _ = req
                                .events
                                .send(AgentEvent::ToolCallFinished { tool_call_id: id.clone(), ok: true, summary })
                                .await;
                            ContentPart::ToolResult { tool_call_id: id, content: output, is_error: false }
                        }
                        Err(e) => {
                            let _ = req
                                .events
                                .send(AgentEvent::ToolCallFinished { tool_call_id: id.clone(), ok: false, summary: e.to_string() })
                                .await;
                            ContentPart::ToolResult { tool_call_id: id, content: e.to_string(), is_error: true }
                        }
                    }
                }
            };
            results.push(result);
        }
        let tool_msg = Message { role: Role::Tool, parts: results };
        messages.push(tool_msg.clone());
        produced.push(tool_msg);
    }

    let err = Error::Tool(format!("max iterations ({}) reached", req.max_iterations));
    let _ = req.events.send(AgentEvent::Error(err.to_string())).await;
    Err(err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::approvals::ApprovalBroker;
    use crate::core::providers::mock::MockProvider;
    use crate::core::tools::{Tool, ToolContext};
    use crate::core::types::*;
    use tokio::sync::mpsc;

    /// Simple test tool: echoes args; configurable approval requirement.
    struct EchoTool { needs_approval: bool }

    #[async_trait::async_trait]
    impl Tool for EchoTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec { name: "echo".into(), description: "echo args".into(), params_schema: serde_json::json!({"type": "object"}) }
        }
        fn needs_approval(&self) -> bool {
            self.needs_approval
        }
        async fn execute(&self, _ctx: &ToolContext, args_json: &str) -> crate::core::error::Result<String> {
            Ok(format!("echoed: {args_json}"))
        }
    }

    struct RunArgs {
        script: Vec<Vec<crate::core::error::Result<ChatEvent>>>,
        mode: ApprovalMode,
        tools: Vec<Box<dyn Tool>>,
        max_iterations: usize,
    }

    async fn run_agent(args: RunArgs) -> (crate::core::error::Result<Vec<Message>>, Vec<AgentEvent>, std::sync::Arc<MockProvider>, std::sync::Arc<ApprovalBroker>) {
        let provider = std::sync::Arc::new(MockProvider::new(args.script));
        let (events_tx, mut events_rx) = mpsc::channel(64);
        let broker = std::sync::Arc::new(ApprovalBroker::new(args.mode, events_tx.clone()));
        let dir = tempfile::tempdir().unwrap();
        let req = AgentRequest {
            workspace_root: dir.path().to_path_buf(),
            provider: provider.clone(),
            model: "m".into(),
            history: vec![Message::text(Role::User, "go")],
            tools: args.tools,
            approvals: broker.clone(),
            events: events_tx,
            cancel: tokio_util::sync::CancellationToken::new(),
            max_iterations: args.max_iterations,
        };
        let handle = tokio::spawn(run(req));
        let mut events = vec![];
        while let Some(ev) = events_rx.recv().await {
            let done = matches!(ev, AgentEvent::MessageDone | AgentEvent::Error(_) | AgentEvent::Cancelled);
            events.push(ev);
            if done {
                break;
            }
        }
        let result = handle.await.unwrap();
        (result, events, provider, broker)
    }

    #[tokio::test]
    async fn text_only_turn() {
        let script = vec![vec![Ok(ChatEvent::TextDelta("hi ".into())), Ok(ChatEvent::TextDelta("there".into())), Ok(ChatEvent::Done)]];
        let (result, events, _, _) = run_agent(RunArgs { script, mode: ApprovalMode::Auto, tools: vec![], max_iterations: 5 }).await;
        let msgs = result.unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, Role::Assistant);
        assert_eq!(msgs[0].parts, vec![ContentPart::Text { text: "hi there".into() }]);
        assert!(events.contains(&AgentEvent::TextDelta("hi ".into())));
        assert!(events.contains(&AgentEvent::MessageDone));
    }

    #[tokio::test]
    async fn tool_cycle_appends_results_and_continues() {
        let script = vec![
            vec![Ok(ChatEvent::ToolCall { id: "c1".into(), name: "echo".into(), args_json: "{\"a\":1}".into() }), Ok(ChatEvent::Done)],
            vec![Ok(ChatEvent::TextDelta("done!".into())), Ok(ChatEvent::Done)],
        ];
        let (result, events, provider, _) = run_agent(RunArgs {
            script,
            mode: ApprovalMode::Auto,
            tools: vec![Box::new(EchoTool { needs_approval: false })],
            max_iterations: 5,
        })
        .await;
        let msgs = result.unwrap();
        assert_eq!(msgs.len(), 3, "assistant(call) + tool result + assistant(final): {msgs:?}");
        assert_eq!(msgs[1].role, Role::Tool);
        assert_eq!(
            msgs[1].parts,
            vec![ContentPart::ToolResult { tool_call_id: "c1".into(), content: "echoed: {\"a\":1}".into(), is_error: false }]
        );
        // second provider call must include the tool result in history
        let calls = provider.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls[1].1.iter().any(|m| m.role == Role::Tool));
        assert!(events.contains(&AgentEvent::ToolCallProposed { id: "c1".into(), name: "echo".into(), args_json: "{\"a\":1}".into() }));
        assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolCallFinished { ok: true, .. })));
    }

    #[tokio::test]
    async fn denied_approval_becomes_error_tool_result() {
        let script = vec![
            vec![Ok(ChatEvent::ToolCall { id: "c1".into(), name: "echo".into(), args_json: "{}".into() }), Ok(ChatEvent::Done)],
            vec![Ok(ChatEvent::TextDelta("ok".into())), Ok(ChatEvent::Done)],
        ];
        let provider = std::sync::Arc::new(MockProvider::new(script));
        let (events_tx, mut events_rx) = mpsc::channel(64);
        let broker = std::sync::Arc::new(ApprovalBroker::new(ApprovalMode::Manual, events_tx.clone()));
        let dir = tempfile::tempdir().unwrap();
        let req = AgentRequest {
            workspace_root: dir.path().to_path_buf(),
            provider: provider.clone(),
            model: "m".into(),
            history: vec![Message::text(Role::User, "go")],
            tools: vec![Box::new(EchoTool { needs_approval: true })],
            approvals: broker.clone(),
            events: events_tx,
            cancel: tokio_util::sync::CancellationToken::new(),
            max_iterations: 5,
        };
        let handle = tokio::spawn(run(req));
        // deny the approval request when it arrives
        while let Some(ev) = events_rx.recv().await {
            if let AgentEvent::ApprovalRequested { request_id, .. } = ev {
                broker.resolve(&request_id, false).unwrap();
                break;
            }
        }
        let msgs = handle.await.unwrap().unwrap();
        assert_eq!(
            msgs[1].parts,
            vec![ContentPart::ToolResult { tool_call_id: "c1".into(), content: "user denied this action".into(), is_error: true }]
        );
    }

    #[tokio::test]
    async fn unknown_tool_is_error_result_not_crash() {
        let script = vec![
            vec![Ok(ChatEvent::ToolCall { id: "c1".into(), name: "nope".into(), args_json: "{}".into() }), Ok(ChatEvent::Done)],
            vec![Ok(ChatEvent::TextDelta("recovered".into())), Ok(ChatEvent::Done)],
        ];
        let (result, _, _, _) = run_agent(RunArgs { script, mode: ApprovalMode::Auto, tools: vec![], max_iterations: 5 }).await;
        let msgs = result.unwrap();
        match &msgs[1].parts[0] {
            ContentPart::ToolResult { content, is_error, .. } => {
                assert!(is_error);
                assert!(content.contains("unknown tool"), "{content}");
            }
            other => panic!("expected tool result, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn max_iterations_guard_trips() {
        let script = (0..5)
            .map(|_| vec![Ok(ChatEvent::ToolCall { id: "c".into(), name: "echo".into(), args_json: "{}".into() }), Ok(ChatEvent::Done)])
            .collect();
        let (result, events, _, _) = run_agent(RunArgs {
            script,
            mode: ApprovalMode::Auto,
            tools: vec![Box::new(EchoTool { needs_approval: false })],
            max_iterations: 2,
        })
        .await;
        assert!(result.is_err());
        assert!(events.iter().any(|e| matches!(e, AgentEvent::Error(_))));
    }

    #[tokio::test]
    async fn provider_error_event_aborts() {
        let script = vec![vec![Ok(ChatEvent::TextDelta("partial".into())), Ok(ChatEvent::Error("overloaded".into()))]];
        let (result, events, _, _) = run_agent(RunArgs { script, mode: ApprovalMode::Auto, tools: vec![], max_iterations: 5 }).await;
        assert!(result.is_err());
        assert!(events.iter().any(|e| matches!(e, AgentEvent::Error(_))));
    }

    #[tokio::test]
    async fn pre_cancelled_token_aborts() {
        let script = vec![vec![Ok(ChatEvent::TextDelta("x".into())), Ok(ChatEvent::Done)]];
        let provider = std::sync::Arc::new(MockProvider::new(script));
        let (events_tx, mut events_rx) = mpsc::channel(64);
        let broker = std::sync::Arc::new(ApprovalBroker::new(ApprovalMode::Auto, events_tx.clone()));
        let dir = tempfile::tempdir().unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();
        let req = AgentRequest {
            workspace_root: dir.path().to_path_buf(),
            provider,
            model: "m".into(),
            history: vec![Message::text(Role::User, "go")],
            tools: vec![],
            approvals: broker,
            events: events_tx,
            cancel,
            max_iterations: 5,
        };
        let result = run(req).await;
        assert!(matches!(result, Err(crate::core::error::Error::Cancelled)));
        let mut saw_cancelled = false;
        while let Ok(ev) = events_rx.try_recv() {
            if ev == AgentEvent::Cancelled {
                saw_cancelled = true;
            }
        }
        assert!(saw_cancelled);
    }

    #[tokio::test]
    async fn multiple_tool_calls_execute_in_order() {
        let script = vec![
            vec![
                Ok(ChatEvent::ToolCall { id: "c1".into(), name: "echo".into(), args_json: "{\"n\":1}".into() }),
                Ok(ChatEvent::ToolCall { id: "c2".into(), name: "echo".into(), args_json: "{\"n\":2}".into() }),
                Ok(ChatEvent::Done),
            ],
            vec![Ok(ChatEvent::TextDelta("both done".into())), Ok(ChatEvent::Done)],
        ];
        let (result, _, _, _) = run_agent(RunArgs { script, mode: ApprovalMode::Auto, tools: vec![Box::new(EchoTool { needs_approval: false })], max_iterations: 5 }).await;
        let msgs = result.unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(
            msgs[1].parts,
            vec![
                ContentPart::ToolResult { tool_call_id: "c1".into(), content: "echoed: {\"n\":1}".into(), is_error: false },
                ContentPart::ToolResult { tool_call_id: "c2".into(), content: "echoed: {\"n\":2}".into(), is_error: false },
            ]
        );
    }

    #[tokio::test]
    async fn manual_approval_allow_executes_tool() {
        let script = vec![
            vec![Ok(ChatEvent::ToolCall { id: "c1".into(), name: "echo".into(), args_json: "{}".into() }), Ok(ChatEvent::Done)],
            vec![Ok(ChatEvent::TextDelta("ok".into())), Ok(ChatEvent::Done)],
        ];
        let provider = std::sync::Arc::new(MockProvider::new(script));
        let (events_tx, mut events_rx) = mpsc::channel(64);
        let broker = std::sync::Arc::new(ApprovalBroker::new(ApprovalMode::Manual, events_tx.clone()));
        let dir = tempfile::tempdir().unwrap();
        let req = AgentRequest {
            workspace_root: dir.path().to_path_buf(),
            provider,
            model: "m".into(),
            history: vec![Message::text(Role::User, "go")],
            tools: vec![Box::new(EchoTool { needs_approval: true })],
            approvals: broker.clone(),
            events: events_tx,
            cancel: tokio_util::sync::CancellationToken::new(),
            max_iterations: 5,
        };
        let handle = tokio::spawn(run(req));
        while let Some(ev) = events_rx.recv().await {
            if let AgentEvent::ApprovalRequested { request_id, .. } = ev {
                broker.resolve(&request_id, true).unwrap();
                break;
            }
        }
        let msgs = handle.await.unwrap().unwrap();
        assert_eq!(
            msgs[1].parts,
            vec![ContentPart::ToolResult { tool_call_id: "c1".into(), content: "echoed: {}".into(), is_error: false }]
        );
    }

    #[tokio::test]
    async fn cancel_during_approval_wait_aborts() {
        let script = vec![
            vec![Ok(ChatEvent::ToolCall { id: "c1".into(), name: "echo".into(), args_json: "{}".into() }), Ok(ChatEvent::Done)],
        ];
        let provider = std::sync::Arc::new(MockProvider::new(script));
        let (events_tx, mut events_rx) = mpsc::channel(64);
        let broker = std::sync::Arc::new(ApprovalBroker::new(ApprovalMode::Manual, events_tx.clone()));
        let dir = tempfile::tempdir().unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        let req = AgentRequest {
            workspace_root: dir.path().to_path_buf(),
            provider,
            model: "m".into(),
            history: vec![Message::text(Role::User, "go")],
            tools: vec![Box::new(EchoTool { needs_approval: true })],
            approvals: broker,
            events: events_tx,
            cancel: cancel.clone(),
            max_iterations: 5,
        };
        let handle = tokio::spawn(run(req));
        // when the approval request arrives, cancel instead of resolving
        while let Some(ev) = events_rx.recv().await {
            if matches!(ev, AgentEvent::ApprovalRequested { .. }) {
                cancel.cancel();
                break;
            }
        }
        let result = handle.await.unwrap();
        assert!(matches!(result, Err(crate::core::error::Error::Cancelled)));
    }
}
