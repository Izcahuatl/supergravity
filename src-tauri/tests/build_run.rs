//! Build run: drive the real agent loop on a real folder (B:\test\shit),
//! logging every event live to B:\test\chat-log.txt.
//!
//! Run: LAB_MODEL=qwen3:4b cargo test --test build_run -- --ignored --nocapture

use supergravity::core::agent::{self, AgentRequest, DEFAULT_MAX_ITERATIONS};
use supergravity::core::approvals::ApprovalBroker;
use supergravity::core::providers::ollama::OllamaProvider;
use supergravity::core::providers::Provider;
use supergravity::core::tools::default_tools;
use supergravity::core::types::{AgentEvent, ApprovalMode, Message, Role};
use std::io::Write;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

fn model() -> String {
    std::env::var("LAB_MODEL").unwrap_or_else(|_| "qwen3:4b".into())
}

const WORKSPACE: &str = "B:\\test\\shit";
const LOG: &str = "B:\\test\\chat-log2.txt";

const TASK: &str = "Create a file index.html in this workspace: a simple dark-themed landing page for a fictional app called 'Supergravity'. It needs a title, a subtitle, and exactly 3 feature cards. Inline CSS only, no JavaScript. After writing it, use list_dir to confirm the file is there, then tell me what you made.";

struct Logger {
    file: std::fs::File,
    text_buf: String,
}

impl Logger {
    fn new() -> Self {
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(LOG).unwrap();
        let _ = writeln!(file, "\n============================================================");
        let _ = writeln!(file, "BUILD RUN — model: {} — task:", model());
        let _ = writeln!(file, "{TASK}");
        let _ = writeln!(file, "============================================================\n");
        Logger { file, text_buf: String::new() }
    }

    fn flush_text(&mut self) {
        if !self.text_buf.is_empty() {
            let _ = writeln!(self.file, "[assistant] {}", self.text_buf.trim_end());
            self.text_buf.clear();
        }
    }

    fn event(&mut self, ev: &AgentEvent) {
        match ev {
            AgentEvent::TextDelta(d) => self.text_buf.push_str(d),
            AgentEvent::ToolCallProposed { name, args_json, .. } => {
                self.flush_text();
                let _ = writeln!(self.file, "\n[tool-call] {name} {args_json}");
            }
            AgentEvent::ApprovalRequested { name, args_json, .. } => {
                self.flush_text();
                let _ = writeln!(self.file, "[approval-request] {name} {args_json}");
            }
            AgentEvent::ToolCallFinished { ok, summary, .. } => {
                let _ = writeln!(self.file, "[tool-{}] {}", if *ok { "ok" } else { "FAIL" }, summary);
            }
            AgentEvent::MessageDone => {
                self.flush_text();
                let _ = writeln!(self.file, "\n[message done]");
            }
            AgentEvent::Error(e) => {
                self.flush_text();
                let _ = writeln!(self.file, "[ERROR] {e}");
            }
            AgentEvent::Cancelled => {
                self.flush_text();
                let _ = writeln!(self.file, "[cancelled]");
            }
        }
    }
}

#[tokio::test]
#[ignore = "needs a local Ollama server; writes to B:\\test"]
async fn build_something() {
    std::fs::create_dir_all(WORKSPACE).unwrap();
    let mut logger = Logger::new();

    let provider: Arc<dyn Provider> = Arc::new(OllamaProvider::new(None));
    let (events_tx, mut events_rx) = mpsc::channel::<AgentEvent>(4096);
    let broker = Arc::new(ApprovalBroker::new(ApprovalMode::Auto, events_tx.clone()));
    let req = AgentRequest {
        workspace_root: WORKSPACE.into(),
        provider,
        model: model(),
        history: vec![Message::text(Role::User, TASK)],
        tools: default_tools(),
        approvals: broker,
        events: events_tx,
        cancel: CancellationToken::new(),
        max_iterations: DEFAULT_MAX_ITERATIONS,
        backup: None,
    };
    let handle = tokio::spawn(agent::run(req));
    while let Some(ev) = events_rx.recv().await {
        logger.event(&ev);
    }
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(600), handle)
        .await
        .expect("build timeout")
        .unwrap();

    match &outcome.error {
        None => writeln!(logger.file, "\n[outcome] OK — run completed").unwrap(),
        Some(e) => writeln!(logger.file, "\n[outcome] ERROR: {e}").unwrap(),
    }
    logger.file.flush().unwrap();

    let index = std::path::Path::new(WORKSPACE).join("index.html");
    assert!(index.exists(), "index.html was not created — see {LOG}");
    let html = std::fs::read_to_string(&index).unwrap();
    println!("index.html created: {} bytes", html.len());
    assert!(html.to_lowercase().contains("supergravity"), "page should mention Supergravity");
}
