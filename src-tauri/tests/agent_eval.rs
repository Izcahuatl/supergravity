//! Agent eval: scored scenarios against a live Ollama model.
//! Measures tool-call quality (selection, args, loops, error recovery),
//! attribution quality, and termination — per model, so prompt/tool changes
//! can be compared as data (e.g. LAB_MODEL=qwen3:0.6b vs qwen3:4b).
//!
//! Run: cargo test --test agent_eval -- --ignored --nocapture

use supergravity::core::agent::{self, AgentRequest, DEFAULT_MAX_ITERATIONS};
use supergravity::core::approvals::ApprovalBroker;
use supergravity::core::providers::ollama::OllamaProvider;
use supergravity::core::providers::Provider;
use supergravity::core::tools::default_tools;
use supergravity::core::types::{AgentEvent, ApprovalMode, ContentPart, Message, Role};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

fn model() -> String {
    std::env::var("LAB_MODEL").unwrap_or_else(|_| "qwen3:0.6b".into())
}

struct EvalResult {
    name: &'static str,
    iterations: usize,
    tool_calls: Vec<String>,
    tool_errors: usize,
    attribution_confusion: usize,
    final_text: String,
    run_error: Option<String>,
    checks: Vec<(&'static str, bool)>,
}

impl EvalResult {
    fn print(&self) {
        println!("\n### {}", self.name);
        println!("  iterations: {} | tool calls: {} | tool errors: {} | attribution confusion: {}",
            self.iterations, self.tool_calls.len(), self.tool_errors, self.attribution_confusion);
        for c in &self.tool_calls {
            println!("    call: {c}");
        }
        if let Some(e) = &self.run_error {
            println!("  RUN ERROR: {e}");
        }
        for (label, ok) in &self.checks {
            println!("  [{}] {label}", if *ok { "PASS" } else { "FAIL" });
        }
        let preview: String = self.final_text.chars().take(220).collect();
        println!("  final: {}", preview.replace('\n', " | "));
    }
}

fn count_confusion(text: &str) -> usize {
    let pats = [
        "the user ran", "the user tried", "the user called", "the user executed",
        "the user used the", "the user typed", "user's command", "the user did the",
    ];
    pats.iter().map(|p| text.matches(p).count()).sum()
}

async fn eval_scenario(
    name: &'static str,
    workspace: &std::path::Path,
    prompt: &str,
    checks: Vec<(&'static str, bool)>,
) -> EvalResult {
    let provider: Arc<dyn Provider> = Arc::new(OllamaProvider::new(None));
    let (events_tx, mut events_rx) = mpsc::channel::<AgentEvent>(4096);
    let broker = Arc::new(ApprovalBroker::new(ApprovalMode::Auto, events_tx.clone()));
    let req = AgentRequest {
        workspace_root: workspace.to_path_buf(),
        provider,
        model: model(),
        history: vec![Message::text(Role::User, prompt)],
        tools: default_tools(),
        approvals: broker,
        events: events_tx,
        cancel: CancellationToken::new(),
        max_iterations: DEFAULT_MAX_ITERATIONS,
        backup: None,
        workshop_root: None,
    };
    let handle = tokio::spawn(agent::run(req));
    let mut tool_calls = vec![];
    let mut tool_errors = 0;
    while let Some(ev) = events_rx.recv().await {
        match ev {
            AgentEvent::ToolCallProposed { name, args_json, .. } => tool_calls.push(format!("{name} {args_json}")),
            AgentEvent::ToolCallFinished { ok: false, .. } => tool_errors += 1,
            _ => {}
        }
    }
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(600), handle)
        .await
        .expect("scenario timeout")
        .unwrap();

    let mut text = String::new();
    let mut iterations = 0;
    for m in &outcome.produced {
        if m.role == Role::Assistant {
            iterations += 1;
        }
        for p in &m.parts {
            if let ContentPart::Text { text: t } = p {
                text.push_str(t);
                text.push('\n');
            }
        }
    }
    let attribution_confusion = count_confusion(&text);
    EvalResult {
        name,
        iterations,
        tool_calls,
        tool_errors,
        attribution_confusion,
        final_text: text.trim().into(),
        run_error: outcome.error.map(|e| e.to_string()),
        checks,
    }
}

fn has_call(calls: &[String], name: &str) -> bool {
    calls.iter().any(|c| c.starts_with(name))
}

#[tokio::test]
#[ignore = "needs a local Ollama server; set LAB_MODEL to compare models"]
async fn eval_all() {
    println!("=== agent_eval — model: {} ===", model());
    let mut results = vec![];

    // S1: simple listing
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(ws.path().join("notes.txt"), "notes\n").unwrap();
    let r = eval_scenario(
        "S1 list files",
        ws.path(),
        "List the files in this workspace.",
        vec![],
    )
    .await;
    let r = EvalResult {
        checks: vec![
            ("single list_dir call", r.tool_calls.len() == 1 && has_call(&r.tool_calls, "list_dir")),
            ("answer names both files", r.final_text.contains("main.rs") && r.final_text.contains("notes.txt")),
            ("no attribution confusion", r.attribution_confusion == 0),
            ("no run error", r.run_error.is_none()),
        ],
        ..r
    };
    r.print();
    results.push(r);

    // S2: read a specific file
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("secret.txt"), "the magic number is 42\n").unwrap();
    let r = eval_scenario(
        "S2 read file",
        ws.path(),
        "Read the file secret.txt and tell me the magic number.",
        vec![],
    )
    .await;
    let r = EvalResult {
        checks: vec![
            ("read_file called with secret.txt", r.tool_calls.iter().any(|c| c.starts_with("read_file") && c.contains("secret.txt"))),
            ("answer says 42", r.final_text.contains("42")),
            ("no run error", r.run_error.is_none()),
        ],
        ..r
    };
    r.print();
    results.push(r);

