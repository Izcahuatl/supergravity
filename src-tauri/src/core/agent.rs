use crate::core::approvals::ApprovalBroker;
use crate::core::error::Error;
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

/// Result of one agent run: the messages produced (persist these even on
/// failure) plus the error that ended the run, if any.
pub struct AgentOutcome {
    /// Messages produced during the run — persist these even when `error` is Some.
    pub produced: Vec<Message>,
    /// The failure that ended the run, if any.
    pub error: Option<Error>,
}

/// Run the tool-call loop until the model stops calling tools.
/// Returns an [`AgentOutcome`] with the messages produced during this run
/// (assistant + tool messages), which the caller persists to the store.
pub async fn run(req: AgentRequest) -> AgentOutcome {
    // System prompt is built per-run and NOT persisted to history.
    let mut messages = Vec::with_capacity(req.history.len() + 1);
    messages.push(Message::text(
        Role::System,
        system_prompt(&req.workspace_root, req.approvals.mode(), &req.tools),
    ));
    messages.extend(req.history.iter().cloned());
    let mut produced: Vec<Message> = Vec::new();

    for _ in 0..req.max_iterations {
        if req.cancel.is_cancelled() {
            let _ = req.events.send(AgentEvent::Cancelled).await;
            return AgentOutcome {
                produced,
                error: Some(Error::Cancelled),
            };
        }

        let tool_specs: Vec<crate::core::types::ToolSpec> =
            req.tools.iter().map(|t| t.spec()).collect();
        let mut stream = match req
            .provider
            .stream_chat(&req.model, &messages, &tool_specs)
            .await
        {
            Ok(s) => s,
            Err(e) => {
                let _ = req.events.send(AgentEvent::Error(e.to_string())).await;
                return AgentOutcome {
                    produced,
                    error: Some(e),
                };
            }
        };

        let mut text = String::new();
        let mut calls: Vec<(String, String, String)> = Vec::new(); // (id, name, args_json)
        let mut stream_err: Option<Error> = None;

        while let Some(item) = stream.next().await {
            if req.cancel.is_cancelled() {
                let _ = req.events.send(AgentEvent::Cancelled).await;
                return AgentOutcome {
                    produced,
                    error: Some(Error::Cancelled),
                };
            }
            match item {
                Ok(ChatEvent::TextDelta(d)) => {
                    text.push_str(&d);
                    let _ = req.events.send(AgentEvent::TextDelta(d)).await;
                }
                Ok(ChatEvent::ToolCall {
                    id,
                    name,
                    args_json,
                }) => calls.push((id, name, args_json)),
                Ok(ChatEvent::Usage { .. }) => {}
                Ok(ChatEvent::Error(msg)) => {
                    stream_err = Some(Error::Provider {
                        status: 0,
                        body: msg,
                    });
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
            return AgentOutcome {
                produced,
                error: Some(e),
            };
        }

        let mut parts: Vec<ContentPart> = Vec::new();
        if !text.is_empty() {
            parts.push(ContentPart::Text { text });
        }
        for (id, name, args_json) in &calls {
            parts.push(ContentPart::ToolCall {
                id: id.clone(),
                name: name.clone(),
                args_json: args_json.clone(),
            });
        }
        let assistant = Message {
            role: Role::Assistant,
            parts,
        };
        messages.push(assistant.clone());
        produced.push(assistant);

        if calls.is_empty() {
            let _ = req.events.send(AgentEvent::MessageDone).await;
            return AgentOutcome {
                produced,
                error: None,
            };
        }

        let ctx = ToolContext {
            workspace_root: req.workspace_root.clone(),
        };
        let mut results: Vec<ContentPart> = Vec::new();
        let mut iter = calls.into_iter();
        let mut cancelled = false;
        while let Some((id, name, args_json)) = iter.next() {
            if req.cancel.is_cancelled() {
                // Close out this and all remaining calls so persisted history
                // stays protocol-valid (every tool_call needs a tool_result).
                results.push(ContentPart::ToolResult {
                    tool_call_id: id.clone(),
                    content: "cancelled by user".into(),
                    is_error: true,
                });
                for (rid, _, _) in iter.by_ref() {
                    results.push(ContentPart::ToolResult {
                        tool_call_id: rid,
                        content: "cancelled by user".into(),
                        is_error: true,
                    });
                }
                cancelled = true;
                break;
            }
            let _ = req
                .events
                .send(AgentEvent::ToolCallProposed {
                    tool_call_id: id.clone(),
                    name: name.clone(),
                    args_json: args_json.clone(),
                })
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
                    ContentPart::ToolResult {
                        tool_call_id: id,
                        content: format!("unknown tool: {name}"),
                        is_error: true,
                    }
                }
                Some(t) => {
                    if t.needs_approval() {
                        // Cancel must interrupt the approval wait, not just iterations.
                        let decision = tokio::select! {
                            _ = req.cancel.cancelled() => None,
                            res = req.approvals.check(&id, &name, &args_json) => Some(res),
                        };
                        let Some(decision) = decision else {
                            results.push(ContentPart::ToolResult {
                                tool_call_id: id.clone(),
                                content: "cancelled by user".into(),
                                is_error: true,
                            });
                            for (rid, _, _) in iter.by_ref() {
                                results.push(ContentPart::ToolResult {
                                    tool_call_id: rid,
                                    content: "cancelled by user".into(),
                                    is_error: true,
                                });
                            }
                            cancelled = true;
                            break;
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
                                let _ = req
                                    .events
                                    .send(AgentEvent::ToolCallFinished {
                                        tool_call_id: id.clone(),
                                        ok: false,
                                        summary: format!("approval error: {e}"),
                                    })
                                    .await;
                                results.push(ContentPart::ToolResult {
                                    tool_call_id: id,
                                    content: format!("approval error: {e}"),
                                    is_error: true,
                                });
                                continue;
                            }
                        }
                    }
                    // Execute with cancellation — a hung tool (up to 300s shell
                    // timeout) must not ignore Stop.
                    let exec_result = tokio::select! {
                        _ = req.cancel.cancelled() => None,
                        res = t.execute(&ctx, &args_json) => Some(res),
                    };
                    let Some(exec_result) = exec_result else {
                        results.push(ContentPart::ToolResult {
                            tool_call_id: id.clone(),
                            content: "cancelled by user".into(),
                            is_error: true,
                        });
                        for (rid, _, _) in iter.by_ref() {
                            results.push(ContentPart::ToolResult {
                                tool_call_id: rid,
                                content: "cancelled by user".into(),
                                is_error: true,
                            });
                        }
                        cancelled = true;
                        break;
                    };
                    match exec_result {
                        Ok(output) => {
                            let summary: String = output.chars().take(80).collect();
                            let _ = req
                                .events
                                .send(AgentEvent::ToolCallFinished {
                                    tool_call_id: id.clone(),
                                    ok: true,
                                    summary,
                                })
                                .await;
                            ContentPart::ToolResult {
                                tool_call_id: id,
                                content: output,
                                is_error: false,
                            }
                        }
                        Err(e) => {
                            let _ = req
                                .events
                                .send(AgentEvent::ToolCallFinished {
                                    tool_call_id: id.clone(),
                                    ok: false,
                                    summary: e.to_string(),
                                })
                                .await;
                            ContentPart::ToolResult {
                                tool_call_id: id,
                                content: e.to_string(),
                                is_error: true,
                            }
                        }
                    }
                }
            };
            results.push(result);
        }
        let tool_msg = Message {
            role: Role::Tool,
            parts: results,
        };
        produced.push(tool_msg.clone());
        if cancelled {
            let _ = req.events.send(AgentEvent::Cancelled).await;
            return AgentOutcome {
                produced,
                error: Some(Error::Cancelled),
            };
        }
        messages.push(tool_msg);
    }

    let err = Error::Tool(format!("max iterations ({}) reached", req.max_iterations));
    let _ = req.events.send(AgentEvent::Error(err.to_string())).await;
    AgentOutcome {
        produced,
        error: Some(err),
    }
}

