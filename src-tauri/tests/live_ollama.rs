//! Live smoke tests against a local Ollama server (http://localhost:11434).
//! Ignored by default — run explicitly with:
//!   cargo test --test live_ollama -- --ignored

use futures::StreamExt;
use supergravity::core::providers::ollama::OllamaProvider;
use supergravity::core::providers::Provider;
use supergravity::core::types::{ChatEvent, Message, Role, ToolSpec};

const MODEL: &str = "qwen3:0.6b";

#[tokio::test]
#[ignore = "needs a local Ollama server with qwen3:0.6b"]
async fn live_text_stream() {
    let provider = OllamaProvider::new(None);
    let msgs = vec![Message::text(Role::User, "Reply with exactly: SUPERGRAVITY OK")];
    let mut stream = provider.stream_chat(MODEL, &msgs, &[]).await.expect("connect");
    let mut text = String::new();
    let mut done = false;
    while let Some(ev) = stream.next().await {
        match ev.expect("stream error") {
            ChatEvent::TextDelta(d) => text.push_str(&d),
            ChatEvent::Done => done = true,
            _ => {}
        }
    }
    assert!(done, "stream must end with Done");
    assert!(!text.trim().is_empty(), "model returned no text");
    println!("live text: {}", &text[..text.len().min(200)]);
}

#[tokio::test]
#[ignore = "needs a local Ollama server with qwen3:0.6b — small models can be flaky at tool calls"]
async fn live_tool_call() {
    // NOTE: qwen3:0.6b sometimes emits the tool call as plain text instead of a
    // structured `tool_calls` entry (model capability, not a protocol issue).
    // The structured-call assembly is pinned by unit tests; this test verifies
    // the live stream completes correctly and that IF calls arrive, they have
    // the expected shape. For a strict check, point MODEL at qwen3:4b+.
    let provider = OllamaProvider::new(None);
    let msgs = vec![Message::text(
        Role::User,
        "Call the get_weather tool for Paris. Do not answer in text.",
    )];
    let tools = vec![ToolSpec {
        name: "get_weather".into(),
        description: "Get the weather for a city".into(),
        params_schema: serde_json::json!({
            "type": "object",
            "properties": {"city": {"type": "string"}},
            "required": ["city"]
        }),
    }];
    let mut stream = provider.stream_chat(MODEL, &msgs, &tools).await.expect("connect");
    let mut calls = vec![];
    let mut text = String::new();
    let mut done = false;
    while let Some(ev) = stream.next().await {
        match ev.expect("stream error") {
            ChatEvent::ToolCall { id, name, args_json } => calls.push((id, name, args_json)),
            ChatEvent::TextDelta(d) => text.push_str(&d),
            ChatEvent::Done => done = true,
            _ => {}
        }
    }
    assert!(done, "stream must end with Done");
    if calls.is_empty() {
        println!("model answered in text (flaky tool use): {}", &text[..text.len().min(200)]);
        return;
    }
    let (id, name, args) = &calls[0];
    assert_eq!(name, "get_weather");
    assert!(id.starts_with("ollama-"), "{id}");
    assert!(args.contains("city"), "{args}");
    println!("live tool call: {name} {args}");
}