    // S3: write task — verifies file actually lands on disk
    let ws = tempfile::tempdir().unwrap();
    let target = ws.path().join("hello.txt");
    let r = eval_scenario(
        "S3 write file",
        ws.path(),
        "Create a file hello.txt containing exactly: world",
        vec![],
    )
    .await;
    let on_disk = std::fs::read_to_string(&target).unwrap_or_default();
    let r = EvalResult {
        checks: vec![
            ("write_file called", has_call(&r.tool_calls, "write_file")),
            ("file exists with 'world'", on_disk.contains("world")),
            ("no run error", r.run_error.is_none()),
        ],
        ..r
    };
    r.print();
    results.push(r);

    // S4: shell + attribution follow-up
    let ws = tempfile::tempdir().unwrap();
    let r = eval_scenario(
        "S4 shell attribution",
        ws.path(),
        "Use run_shell to print hello, then tell me who ran that command.",
        vec![],
    )
    .await;
    let r = EvalResult {
        checks: vec![
            ("run_shell called", has_call(&r.tool_calls, "run_shell")),
            ("no attribution confusion", r.attribution_confusion == 0),
            ("claims ownership (says I/me)", r.final_text.to_lowercase().contains("i ran") || r.final_text.to_lowercase().contains("i did") || r.final_text.to_lowercase().contains("i executed") || r.final_text.to_lowercase().contains("agent")),
        ],
        ..r
    };
    r.print();
    results.push(r);