fn system_prompt(
    workspace_root: &std::path::Path,
    mode: crate::core::types::ApprovalMode,
    tools: &[Box<dyn Tool>],
) -> String {
    let tool_names: Vec<String> = tools.iter().map(|t| t.spec().name).collect();
    let mode_note = match mode {
        crate::core::types::ApprovalMode::Manual => {
            "File writes and shell commands require explicit user approval before executing."
        }
        crate::core::types::ApprovalMode::Auto => {
            "You may write files and run shell commands without user approval."
        }
    };
    format!(
        "You are Supergravity, a coding agent.\n\
         Workspace root: {}\n\
         Available tools: {}.\n\
         Rules: keep all file access inside the workspace root; read before you modify; \
         prefer small, targeted changes; when a tool returns an error, adapt or explain instead of retrying blindly.\n\
         {}",
        workspace_root.display(),
        tool_names.join(", "),
        mode_note
    )
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
    struct EchoTool {
        needs_approval: bool,
    }

    #[async_trait::async_trait]
    impl Tool for EchoTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "echo".into(),
                description: "echo args".into(),
                params_schema: serde_json::json!({"type": "object"}),
            }
        }
        fn needs_approval(&self) -> bool {
            self.needs_approval
        }
        async fn execute(
            &self,
            _ctx: &ToolContext,
            args_json: &str,
        ) -> crate::core::error::Result<String> {
            Ok(format!("echoed: {args_json}"))
        }
    }

    /// Test tool that hangs for 30s — cancel must interrupt it mid-execute.
    struct SleepTool;

    #[async_trait::async_trait]
    impl Tool for SleepTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "sleep".into(),
                description: "sleep for 30s".into(),
                params_schema: serde_json::json!({"type": "object"}),
            }
        }
        fn needs_approval(&self) -> bool {
            false
        }
        async fn execute(
            &self,
            _ctx: &ToolContext,
            _args_json: &str,
        ) -> crate::core::error::Result<String> {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            Ok("slept".into())
        }
    }

    struct RunArgs {
        script: Vec<Vec<crate::core::error::Result<ChatEvent>>>,
        mode: ApprovalMode,
        tools: Vec<Box<dyn Tool>>,
        max_iterations: usize,
    }

    async fn run_agent(
        args: RunArgs,
    ) -> (
        AgentOutcome,
        Vec<AgentEvent>,
        std::sync::Arc<MockProvider>,
        std::sync::Arc<ApprovalBroker>,
    ) {
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
            let done = matches!(
                ev,
                AgentEvent::MessageDone | AgentEvent::Error(_) | AgentEvent::Cancelled
            );
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
        let script = vec![vec![
            Ok(ChatEvent::TextDelta("hi ".into())),
            Ok(ChatEvent::TextDelta("there".into())),
            Ok(ChatEvent::Done),
        ]];
        let (result, events, _, _) = run_agent(RunArgs {
            script,
            mode: ApprovalMode::Auto,
            tools: vec![],
            max_iterations: 5,
        })
        .await;
        let msgs = result.produced;
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, Role::Assistant);
        assert_eq!(
            msgs[0].parts,
            vec![ContentPart::Text {
                text: "hi there".into()
            }]
        );
        assert!(events.contains(&AgentEvent::TextDelta("hi ".into())));
        assert!(events.contains(&AgentEvent::MessageDone));
    }

    #[tokio::test]
    async fn system_prompt_prepended_not_persisted() {
        let script = vec![vec![
            Ok(ChatEvent::TextDelta("hi".into())),
            Ok(ChatEvent::Done),
        ]];
        let (result, _, provider, _) = run_agent(RunArgs {
            script,
            mode: ApprovalMode::Auto,
            tools: vec![],
            max_iterations: 5,
        })
        .await;
        assert!(result.error.is_none());
        let calls = provider.calls.lock().unwrap();
        assert_eq!(
            calls[0].1[0].role,
            Role::System,
            "first message sent to the provider is the system prompt"
        );
        assert!(
            matches!(calls[0].1[0].parts[0], ContentPart::Text { .. }),
            "system prompt is a text part"
        );
        assert_eq!(
            calls[0].1[1].role,
            Role::User,
            "history follows the system prompt"
        );
        drop(calls);
        assert_eq!(
            result.produced[0].role,
            Role::Assistant,
            "system prompt must not appear in produced messages"
        );
    }

    #[tokio::test]
    async fn tool_cycle_appends_results_and_continues() {
        let script = vec![
            vec![
                Ok(ChatEvent::ToolCall {
                    id: "c1".into(),
                    name: "echo".into(),
                    args_json: "{\"a\":1}".into(),
                }),
                Ok(ChatEvent::Done),
            ],
            vec![
                Ok(ChatEvent::TextDelta("done!".into())),
                Ok(ChatEvent::Done),
            ],
        ];
        let (result, events, provider, _) = run_agent(RunArgs {
            script,
            mode: ApprovalMode::Auto,
            tools: vec![Box::new(EchoTool {
                needs_approval: false,
            })],
            max_iterations: 5,
        })
        .await;
        let msgs = result.produced;
        assert_eq!(
            msgs.len(),
            3,
            "assistant(call) + tool result + assistant(final): {msgs:?}"
        );
        assert_eq!(msgs[1].role, Role::Tool);
        assert_eq!(
            msgs[1].parts,
            vec![ContentPart::ToolResult {
                tool_call_id: "c1".into(),
                content: "echoed: {\"a\":1}".into(),
                is_error: false
            }]
        );
        // second provider call must include the tool result in history
        let calls = provider.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls[1].1.iter().any(|m| m.role == Role::Tool));
        assert!(events.contains(&AgentEvent::ToolCallProposed {
            tool_call_id: "c1".into(),
            name: "echo".into(),
            args_json: "{\"a\":1}".into()
        }));
        assert!(events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolCallFinished { ok: true, .. })));
    }

    #[tokio::test]
    async fn denied_approval_becomes_error_tool_result() {
        let script = vec![
            vec![
                Ok(ChatEvent::ToolCall {
                    id: "c1".into(),
                    name: "echo".into(),
                    args_json: "{}".into(),
                }),
                Ok(ChatEvent::Done),
            ],
            vec![Ok(ChatEvent::TextDelta("ok".into())), Ok(ChatEvent::Done)],
        ];
        let provider = std::sync::Arc::new(MockProvider::new(script));
        let (events_tx, mut events_rx) = mpsc::channel(64);
        let broker =
            std::sync::Arc::new(ApprovalBroker::new(ApprovalMode::Manual, events_tx.clone()));
        let dir = tempfile::tempdir().unwrap();
        let req = AgentRequest {
            workspace_root: dir.path().to_path_buf(),
            provider: provider.clone(),
            model: "m".into(),
            history: vec![Message::text(Role::User, "go")],
            tools: vec![Box::new(EchoTool {
                needs_approval: true,
            })],
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
        let msgs = handle.await.unwrap().produced;
        assert_eq!(
            msgs[1].parts,
            vec![ContentPart::ToolResult {
                tool_call_id: "c1".into(),
                content: "user denied this action".into(),
                is_error: true
            }]
        );
    }

    #[tokio::test]
    async fn unknown_tool_is_error_result_not_crash() {
        let script = vec![
            vec![
                Ok(ChatEvent::ToolCall {
                    id: "c1".into(),
                    name: "nope".into(),
                    args_json: "{}".into(),
                }),
                Ok(ChatEvent::Done),
            ],
            vec![
                Ok(ChatEvent::TextDelta("recovered".into())),
                Ok(ChatEvent::Done),
            ],
        ];
        let (result, _, _, _) = run_agent(RunArgs {
            script,
            mode: ApprovalMode::Auto,
            tools: vec![],
            max_iterations: 5,
        })
        .await;
        let msgs = result.produced;
        match &msgs[1].parts[0] {
            ContentPart::ToolResult {
                content, is_error, ..
            } => {
                assert!(is_error);
                assert!(content.contains("unknown tool"), "{content}");
            }
            other => panic!("expected tool result, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn max_iterations_guard_trips() {
        let script = (0..5)
            .map(|_| {
                vec![
                    Ok(ChatEvent::ToolCall {
                        id: "c".into(),
                        name: "echo".into(),
                        args_json: "{}".into(),
                    }),
                    Ok(ChatEvent::Done),
                ]
            })
            .collect();
        let (result, events, _, _) = run_agent(RunArgs {
            script,
            mode: ApprovalMode::Auto,
            tools: vec![Box::new(EchoTool {
                needs_approval: false,
            })],
            max_iterations: 2,
        })
        .await;
        assert!(result.error.is_some());
        assert!(events.iter().any(|e| matches!(e, AgentEvent::Error(_))));
    }

    #[tokio::test]
    async fn provider_error_event_aborts() {
        let script = vec![vec![
            Ok(ChatEvent::TextDelta("partial".into())),
            Ok(ChatEvent::Error("overloaded".into())),
        ]];
        let (result, events, _, _) = run_agent(RunArgs {
            script,
            mode: ApprovalMode::Auto,
            tools: vec![],
            max_iterations: 5,
        })
        .await;
        assert!(result.error.is_some());
        assert!(events.iter().any(|e| matches!(e, AgentEvent::Error(_))));
    }

    #[tokio::test]
    async fn pre_cancelled_token_aborts() {
        let script = vec![vec![
            Ok(ChatEvent::TextDelta("x".into())),
            Ok(ChatEvent::Done),
        ]];
        let provider = std::sync::Arc::new(MockProvider::new(script));
        let (events_tx, mut events_rx) = mpsc::channel(64);
        let broker =
            std::sync::Arc::new(ApprovalBroker::new(ApprovalMode::Auto, events_tx.clone()));
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
        assert!(matches!(
            result.error,
            Some(crate::core::error::Error::Cancelled)
        ));
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
                Ok(ChatEvent::ToolCall {
                    id: "c1".into(),
                    name: "echo".into(),
                    args_json: "{\"n\":1}".into(),
                }),
                Ok(ChatEvent::ToolCall {
                    id: "c2".into(),
                    name: "echo".into(),
                    args_json: "{\"n\":2}".into(),
                }),
                Ok(ChatEvent::Done),
            ],
            vec![
                Ok(ChatEvent::TextDelta("both done".into())),
                Ok(ChatEvent::Done),
            ],
        ];
        let (result, _, _, _) = run_agent(RunArgs {
            script,
            mode: ApprovalMode::Auto,
            tools: vec![Box::new(EchoTool {
                needs_approval: false,
            })],
            max_iterations: 5,
        })
        .await;
        let msgs = result.produced;
        assert_eq!(msgs.len(), 3);
        assert_eq!(
            msgs[1].parts,
            vec![
                ContentPart::ToolResult {
                    tool_call_id: "c1".into(),
                    content: "echoed: {\"n\":1}".into(),
                    is_error: false
                },
                ContentPart::ToolResult {
                    tool_call_id: "c2".into(),
                    content: "echoed: {\"n\":2}".into(),
                    is_error: false
                },
            ]
        );
    }

    #[tokio::test]
    async fn manual_approval_allow_executes_tool() {
        let script = vec![
            vec![
                Ok(ChatEvent::ToolCall {
                    id: "c1".into(),
                    name: "echo".into(),
                    args_json: "{}".into(),
                }),
                Ok(ChatEvent::Done),
            ],
            vec![Ok(ChatEvent::TextDelta("ok".into())), Ok(ChatEvent::Done)],
        ];
        let provider = std::sync::Arc::new(MockProvider::new(script));
        let (events_tx, mut events_rx) = mpsc::channel(64);
        let broker =
            std::sync::Arc::new(ApprovalBroker::new(ApprovalMode::Manual, events_tx.clone()));
        let dir = tempfile::tempdir().unwrap();
        let req = AgentRequest {
            workspace_root: dir.path().to_path_buf(),
            provider,
            model: "m".into(),
            history: vec![Message::text(Role::User, "go")],
            tools: vec![Box::new(EchoTool {
                needs_approval: true,
            })],
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
        let msgs = handle.await.unwrap().produced;
        assert_eq!(
            msgs[1].parts,
            vec![ContentPart::ToolResult {
                tool_call_id: "c1".into(),
                content: "echoed: {}".into(),
                is_error: false
            }]
        );
    }

    #[tokio::test]
    async fn cancel_during_approval_wait_aborts() {
        let script = vec![vec![
            Ok(ChatEvent::ToolCall {
                id: "c1".into(),
                name: "echo".into(),
                args_json: "{}".into(),
            }),
            Ok(ChatEvent::Done),
        ]];
        let provider = std::sync::Arc::new(MockProvider::new(script));
        let (events_tx, mut events_rx) = mpsc::channel(64);
        let broker =
            std::sync::Arc::new(ApprovalBroker::new(ApprovalMode::Manual, events_tx.clone()));
        let dir = tempfile::tempdir().unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        let req = AgentRequest {
            workspace_root: dir.path().to_path_buf(),
            provider,
            model: "m".into(),
            history: vec![Message::text(Role::User, "go")],
            tools: vec![Box::new(EchoTool {
                needs_approval: true,
            })],
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
        assert!(matches!(
            result.error,
            Some(crate::core::error::Error::Cancelled)
        ));
    }

    #[tokio::test]
    async fn cancel_during_tool_execute_aborts() {
        let script = vec![vec![
            Ok(ChatEvent::ToolCall {
                id: "c1".into(),
                name: "sleep".into(),
                args_json: "{}".into(),
            }),
            Ok(ChatEvent::Done),
        ]];
        let provider = std::sync::Arc::new(MockProvider::new(script));
        let (events_tx, mut events_rx) = mpsc::channel(64);
        let broker =
            std::sync::Arc::new(ApprovalBroker::new(ApprovalMode::Auto, events_tx.clone()));
        let dir = tempfile::tempdir().unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        let req = AgentRequest {
            workspace_root: dir.path().to_path_buf(),
            provider,
            model: "m".into(),
            history: vec![Message::text(Role::User, "go")],
            tools: vec![Box::new(SleepTool)],
            approvals: broker,
            events: events_tx,
            cancel: cancel.clone(),
            max_iterations: 5,
        };
        let handle = tokio::spawn(run(req));
        // Cancel ~100ms in, while the 30s SleepTool is mid-execute.
        let canceller = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            canceller.cancel();
        });
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), handle)
            .await
            .expect("run must return within 5s of cancellation")
            .unwrap();
        assert!(matches!(
            result.error,
            Some(crate::core::error::Error::Cancelled)
        ));
        let mut saw_cancelled = false;
        while let Ok(ev) = events_rx.try_recv() {
            if ev == AgentEvent::Cancelled {
                saw_cancelled = true;
            }
        }
        assert!(saw_cancelled);
    }

    #[tokio::test]
    async fn cancel_during_tool_execute_produces_valid_history() {
        // One turn with TWO tool calls: echo (instant) then sleep (30s).
        let script = vec![vec![
            Ok(ChatEvent::ToolCall {
                id: "c1".into(),
                name: "echo".into(),
                args_json: "{\"a\":1}".into(),
            }),
            Ok(ChatEvent::ToolCall {
                id: "c2".into(),
                name: "sleep".into(),
                args_json: "{}".into(),
            }),
            Ok(ChatEvent::Done),
        ]];
        let provider = std::sync::Arc::new(MockProvider::new(script));
        let (events_tx, mut events_rx) = mpsc::channel(64);
        let broker =
            std::sync::Arc::new(ApprovalBroker::new(ApprovalMode::Auto, events_tx.clone()));
        let dir = tempfile::tempdir().unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        let req = AgentRequest {
            workspace_root: dir.path().to_path_buf(),
            provider,
            model: "m".into(),
            history: vec![Message::text(Role::User, "go")],
            tools: vec![
                Box::new(EchoTool {
                    needs_approval: false,
                }),
                Box::new(SleepTool),
            ],
            approvals: broker,
            events: events_tx,
            cancel: cancel.clone(),
            max_iterations: 5,
        };
        let handle = tokio::spawn(run(req));
        // Cancel ~100ms in: c1 has echoed, c2 (sleep) is mid-execute.
        let canceller = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            canceller.cancel();
        });
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), handle)
            .await
            .expect("run must return within 5s")
            .unwrap();
        assert!(matches!(
            result.error,
            Some(crate::core::error::Error::Cancelled)
        ));
        // Persisted history must stay protocol-valid: every assistant
        // tool_call gets a matching tool_result, even on cancel.
        let tool_msg = result
            .produced
            .iter()
            .find(|m| m.role == Role::Tool)
            .expect("a tool message must be produced even when cancelled");
        assert_eq!(
            tool_msg.parts,
            vec![
                ContentPart::ToolResult {
                    tool_call_id: "c1".into(),
                    content: "echoed: {\"a\":1}".into(),
                    is_error: false
                },
                ContentPart::ToolResult {
                    tool_call_id: "c2".into(),
                    content: "cancelled by user".into(),
                    is_error: true
                },
            ]
        );
        let mut saw_cancelled = false;
        while let Ok(ev) = events_rx.try_recv() {
            if ev == AgentEvent::Cancelled {
                saw_cancelled = true;
            }
        }
        assert!(saw_cancelled);
    }
}
