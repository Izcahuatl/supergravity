use crate::core::approvals::ApprovalBroker;
use crate::core::error::Error;
use crate::core::tools::{Tool, ToolContext};
use crate::core::types::{AgentEvent, ChatEvent, ContentPart, Message, Role};
use futures::StreamExt;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Detect a tool call the model wrote as TEXT instead of using the
/// function-calling channel (a known weak-model failure mode, e.g.
/// `{"tool": "name", "arguments": {...}}` or qwen's `<tool_call>` XML).
/// Matches only when the name is a registered tool and args parse as an object.
#[doc(hidden)] // exposed for unit tests
pub fn detect_text_tool_call(text: &str, tool_names: &[&str]) -> Option<(String, String)> {
    for (start, _) in text.match_indices('{') {
        let mut stream = serde_json::Deserializer::from_str(&text[start..]).into_iter::<serde_json::Value>();
        let Some(Ok(value)) = stream.next() else {
            continue;
        };
        let name = value.get("tool").or_else(|| value.get("name")).and_then(|n| n.as_str());
        let Some(name) = name else { continue };
        if !tool_names.contains(&name) {
            continue;
        }
        let args = value
            .get("arguments")
            .or_else(|| value.get("args"))
            .or_else(|| value.get("parameters"))
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        if !args.is_object() {
            continue;
        }
        return Some((name.to_string(), args.to_string()));
    }
    None
}

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
    /// Checkpoint sink for Rewind; None disables file backups.
    pub backup: Option<BackupCtx>,
}

/// Checkpoint sink: snapshots a file's bytes before a mutating tool changes
/// it, so a later Rewind restores the workspace alongside the history.
/// Best-effort — backup failures never abort a run.
pub struct BackupCtx {
    pub store: Arc<crate::core::store::Store>,
    pub conversation_id: String,
    /// The user message whose run these backups belong to.
    pub after_message_id: i64,
}

