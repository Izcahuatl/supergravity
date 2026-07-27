//! Prompt lab: runs the real agent loop against a live Ollama server and dumps
//! full transcripts — outgoing request bodies (per iteration) and incoming
//! thinking/text/tool events — so prompt/serialization changes can be compared
//! against data, not vibes.
//!
//! Run: cargo test --test prompt_lab -- --ignored --nocapture

use supergravity::core::agent::{self, AgentRequest, DEFAULT_MAX_ITERATIONS};
use supergravity::core::approvals::ApprovalBroker;
use supergravity::core::providers::ollama::{self, OllamaProvider};
use supergravity::core::providers::Provider;
use supergravity::core::tools::default_tools;
use supergravity::core::types::{AgentEvent, ApprovalMode, Message, Role};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

fn model() -> String {
    std::env::var("LAB_MODEL").unwrap_or_else(|_| "qwen3:0.6b".into())
}

struct Lab {
    workspace: tempfile::TempDir,
    events: Vec<AgentEvent>,
    bodies: Vec<serde_json::Value>,
}

impl Lab {
    fn new() -> Self {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("main.rs"), "fn main() { println!(\"hi\"); }\n").unwrap();
        std::fs::write(workspace.path().join("notes.txt"), "some notes\n").unwrap();
        Lab { workspace, events: vec![], bodies: vec![] }
    }

    /// Run one user message through the real agent loop with a transcript tap.
    async fn run_turn(&mut self, history: &[Message], user_text: &str) -> Vec<Message> {
        let mut messages = history.to_vec();
        messages.push(Message::text(Role::User, user_text));

        let provider: Arc<dyn Provider> = Arc::new(OllamaProvider::new(None));
        // Record what the agent will send on iteration 1 (system prompt + history).
        let mut with_system = vec![Message::text(
            Role::System,
            agent::system_prompt(self.workspace.path(), ApprovalMode::Auto, &default_tools(), None),
        )];
        with_system.extend(messages.iter().cloned());
        self.bodies.push(ollama::build_body(&model(), &with_system, &default_tools_specs()));

        let (events_tx, mut events_rx) = mpsc::channel::<AgentEvent>(4096);
        let broker = Arc::new(ApprovalBroker::new(ApprovalMode::Auto, events_tx.clone()));
        let req = AgentRequest {
            workspace_root: self.workspace.path().to_path_buf(),
            provider,
            model: model(),
            history: messages,
            tools: default_tools(),
            approvals: broker,
            events: events_tx,
            cancel: CancellationToken::new(),
            max_iterations: DEFAULT_MAX_ITERATIONS,
            backup: None,
            workshop_root: None,
        };
        let handle = tokio::spawn(agent::run(req));
        while let Some(ev) = events_rx.recv().await {
            self.events.push(ev);
        }
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(300), handle)
            .await
            .expect("timeout")
            .unwrap();
        if let Some(err) = outcome.error {
            println!("!! run error: {err}");
        }
        outcome.produced
    }

    fn print_events(&self) {
        let mut buf = String::new();
        for ev in &self.events {
            match ev {
                AgentEvent::TextDelta(d) => buf.push_str(d),
                AgentEvent::ToolCallProposed { name, args_json, .. } => {
                    if !buf.is_empty() {
                        println!("[text] {buf}");
                        buf.clear();
                    }
                    println!("[tool-call] {name} {args_json}");
                }
                AgentEvent::ToolCallFinished { ok, summary, .. } => {
                    println!("[tool-{}] {}", if *ok { "ok" } else { "fail" }, summary.replace('\n', " | "));
                }
                AgentEvent::MessageDone => {
                    if !buf.is_empty() {
                        println!("[text] {buf}");
                        buf.clear();
                    }
                    println!("[done]");
                }
                AgentEvent::Error(e) => println!("[error] {e}"),
                AgentEvent::Cancelled => println!("[cancelled]"),
                AgentEvent::ApprovalRequested { name, .. } => println!("[approval-req] {name}"),
            }
        }
        if !buf.is_empty() {
            println!("[text] {buf}");
        }
    }
}

fn default_tools_specs() -> Vec<supergravity::core::types::ToolSpec> {
    default_tools().iter().map(|t| t.spec()).collect()
}

fn print_bodies(lab: &Lab) {
    for (i, body) in lab.bodies.iter().enumerate() {
        println!("=== outgoing body (iteration {i}) ===");
        println!("{}", serde_json::to_string_pretty(body).unwrap());
    }
}

#[tokio::test]
#[ignore = "needs a local Ollama server with qwen3:0.6b"]
async fn lab_simple_tool_task() {
    let mut lab = Lab::new();
    println!(">>> turn 1: 'List the files in this workspace using the list_dir tool.'");
    let produced = lab.run_turn(&[], "List the files in this workspace using the list_dir tool.").await;
    lab.print_events();
    println!("\n>>> follow-up: 'What did you just do? Who listed the files?'");
    let history: Vec<Message> = produced;
    lab.events.clear();
    lab.run_turn(&history, "What did you just do? Who listed the files?").await;
    lab.print_events();
    print_bodies(&lab);
}

#[tokio::test]
#[ignore = "needs a local Ollama server with qwen3:0.6b"]
async fn lab_shell_task() {
    let mut lab = Lab::new();
    println!(">>> turn 1: 'Use run_shell to print hello.'");
    let produced = lab.run_turn(&[], "Use the run_shell tool to print the word hello, then tell me what it printed.").await;
    lab.print_events();
    println!("\n>>> follow-up: 'Who ran that command?'");
    lab.events.clear();
    lab.run_turn(&produced, "Who ran that command — you or me?").await;
    lab.print_events();
    print_bodies(&lab);
}
