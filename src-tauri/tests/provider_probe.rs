//! Isolation probe: provider-level stream dump with the exact inputs the agent uses.
//! Run: cargo test --test provider_probe -- --ignored --nocapture

use futures::StreamExt;
use supergravity::core::providers::ollama::OllamaProvider;
use supergravity::core::providers::Provider;
use supergravity::core::tools::default_tools;
use supergravity::core::types::{ChatEvent, Message, Role, ToolSpec};

const MODEL: &str = "qwen3:0.6b";

fn sys_prompt() -> String {
    let tools: Vec<String> = default_tools().iter().map(|t| t.spec().name).collect();
    format!(
        "You are Supergravity, a coding agent.\nWorkspace root: C:\\Users\\Owner\\AppData\\Local\\Temp\\.tmpAbC12\nAvailable tools: {}.\nRules: keep all file access inside the workspace root; read before you modify; prefer small, targeted changes; when a tool returns an error, adapt or explain instead of retrying blindly.\nYou may write files and run shell commands without user approval.",
        tools.join(", ")
    )
}

fn tool_specs() -> Vec<ToolSpec> {
    default_tools().iter().map(|t| t.spec()).collect()
}

async fn dump(tag: &str, messages: &[Message]) {
    let provider = OllamaProvider::new(None);
    let specs = tool_specs();
    let mut stream = provider.stream_chat(MODEL, messages, &specs).await.unwrap();
    let mut n_deltas = 0;
    let mut total_text = String::new();
    let mut calls = vec![];
    let mut done = false;
    while let Some(ev) = stream.next().await {
        match ev.unwrap() {
            ChatEvent::TextDelta(d) => {
                n_deltas += 1;
                total_text.push_str(&d);
            }
            ChatEvent::ToolCall { name, args_json, .. } => calls.push(format!("{name} {args_json}")),
            ChatEvent::Done => done = true,
            _ => {}
        }
    }
    println!("=== {tag}: deltas={n_deltas} text_len={} done={done} calls={calls:?}", total_text.len());
    println!("--- text ---\n{total_text}\n");
}

#[tokio::test]
#[ignore = "needs a local Ollama server with qwen3:0.6b"]
async fn probe_iteration1() {
    let messages = vec![
        Message::text(Role::System, sys_prompt()),
        Message::text(Role::User, "List the files in this workspace using the list_dir tool."),
    ];
    dump("iteration 1", &messages).await;
}

#[tokio::test]
#[ignore = "needs a local Ollama server with qwen3:0.6b"]
async fn probe_iteration2() {
    let messages = vec![
        Message::text(Role::System, sys_prompt()),
        Message::text(Role::User, "List the files in this workspace using the list_dir tool."),
        Message {
            role: Role::Assistant,
            parts: vec![
                supergravity::core::types::ContentPart::Text { text: "<think>".into() },
                supergravity::core::types::ContentPart::ToolCall {
                    id: "ollama-abc".into(),
                    name: "list_dir".into(),
                    args_json: "{\"depth\":1,\"path\":\".\"}".into(),
                },
            ],
        },
        Message {
            role: Role::Tool,
            parts: vec![supergravity::core::types::ContentPart::ToolResult {
                tool_call_id: "ollama-abc".into(),
                content: "main.rs\nnotes.txt".into(),
                is_error: false,
            }],
        },
    ];
    dump("iteration 2", &messages).await;
}