    // S5: multi-step glob → read
    let ws = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join("src")).unwrap();
    std::fs::write(ws.path().join("src/lib.rs"), "pub const ANSWER: u32 = 7;\n").unwrap();
    let r = eval_scenario(
        "S5 glob then read",
        ws.path(),
        "Find all .rs files in this workspace, then read the first one you find and tell me the value of ANSWER.",
        vec![],
    )
    .await;
    let r = EvalResult {
        checks: vec![
            ("glob or list called", has_call(&r.tool_calls, "glob") || has_call(&r.tool_calls, "list_dir")),
            ("read_file called", has_call(&r.tool_calls, "read_file")),
            ("answer says 7", r.final_text.contains('7')),
            ("no run error", r.run_error.is_none()),
        ],
        ..r
    };
    r.print();
    results.push(r);

    // S6: error recovery on missing file
    let ws = tempfile::tempdir().unwrap();
    let r = eval_scenario(
        "S6 missing file recovery",
        ws.path(),
        "Read the file does-not-exist.txt and summarize it for me.",
        vec![],
    )
    .await;
    let read_attempts = r.tool_calls.iter().filter(|c| c.starts_with("read_file")).count();
    let checked_existence_first = has_call(&r.tool_calls, "list_dir") || has_call(&r.tool_calls, "glob");
    let r = EvalResult {
        checks: vec![
            ("read_file attempted OR existence checked first", read_attempts >= 1 || checked_existence_first),
            ("at most 2 read attempts (no blind retry)", read_attempts <= 2),
            ("reports missing gracefully", {
                let t = r.final_text.to_lowercase();
                t.contains("doesn't exist") || t.contains("does not exist") || t.contains("not found") || t.contains("cannot read") || t.contains("no such file") || t.contains("error")
            }),
        ],
        ..r
    };
    r.print();
    results.push(r);

    // S7: loop resistance — task already complete on disk
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("done.txt"), "already here\n").unwrap();
    let r = eval_scenario(
        "S7 loop resistance",
        ws.path(),
        "There is a file done.txt in this workspace. Read it and confirm its contents.",
        vec![],
    )
    .await;
    let r = EvalResult {
        checks: vec![
            ("few iterations (<= 3)", r.iterations <= 3),
            ("mentions 'already here'", r.final_text.contains("already here")),
            ("no run error", r.run_error.is_none()),
        ],
        ..r
    };
    r.print();
    results.push(r);

    // S8: surgical single-string edit — file must keep everything else intact
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(
        ws.path().join("config.txt"),
        "host=localhost\nport=8080\ndebug=false\n",
    )
    .unwrap();
    let r = eval_scenario(
        "S8 surgical edit",
        ws.path(),
        "In config.txt, change the port from 8080 to 9090. Leave everything else unchanged.",
        vec![],
    )
    .await;
    let on_disk = std::fs::read_to_string(ws.path().join("config.txt")).unwrap_or_default();
    let used_edit = has_call(&r.tool_calls, "edit_file");
    let r = EvalResult {
        checks: vec![
            ("port is now 9090", on_disk.contains("port=9090")),
            ("other lines intact", on_disk.contains("host=localhost") && on_disk.contains("debug=false")),
            ("used edit_file (surgical, not rewrite)", used_edit),
            ("no run error", r.run_error.is_none()),
        ],
        ..r
    };
    r.print();
    results.push(r);

    // S9: multi-line replacement in source code
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(
        ws.path().join("calc.rs"),
        "fn add(a: i32, b: i32) -> i32 {\n    a - b\n}\n\nfn main() {\n    println!(\"{}\", add(2, 3));\n}\n",
    )
    .unwrap();
    let r = eval_scenario(
        "S9 code edit",
        ws.path(),
        "In calc.rs, the add function is wrong: it subtracts. Fix it to actually add, without changing main.",
        vec![],
    )
    .await;
    let on_disk = std::fs::read_to_string(ws.path().join("calc.rs")).unwrap_or_default();
    let r = EvalResult {
        checks: vec![
            ("body now adds", on_disk.contains("a + b")),
            ("no subtraction left in add", !on_disk.contains("a - b")),
            ("main unchanged", on_disk.contains("fn main()") && on_disk.contains("add(2, 3)")),
            ("no run error", r.run_error.is_none()),
        ],
        ..r
    };
    r.print();
    results.push(r);

    // S10: ambiguous string — model must add context or use expected_replacements
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("dup.txt"), "x = 1;\nx = 2;\n").unwrap();
    let r = eval_scenario(
        "S10 ambiguous string",
        ws.path(),
        "In dup.txt, change only the SECOND 'x = ' line so it reads 'y = 2;'. The first line must stay 'x = 1;'.",
        vec![],
    )
    .await;
    let on_disk = std::fs::read_to_string(ws.path().join("dup.txt")).unwrap_or_default();
    let r = EvalResult {
        checks: vec![
            ("first line unchanged", on_disk.contains("x = 1;")),
            ("second line changed", on_disk.contains("y = 2;")),
            ("no run error", r.run_error.is_none()),
        ],
        ..r
    };
    r.print();
    results.push(r);

    // Summary
    let pass: usize = results.iter().map(|r| r.checks.iter().filter(|(_, ok)| *ok).count()).sum();
    let total: usize = results.iter().map(|r| r.checks.len()).sum();
    let confusion: usize = results.iter().map(|r| r.attribution_confusion).sum();
    let tool_errs: usize = results.iter().map(|r| r.tool_errors).sum();
    println!("\n=== SUMMARY ({}): checks {pass}/{total} | attribution confusion {confusion} | tool errors {tool_errs} ===", model());
    for r in &results {
        let ok = r.checks.iter().filter(|(_, ok)| *ok).count();
        println!("  {}: {}/{}", r.name, ok, r.checks.len());
    }
}