impl BackupCtx {
    fn record(&self, workspace_root: &std::path::Path, args_json: &str) {
        let path = serde_json::from_str::<serde_json::Value>(args_json)
            .ok()
            .and_then(|v| v.get("path")?.as_str().map(str::to_string));
        let Some(path) = path else { return };
        let Ok(abs) = crate::core::tools::resolve_in_workspace(workspace_root, &path) else {
            return;
        };
        // None when the file does not exist yet (restore = delete it).
        let content = std::fs::read(&abs).ok();
        if let Err(e) = self.store.add_file_backup(
            &self.conversation_id,
            self.after_message_id,
            &path,
            content.as_deref(),
        ) {
            eprintln!("supergravity: file backup failed for {path}: {e}");
        }
    }
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
                }) => calls.push((id, name, crate::core::types::sanitize_args_json(&args_json))),
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

        // Repair: the model wrote its tool call as text instead of using the
        // function-calling channel — convert it into a real call so the loop
        // still executes it (weak-model resilience).
        if calls.is_empty() && !text.is_empty() {
            let owned: Vec<String> = req.tools.iter().map(|t| t.spec().name).collect();
            let names: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
            if let Some((name, args_json)) = detect_text_tool_call(&text, &names) {
                calls.push((
                    format!("repair-{}", uuid::Uuid::new_v4()),
                    name,
                    crate::core::types::sanitize_args_json(&args_json),
                ));
            }
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
                    // Checkpoint the target before a mutating tool runs (Rewind).
                    if let Some(b) = &req.backup {
                        if matches!(name.as_str(), "write_file" | "edit_file") {
                            b.record(&req.workspace_root, &args_json);
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
            // Let live tool cards settle before the terminal event.
            for part in &tool_msg.parts {
                if let ContentPart::ToolResult { tool_call_id, content, .. } = part {
                    if content == "cancelled by user" {
                        let _ = req
                            .events
                            .send(AgentEvent::ToolCallFinished {
                                tool_call_id: tool_call_id.clone(),
                                ok: false,
                                summary: "cancelled by user".into(),
                            })
                            .await;
                    }
                }
            }
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

#[doc(hidden)] // exposed for tests/prompt_lab
pub fn system_prompt(
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
         How this works: you call tools by name; their results come back to you as tool-role messages. \
         Those results are outputs of YOUR tool calls, executed by the Supergravity runtime — \
         the user does not run tools or type commands themselves. \
         Never describe tool results as something the user did.\n\
         Act with tools, don't just describe: when the user asks you to create, change, run, find, \
         or read something, make the tool call immediately — never reply with only an explanation \
         of what you would do. If a file or command might not exist, try it and handle the error.\n\
         The ONLY tools that exist are the ones listed above — never invent tools (e.g. no \
         \"create_html_page\", no \"create_file\"). Never write tool-call JSON or XML as plain text; \
         issue tool calls through the function-calling mechanism only. \
         To create or change a file you MUST call write_file with path and content.\n\
         Example of a correct loop: you call run_shell {{\"command\": \"echo hello\"}}; a tool message returns \
         \"[output of run_shell \\\"echo hello\\\" — exit code 0]\\nhello\"; you then tell the user: \
         It printed: hello. The user ran nothing.\n\
         Rules: keep all file access inside the workspace root; read before you modify; \
         prefer small, targeted changes — for editing an existing file, use edit_file with an exact \
         old_string instead of rewriting the whole file with write_file; \
         when a tool returns an error, adapt or explain instead of retrying blindly.\n\
         Planning: for any task with 2+ steps, maintain the visible plan via update_plan — \
         call it FIRST with your steps (exactly one in_progress), update it as steps complete, \
         and mark every step done before your final summary. Skip it only for trivial one-liners.\n\
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
        backup: Option<BackupCtx>,
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
            backup: args.backup,
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

    /// A mutating tool call snapshots the target file into the checkpoint
    /// store before execution (Rewind support).
    #[tokio::test]
    async fn write_file_records_checkpoint_backup() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "original").unwrap();
        let store = std::sync::Arc::new(crate::core::store::Store::open_in_memory().unwrap());
        let ws = store
            .add_workspace("p", &dir.path().to_string_lossy())
            .unwrap();
        let cid = store
            .create_conversation(&ws, "c", "mock", "m", ApprovalMode::Auto)
            .unwrap();
        let provider = std::sync::Arc::new(MockProvider::new(vec![
            vec![
                Ok(ChatEvent::ToolCall {
                    id: "c1".into(),
                    name: "write_file".into(),
                    args_json: r#"{"path":"a.txt","content":"changed"}"#.into(),
                }),
                Ok(ChatEvent::Done),
            ],
            vec![Ok(ChatEvent::TextDelta("ok".into())), Ok(ChatEvent::Done)],
        ]));
        let (events_tx, mut events_rx) = mpsc::channel(64);
        let broker = std::sync::Arc::new(ApprovalBroker::new(ApprovalMode::Auto, events_tx.clone()));
        let req = AgentRequest {
            workspace_root: dir.path().to_path_buf(),
            provider: provider.clone(),
            model: "m".into(),
            history: vec![Message::text(Role::User, "go")],
            tools: crate::core::tools::default_tools(),
            approvals: broker,
            events: events_tx,
            cancel: tokio_util::sync::CancellationToken::new(),
            max_iterations: 5,
            backup: Some(BackupCtx {
                store: store.clone(),
                conversation_id: cid.clone(),
                after_message_id: 7,
            }),
        };
        let handle = tokio::spawn(run(req));
        while events_rx.recv().await.is_some() {}
        let outcome = handle.await.unwrap();
        assert!(outcome.error.is_none(), "{:?}", outcome.error);

        let backups = store.file_backups_from(&cid, 7).unwrap();
        assert_eq!(backups.len(), 1);
        assert_eq!(backups[0].0, "a.txt");
        assert_eq!(backups[0].1.as_deref(), Some(b"original".as_slice()));
        // The write itself still happened.
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "changed"
        );
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
            backup: None,
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

    #[test]
    fn detect_finds_json_tool_call() {
        let text = "Sure! {\"tool\": \"echo\", \"arguments\": {\"a\": 1}} done.";
        assert_eq!(
            detect_text_tool_call(text, &["echo"]),
            Some(("echo".to_string(), "{\"a\":1}".to_string()))
        );
    }

    #[test]
    fn detect_accepts_name_variant_and_defaults_args() {
        let text = "<tool_call>\n{\"name\": \"echo\", \"arguments\": {\"x\": true}}\n</tool_call>";
        assert_eq!(
            detect_text_tool_call(text, &["echo"]),
            Some(("echo".to_string(), "{\"x\":true}".to_string()))
        );
        let no_args = "{\"tool\": \"echo\"}";
        assert_eq!(
            detect_text_tool_call(no_args, &["echo"]),
            Some(("echo".to_string(), "{}".to_string()))
        );
    }

    #[test]
    fn detect_rejects_unknown_tool_and_non_tool_json() {
        assert_eq!(detect_text_tool_call("{\"tool\": \"nope\", \"arguments\": {}}", &["echo"]), None);
        assert_eq!(detect_text_tool_call("{\"name\": \"config.json\", \"version\": 2}", &["echo"]), None);
        assert_eq!(detect_text_tool_call("no json at all", &["echo"]), None);
        assert_eq!(detect_text_tool_call("{\"broken", &["echo"]), None);
    }

    #[tokio::test]
    async fn repair_executes_text_tool_call_and_continues() {
        // Model answers with a text-formatted tool call (no native tool_calls),
        // then a follow-up turn answers in text.
        let script = vec![
            vec![
                Ok(ChatEvent::TextDelta("Let me do that. {\"tool\": \"echo\", \"arguments\": {\"a\": 1}}".into())),
                Ok(ChatEvent::Done),
            ],
            vec![Ok(ChatEvent::TextDelta("done!".into())), Ok(ChatEvent::Done)],
        ];
        let (result, events, provider, _) = run_agent(RunArgs {
            script,
            mode: ApprovalMode::Auto,
            tools: vec![Box::new(EchoTool {
                needs_approval: false,
            })],
            max_iterations: 5,
            backup: None,
        })
        .await;
        let msgs = result.produced;
        assert_eq!(msgs.len(), 3, "assistant(call) + tool result + assistant(final): {msgs:?}");
        // The assistant message carries a repaired ToolCall part.
        match &msgs[0].parts[1] {
            ContentPart::ToolCall { id, name, args_json } => {
                assert!(id.starts_with("repair-"), "{id}");
                assert_eq!(name, "echo");
                assert_eq!(args_json, "{\"a\":1}");
            }
            other => panic!("expected repaired tool call, got {other:?}"),
        }
        assert_eq!(
            msgs[1].parts,
            vec![ContentPart::ToolResult {
                tool_call_id: match &msgs[0].parts[1] {
                    ContentPart::ToolCall { id, .. } => id.clone(),
                    _ => unreachable!(),
                },
                content: "echoed: {\"a\":1}".into(),
                is_error: false,
            }]
        );
        // Second provider call happened (loop continued) with the tool result in history.
        assert_eq!(provider.calls.lock().unwrap().len(), 2);
        assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolCallProposed { name, .. } if name == "echo")));
    }

    #[tokio::test]
    async fn repair_skips_unknown_tool_name() {
        let script = vec![vec![
            Ok(ChatEvent::TextDelta("{\"tool\": \"not_a_tool\", \"arguments\": {}}".into())),
            Ok(ChatEvent::Done),
        ]];
        let (result, _, provider, _) = run_agent(RunArgs {
            script,
            mode: ApprovalMode::Auto,
            tools: vec![Box::new(EchoTool {
                needs_approval: false,
            })],
            max_iterations: 5,
            backup: None,
        })
        .await;
        assert_eq!(result.produced.len(), 1, "plain text answer, no repair");
        assert!(result.error.is_none());
        assert_eq!(provider.calls.lock().unwrap().len(), 1);
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
            backup: None,
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
            backup: None,
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
            backup: None,
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
            backup: None,
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
            backup: None,
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
            backup: None,
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
            backup: None,
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
            backup: None,
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
            backup: None,
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
            backup: None,
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
            backup: None,
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
            backup: None,
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
