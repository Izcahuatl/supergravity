# Supergravity Core Engine — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the pure-Rust `core` library of supergravity: provider abstraction (OpenAI / Anthropic / Gemini / Ollama / OpenAI-compatible), agent tool-call loop, sandboxed tools, approval broker, SQLite store, and config/keychain — fully unit-tested with zero Tauri dependencies.

**Architecture:** One crate at `src-tauri/` (lib target only for now; the Tauri bin/bridge arrives in a follow-up plan). All logic lives under `src/core/`. Provider network code is a thin async wrapper over sync, pure assembler functions that are tested against recorded SSE/NDJSON streams. The agent loop is tested with a scripted `MockProvider`.

**Tech Stack:** Rust 1.95 (edition 2021), tokio, reqwest, async-stream, futures, serde/serde_json, thiserror, rusqlite (bundled), keyring, directories, toml, regex, walkdir, glob, uuid, tokio-util; dev: tempfile.

**Spec:** `docs/superpowers/specs/2026-07-17-supergravity-design.md`

**Working directory note:** All cargo commands run in `src-tauri/`. All git commands run at the repo root `B:/Jetbrains/projects/kimislop`.

---

### Task 1: Crate scaffold

**Files:**
- Delete: `Cargo.toml`, `Cargo.lock`, `src/main.rs` (repo root)
- Create: `src-tauri/Cargo.toml`, `src-tauri/src/lib.rs`, `src-tauri/src/core/mod.rs`

- [x] **Step 1: Remove the placeholder crate, create the new structure**

```bash
cd /b/Jetbrains/projects/kimislop
git rm -r -q src Cargo.toml Cargo.lock
mkdir -p src-tauri/src/core
```

- [x] **Step 2: Write `src-tauri/Cargo.toml`**

```toml
[package]
name = "supergravity"
version = "0.1.0"
edition = "2021"

[lib]
name = "supergravity"
path = "src/lib.rs"

[dependencies]
```

- [x] **Step 3: Write `src-tauri/src/lib.rs`**

```rust
pub mod core;
```

- [x] **Step 4: Write `src-tauri/src/core/mod.rs` (with smoke test)**

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn scaffold_smoke() {
        assert_eq!(2 + 2, 4);
    }
}
```

- [x] **Step 5: Add dependencies**

```bash
cd /b/Jetbrains/projects/kimislop/src-tauri
cargo add tokio --features rt-multi-thread,macros,time,process,sync
cargo add async-trait futures async-stream serde_json thiserror toml directories regex walkdir glob tokio-util
cargo add serde --features derive
cargo add reqwest --features json,stream
cargo add rusqlite --features bundled
cargo add keyring
cargo add uuid --features v4
cargo add tempfile --dev
```

- [x] **Step 6: Verify build and smoke test**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test`
Expected: `test result: ok. 1 passed`

- [x] **Step 7: Commit**

```bash
cd /b/Jetbrains/projects/kimislop
git add src-tauri
git commit -m "chore: scaffold supergravity core crate at src-tauri/"
```

---

### Task 2: Core types and error

**Files:**
- Create: `src-tauri/src/core/types.rs`
- Create: `src-tauri/src/core/error.rs`
- Modify: `src-tauri/src/core/mod.rs`

- [x] **Step 1: Write the failing tests (append to `src-tauri/src/core/types.rs` after creating it empty, or write whole file with tests then impl — here: write tests first in `src-tauri/src/core/types.rs`)**

Create `src-tauri/src/core/types.rs` containing ONLY:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_part_serde_roundtrip() {
        let parts = vec![
            ContentPart::Text { text: "hello".into() },
            ContentPart::ToolCall { id: "c1".into(), name: "read_file".into(), args_json: "{}".into() },
            ContentPart::ToolResult { tool_call_id: "c1".into(), content: "data".into(), is_error: false },
        ];
        for p in parts {
            let json = serde_json::to_string(&p).unwrap();
            let back: ContentPart = serde_json::from_str(&json).unwrap();
            assert_eq!(p, back);
        }
    }

    #[test]
    fn content_part_tagged_shape() {
        let json = serde_json::to_value(ContentPart::Text { text: "hi".into() }).unwrap();
        assert_eq!(json, serde_json::json!({"type": "text", "text": "hi"}));
    }

    #[test]
    fn role_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Role::Assistant).unwrap(), "\"assistant\"");
    }

    #[test]
    fn approval_mode_snake_case() {
        assert_eq!(serde_json::to_string(&ApprovalMode::Manual).unwrap(), "\"manual\"");
        assert_eq!(serde_json::to_string(&ApprovalMode::Auto).unwrap(), "\"auto\"");
    }

    #[test]
    fn provider_kind_snake_case() {
        assert_eq!(serde_json::to_string(&ProviderKind::OpenAiCompatible).unwrap(), "\"open_ai_compatible\"");
    }

    #[test]
    fn message_text_constructor() {
        let m = Message::text(Role::User, "hey");
        assert_eq!(m.role, Role::User);
        assert_eq!(m.parts, vec![ContentPart::Text { text: "hey".into() }]);
    }

    #[test]
    fn provider_config_roundtrip() {
        let cfg = ProviderConfig {
            id: "groq".into(),
            label: "Groq".into(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: Some("https://api.groq.com/openai/v1".into()),
            has_key: true,
            models: vec!["llama-3.3-70b".into()],
            extra_headers: vec![("X-Team".into(), "a".into())],
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: ProviderConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }
}
```

Create `src-tauri/src/core/error.rs` containing ONLY:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_provider_error() {
        let e = Error::Provider { status: 401, body: "bad key".into() };
        let s = e.to_string();
        assert!(s.contains("401"), "{s}");
        assert!(s.contains("bad key"), "{s}");
    }

    #[test]
    fn from_json_error() {
        let r: std::result::Result<serde_json::Value, _> = serde_json::from_str("{nope");
        let e: Error = r.unwrap_err().into();
        assert!(matches!(e, Error::Json(_)));
    }

    #[test]
    fn cancelled_display() {
        assert!(!Error::Cancelled.to_string().is_empty());
    }
}
```

Set `src-tauri/src/core/mod.rs` to:

```rust
pub mod error;
pub mod types;

#[cfg(test)]
mod tests {
    #[test]
    fn scaffold_smoke() {
        assert_eq!(2 + 2, 4);
    }
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test`
Expected: compile errors — `ContentPart`, `Role`, `Error` etc. not found.

- [x] **Step 3: Implement `src-tauri/src/core/types.rs` (prepend above the test module)**

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    ToolCall { id: String, name: String, args_json: String },
    ToolResult { tool_call_id: String, content: String, is_error: bool },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub parts: Vec<ContentPart>,
}

impl Message {
    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Message { role, parts: vec![ContentPart::Text { text: text.into() }] }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub params_schema: serde_json::Value,
}

/// One event from a provider's streaming chat response.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatEvent {
    TextDelta(String),
    ToolCall { id: String, name: String, args_json: String },
    Usage { input_tokens: u64, output_tokens: u64 },
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    Manual,
    Auto,
}

/// One event from the agent loop toward the UI.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    TextDelta(String),
    ToolCallProposed { id: String, name: String, args_json: String },
    ApprovalRequested { request_id: String, tool_call_id: String, name: String, args_json: String },
    ToolCallFinished { tool_call_id: String, ok: bool, summary: String },
    MessageDone,
    Error(String),
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    OpenAi,
    Anthropic,
    Gemini,
    Ollama,
    OpenAiCompatible,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub label: String,
    pub kind: ProviderKind,
    pub base_url: Option<String>,
    /// True when an API key exists in the OS keychain (keys never live in the DB/TOML).
    pub has_key: bool,
    pub models: Vec<String>,
    pub extra_headers: Vec<(String, String)>,
}
```

- [x] **Step 4: Implement `src-tauri/src/core/error.rs` (prepend above the test module)**

```rust
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("provider returned status {status}: {body}")]
    Provider { status: u16, body: String },
    #[error("tool error: {0}")]
    Tool(String),
    #[error("store error: {0}")]
    Store(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("regex error: {0}")]
    Regex(#[from] regex::Error),
    #[error("config error: {0}")]
    Config(String),
    #[error("cancelled")]
    Cancelled,
    #[error("approval channel closed")]
    ApprovalClosed,
}

pub type Result<T> = std::result::Result<T, Error>;
```

- [x] **Step 5: Run tests to verify they pass**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test`
Expected: `test result: ok. 11 passed`

- [x] **Step 6: Commit**

```bash
cd /b/Jetbrains/projects/kimislop
git add src-tauri
git commit -m "feat(core): message/provider/agent types and error enum"
```

---

### Task 3: SSE and line decoders

Shared streaming primitives used by the OpenAI, Anthropic, and Gemini providers (SSE) and Ollama (NDJSON lines). Pure and synchronous — test against recorded byte streams.

**Files:**
- Create: `src-tauri/src/core/providers/mod.rs`
- Create: `src-tauri/src/core/providers/sse.rs`
- Modify: `src-tauri/src/core/mod.rs` (add `pub mod providers;`)

- [ ] **Step 1: Write the failing tests — create `src-tauri/src/core/providers/sse.rs` containing ONLY**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_decoder_splits_chunks() {
        let mut d = LineDecoder::new();
        assert!(d.push("hel").is_empty());
        assert_eq!(d.push("lo\nwor"), vec!["hello".to_string()]);
        assert_eq!(d.push("ld\n"), vec!["world".to_string()]);
        assert_eq!(d.finish(), None);
    }

    #[test]
    fn line_decoder_crlf_and_trailing() {
        let mut d = LineDecoder::new();
        assert_eq!(d.push("a\r\nb\r\n"), vec!["a".to_string(), "b".to_string()]);
        let mut d2 = LineDecoder::new();
        assert!(d2.push("tail").is_empty());
        assert_eq!(d2.finish(), Some("tail".to_string()));
    }

    #[test]
    fn sse_single_data_event() {
        let mut d = SseDecoder::new();
        let evs = d.push("data: {\"a\":1}\n\n");
        assert_eq!(evs, vec![SseEvent { event: None, data: "{\"a\":1}".to_string() }]);
    }

    #[test]
    fn sse_multiline_data_and_event_field() {
        let mut d = SseDecoder::new();
        let evs = d.push("event: message_start\ndata: {\"x\":\ndata: 1}\n\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].event.as_deref(), Some("message_start"));
        assert_eq!(evs[0].data, "{\"x\":\n1}");
    }

    #[test]
    fn sse_comments_and_empty_lines_ignored() {
        let mut d = SseDecoder::new();
        let evs = d.push(": keepalive\n\n\ndata: hi\n\n");
        assert_eq!(evs, vec![SseEvent { event: None, data: "hi".to_string() }]);
    }

    #[test]
    fn sse_chunk_split_across_events() {
        let mut d = SseDecoder::new();
        assert!(d.push("data: one\n").is_empty());
        let evs = d.push("\ndata: two\n\n");
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].data, "one");
        assert_eq!(evs[1].data, "two");
    }

    #[test]
    fn sse_finish_flushes_pending_event() {
        let mut d = SseDecoder::new();
        assert!(d.push("data: last\n").is_empty());
        let evs = d.finish();
        assert_eq!(evs, vec![SseEvent { event: None, data: "last".to_string() }]);
    }
}
```

Create `src-tauri/src/core/providers/mod.rs`:

```rust
pub mod sse;
```

Modify `src-tauri/src/core/mod.rs` — add at the top:

```rust
pub mod providers;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test`
Expected: compile errors — `LineDecoder`, `SseDecoder`, `SseEvent` not found.

- [ ] **Step 3: Implement the decoders (prepend to `src-tauri/src/core/providers/sse.rs`)**

```rust
/// Incremental UTF-8-safe line splitter. Feed string chunks; get back complete
/// lines without terminators. Handles `\n` and `\r\n`.
#[derive(Default)]
pub struct LineDecoder {
    buf: String,
}

impl LineDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &str) -> Vec<String> {
        self.buf.push_str(chunk);
        let mut lines = Vec::new();
        while let Some(pos) = self.buf.find('\n') {
            let mut line: String = self.buf.drain(..=pos).collect();
            line.pop(); // '\n'
            if line.ends_with('\r') {
                line.pop();
            }
            lines.push(line);
        }
        lines
    }

    /// Flush a trailing unterminated line, if any.
    pub fn finish(&mut self) -> Option<String> {
        if self.buf.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.buf))
        }
    }
}

/// One parsed Server-Sent-Events block.
#[derive(Debug, Clone, PartialEq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

/// Incremental SSE parser built on [`LineDecoder`]. Events are dispatched on
/// blank lines; `:` comment lines and `id:`/`retry:` fields are ignored.
#[derive(Default)]
pub struct SseDecoder {
    lines: LineDecoder,
    cur_event: Option<String>,
    cur_data: Vec<String>,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &str) -> Vec<SseEvent> {
        let mut out = Vec::new();
        for line in self.lines.push(chunk) {
            self.process_line(line, &mut out);
        }
        out
    }

    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut out = Vec::new();
        if let Some(line) = self.lines.finish() {
            self.process_line(line, &mut out);
        }
        self.dispatch(&mut out);
        out
    }

    fn process_line(&mut self, line: String, out: &mut Vec<SseEvent>) {
        if line.is_empty() {
            self.dispatch(out);
        } else if line.starts_with(':') {
            // comment / keepalive
        } else if let Some(data) = line.strip_prefix("data:") {
            let data = data.strip_prefix(' ').unwrap_or(data);
            self.cur_data.push(data.to_string());
        } else if let Some(ev) = line.strip_prefix("event:") {
            let ev = ev.strip_prefix(' ').unwrap_or(ev);
            self.cur_event = Some(ev.to_string());
        }
        // id: and retry: fields are ignored
    }

    fn dispatch(&mut self, out: &mut Vec<SseEvent>) {
        if self.cur_data.is_empty() && self.cur_event.is_none() {
            return;
        }
        out.push(SseEvent {
            event: self.cur_event.take(),
            data: std::mem::take(&mut self.cur_data).join("\n"),
        });
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test sse`
Expected: `test result: ok. 7 passed`

- [ ] **Step 5: Commit**

```bash
cd /b/Jetbrains/projects/kimislop
git add src-tauri
git commit -m "feat(core): incremental SSE and line decoders"
```

---

### Task 4: Provider trait, HTTP helper, MockProvider

**Files:**
- Create: `src-tauri/src/core/providers/http.rs`
- Create: `src-tauri/src/core/providers/mock.rs`
- Modify: `src-tauri/src/core/providers/mod.rs`

- [ ] **Step 1: Write the failing tests — create `src-tauri/src/core/providers/mock.rs` containing ONLY**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::*;
    use futures::StreamExt;

    fn script() -> Vec<Vec<crate::core::error::Result<ChatEvent>>> {
        vec![vec![
            Ok(ChatEvent::TextDelta("he".into())),
            Ok(ChatEvent::TextDelta("llo".into())),
            Ok(ChatEvent::Usage { input_tokens: 3, output_tokens: 2 }),
            Ok(ChatEvent::Done),
        ]]
    }

    #[tokio::test]
    async fn mock_yields_scripted_events() {
        let p = MockProvider::new(script());
        let msgs = vec![Message::text(Role::User, "hi")];
        let mut stream = p.stream_chat("test-model", &msgs, &[]).await.unwrap();
        let mut events = vec![];
        while let Some(e) = stream.next().await {
            events.push(e.unwrap());
        }
        assert_eq!(
            events,
            vec![
                ChatEvent::TextDelta("he".into()),
                ChatEvent::TextDelta("llo".into()),
                ChatEvent::Usage { input_tokens: 3, output_tokens: 2 },
                ChatEvent::Done,
            ]
        );
    }

    #[tokio::test]
    async fn mock_records_calls() {
        let p = MockProvider::new(script());
        let msgs = vec![Message::text(Role::User, "hi")];
        let tools = vec![ToolSpec {
            name: "t".into(),
            description: "d".into(),
            params_schema: serde_json::json!({"type": "object"}),
        }];
        let _ = p.stream_chat("m1", &msgs, &tools).await.unwrap();
        let calls = p.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "m1");
        assert_eq!(calls[0].1, msgs);
        assert_eq!(calls[0].2, tools);
    }

    #[tokio::test]
    async fn mock_exhausted_script_errors() {
        let p = MockProvider::new(vec![]);
        let msgs = vec![Message::text(Role::User, "hi")];
        let err = p.stream_chat("m", &msgs, &[]).await.unwrap_err();
        assert!(err.to_string().contains("mock script exhausted"), "{err}");
    }
}
```

Set `src-tauri/src/core/providers/mod.rs` to:

```rust
pub mod http;
pub mod mock;
pub mod sse;

use crate::core::error::Result;
use crate::core::types::{ChatEvent, Message, ToolSpec};
use futures::Stream;
use std::pin::Pin;

/// A chat-completion style model backend.
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    /// Stream one assistant turn. `tools` are the tool specs the model may call.
    async fn stream_chat(
        &self,
        model: &str,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatEvent>> + Send>>>;
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test mock`
Expected: compile error — `MockProvider` not found.

- [ ] **Step 3: Implement `src-tauri/src/core/providers/http.rs`**

```rust
use crate::core::error::{Error, Result};
use futures::{Stream, StreamExt};

/// POST a request and return the response body as a stream of text chunks.
/// Non-2xx responses become [`Error::Provider`] with a truncated body.
/// Per-request timeout: 120 s.
pub async fn post_stream(
    req: reqwest::RequestBuilder,
) -> Result<impl Stream<Item = Result<String>> + Send> {
    let resp = req.timeout(std::time::Duration::from_secs(120)).send().await?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        let body: String = body.chars().take(500).collect();
        return Err(Error::Provider { status: status.as_u16(), body });
    }
    Ok(resp
        .bytes_stream()
        .map(|chunk| chunk.map(|b| String::from_utf8_lossy(&b).into_owned()).map_err(Error::from)))
}
```

- [ ] **Step 4: Implement `src-tauri/src/core/providers/mock.rs` (prepend above the test module)**

```rust
use crate::core::error::{Error, Result};
use crate::core::types::{ChatEvent, Message, ToolSpec};
use futures::Stream;
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Mutex;

use super::Provider;

/// Scripted provider for tests and UI development without API keys.
/// Each `stream_chat` call pops one turn (a Vec of events) from the script.
pub struct MockProvider {
    pub calls: Mutex<Vec<(String, Vec<Message>, Vec<ToolSpec>)>>,
    script: Mutex<VecDeque<Vec<Result<ChatEvent>>>>,
}

impl MockProvider {
    pub fn new(script: Vec<Vec<Result<ChatEvent>>>) -> Self {
        MockProvider { calls: Mutex::new(vec![]), script: Mutex::new(script.into()) }
    }
}

#[async_trait::async_trait]
impl Provider for MockProvider {
    async fn stream_chat(
        &self,
        model: &str,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatEvent>> + Send>>> {
        self.calls.lock().unwrap().push((model.to_string(), messages.to_vec(), tools.to_vec()));
        let events = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| Error::Provider { status: 0, body: "mock script exhausted".into() })?;
        Ok(Box::pin(futures::stream::iter(events)))
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test`
Expected: `test result: ok. 21 passed`

- [ ] **Step 6: Commit**

```bash
cd /b/Jetbrains/projects/kimislop
git add src-tauri
git commit -m "feat(core): provider trait, streaming POST helper, scripted MockProvider"
```

---

### Task 5: OpenAI provider (+ OpenAI-compatible endpoints)

`OpenAiProvider` also backs `ProviderKind::OpenAiCompatible` — a custom `base_url` and optional extra headers cover OpenRouter, Groq, Mistral, Together, llama.cpp, vLLM, etc.

**Files:**
- Create: `src-tauri/src/core/providers/openai.rs`
- Modify: `src-tauri/src/core/providers/mod.rs` (add `pub mod openai;`)

- [ ] **Step 1: Write the failing tests — create `src-tauri/src/core/providers/openai.rs` containing ONLY**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::*;

    #[test]
    fn body_contains_model_stream_and_tools() {
        let msgs = vec![Message::text(Role::User, "hi")];
        let tools = vec![ToolSpec {
            name: "read_file".into(),
            description: "read a file".into(),
            params_schema: serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        }];
        let body = build_body("gpt-5", &msgs, &tools);
        assert_eq!(body["model"], "gpt-5");
        assert_eq!(body["stream"], true);
        assert_eq!(body["messages"], serde_json::json!([{"role": "user", "content": "hi"}]));
        assert_eq!(
            body["tools"],
            serde_json::json!([{
                "type": "function",
                "function": {
                    "name": "read_file",
                    "description": "read a file",
                    "parameters": {"type": "object", "properties": {"path": {"type": "string"}}}
                }
            }])
        );
    }

    #[test]
    fn body_maps_tool_call_and_result_messages() {
        let msgs = vec![
            Message::text(Role::System, "be brief"),
            Message::text(Role::User, "read x"),
            Message {
                role: Role::Assistant,
                parts: vec![
                    ContentPart::Text { text: "reading".into() },
                    ContentPart::ToolCall { id: "call_1".into(), name: "read_file".into(), args_json: "{\"path\":\"x\"}".into() },
                ],
            },
            Message {
                role: Role::Tool,
                parts: vec![ContentPart::ToolResult { tool_call_id: "call_1".into(), content: "file body".into(), is_error: false }],
            },
        ];
        let body = build_body("m", &msgs, &[]);
        assert_eq!(
            body["messages"],
            serde_json::json!([
                {"role": "system", "content": "be brief"},
                {"role": "user", "content": "read x"},
                {
                    "role": "assistant",
                    "content": "reading",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "read_file", "arguments": "{\"path\":\"x\"}"}
                    }]
                },
                {"role": "tool", "tool_call_id": "call_1", "content": "file body"}
            ])
        );
    }

    #[test]
    fn assistant_without_text_has_null_content() {
        let msgs = vec![Message {
            role: Role::Assistant,
            parts: vec![ContentPart::ToolCall { id: "c".into(), name: "n".into(), args_json: "{}".into() }],
        }];
        let body = build_body("m", &msgs, &[]);
        assert_eq!(body["messages"][0]["content"], serde_json::Value::Null);
    }

    #[test]
    fn no_tools_omits_tools_key() {
        let body = build_body("m", &[Message::text(Role::User, "x")], &[]);
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn assembler_streams_text_and_done() {
        let mut a = OpenAiAssembler::default();
        let evs = a.push_data(r#"{"choices":[{"delta":{"content":"hel"},"finish_reason":null}]}"#);
        assert_eq!(evs, vec![ChatEvent::TextDelta("hel".into())]);
        let evs = a.push_data("[DONE]");
        assert_eq!(evs, vec![ChatEvent::Done]);
        assert!(a.finish().is_empty(), "Done must not be emitted twice");
    }

    #[test]
    fn assembler_collects_streamed_tool_call() {
        let mut a = OpenAiAssembler::default();
        assert!(a.push_data(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_9","function":{"name":"read_file","arguments":""}}]},"finish_reason":null}]}"#).is_empty());
        assert!(a.push_data(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"pa"}}]},"finish_reason":null}]}"#).is_empty());
        assert!(a.push_data(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"x\"}"}}]},"finish_reason":null}]}"#).is_empty());
        let evs = a.push_data(r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#);
        assert_eq!(
            evs,
            vec![ChatEvent::ToolCall { id: "call_9".into(), name: "read_file".into(), args_json: "{\"path\":\"x\"}".into() }]
        );
    }

    #[test]
    fn assembler_usage_and_malformed_tolerance() {
        let mut a = OpenAiAssembler::default();
        let evs = a.push_data(r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":4}}"#);
        assert_eq!(evs, vec![ChatEvent::Usage { input_tokens: 10, output_tokens: 4 }]);
        assert!(a.push_data("{not json").is_empty(), "malformed chunks are skipped");
    }

    #[test]
    fn assembler_finish_emits_done_when_stream_ends_silently() {
        let mut a = OpenAiAssembler::default();
        a.push_data(r#"{"choices":[{"delta":{"content":"x"},"finish_reason":"stop"}]}"#);
        assert_eq!(a.finish(), vec![ChatEvent::Done]);
    }
}
```

Add to `src-tauri/src/core/providers/mod.rs` (top of module list):

```rust
pub mod openai;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test openai`
Expected: compile errors — `build_body`, `OpenAiAssembler` not found.

- [ ] **Step 3: Implement the provider (prepend to `src-tauri/src/core/providers/openai.rs`)**

```rust
use crate::core::error::Result;
use crate::core::types::{ChatEvent, ContentPart, Message, Role, ToolSpec};
use futures::{Stream, StreamExt};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::pin::Pin;

use super::http::post_stream;
use super::sse::SseDecoder;
use super::Provider;

/// OpenAI Chat Completions backend. With a custom `base_url` it also serves any
/// OpenAI-compatible endpoint (OpenRouter, Groq, Mistral, Together, llama.cpp, vLLM…).
pub struct OpenAiProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    extra_headers: Vec<(String, String)>,
}

impl OpenAiProvider {
    pub fn new(base_url: Option<&str>, api_key: Option<String>, extra_headers: Vec<(String, String)>) -> Self {
        OpenAiProvider {
            client: reqwest::Client::new(),
            base_url: base_url.unwrap_or("https://api.openai.com/v1").trim_end_matches('/').to_string(),
            api_key,
            extra_headers,
        }
    }
}

fn concat_text(m: &Message) -> String {
    m.parts
        .iter()
        .filter_map(|p| match p {
            ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

fn openai_messages(messages: &[Message]) -> Vec<Value> {
    let mut out = Vec::new();
    for m in messages {
        match m.role {
            Role::System => out.push(json!({"role": "system", "content": concat_text(m)})),
            Role::User => out.push(json!({"role": "user", "content": concat_text(m)})),
            Role::Assistant => {
                let text = concat_text(m);
                let mut msg = json!({"role": "assistant"});
                msg["content"] = if text.is_empty() { Value::Null } else { json!(text) };
                let calls: Vec<Value> = m
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        ContentPart::ToolCall { id, name, args_json } => Some(json!({
                            "id": id,
                            "type": "function",
                            "function": {"name": name, "arguments": args_json}
                        })),
                        _ => None,
                    })
                    .collect();
                if !calls.is_empty() {
                    msg["tool_calls"] = json!(calls);
                }
                out.push(msg);
            }
            Role::Tool => {
                for p in &m.parts {
                    if let ContentPart::ToolResult { tool_call_id, content, .. } = p {
                        out.push(json!({"role": "tool", "tool_call_id": tool_call_id, "content": content}));
                    }
                }
            }
        }
    }
    out
}

pub fn build_body(model: &str, messages: &[Message], tools: &[ToolSpec]) -> Value {
    let mut body = json!({
        "model": model,
        "stream": true,
        "messages": openai_messages(messages),
    });
    if !tools.is_empty() {
        body["tools"] = tools
            .iter()
            .map(|t| json!({
                "type": "function",
                "function": {"name": t.name, "description": t.description, "parameters": t.params_schema}
            }))
            .collect::<Vec<_>>()
            .into();
        body["tool_choice"] = json!("auto");
    }
    body
}

#[derive(Default)]
struct ToolCallBuf {
    id: String,
    name: String,
    args: String,
}

/// Assembles OpenAI SSE `data:` payloads into [`ChatEvent`]s. Tool calls arrive
/// as fragments indexed by `index`; they are emitted when `finish_reason` is
/// `"tool_calls"`. Malformed chunks are skipped.
#[derive(Default)]
pub struct OpenAiAssembler {
    tool_calls: BTreeMap<u64, ToolCallBuf>,
    done_emitted: bool,
}

impl OpenAiAssembler {
    pub fn push_data(&mut self, data: &str) -> Vec<ChatEvent> {
        let mut out = Vec::new();
        let trimmed = data.trim();
        if trimmed == "[DONE]" {
            self.done_emitted = true;
            out.push(ChatEvent::Done);
            return out;
        }
        let v: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => return out,
        };
        if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
            out.push(ChatEvent::Usage {
                input_tokens: u["prompt_tokens"].as_u64().unwrap_or(0),
                output_tokens: u["completion_tokens"].as_u64().unwrap_or(0),
            });
        }
        if let Some(choices) = v["choices"].as_array() {
            for choice in choices {
                let delta = &choice["delta"];
                if let Some(content) = delta["content"].as_str().filter(|s| !s.is_empty()) {
                    out.push(ChatEvent::TextDelta(content.to_string()));
                }
                if let Some(tcs) = delta["tool_calls"].as_array() {
                    for tc in tcs {
                        let idx = tc["index"].as_u64().unwrap_or(0);
                        let buf = self.tool_calls.entry(idx).or_default();
                        if let Some(id) = tc["id"].as_str() {
                            buf.id = id.to_string();
                        }
                        if let Some(f) = tc.get("function") {
                            if let Some(n) = f["name"].as_str() {
                                buf.name = n.to_string();
                            }
                            if let Some(a) = f["arguments"].as_str() {
                                buf.args.push_str(a);
                            }
                        }
                    }
                }
                if choice["finish_reason"].as_str() == Some("tool_calls") {
                    for (_, buf) in std::mem::take(&mut self.tool_calls) {
                        out.push(ChatEvent::ToolCall { id: buf.id, name: buf.name, args_json: buf.args });
                    }
                }
            }
        }
        out
    }

    /// Call when the byte stream ends; emits `Done` if `[DONE]` never arrived.
    pub fn finish(&mut self) -> Vec<ChatEvent> {
        if self.done_emitted {
            vec![]
        } else {
            self.done_emitted = true;
            vec![ChatEvent::Done]
        }
    }
}

#[async_trait::async_trait]
impl Provider for OpenAiProvider {
    async fn stream_chat(
        &self,
        model: &str,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatEvent>> + Send>>> {
        let url = format!("{}/chat/completions", self.base_url);
        let mut req = self.client.post(url).json(&build_body(model, messages, tools));
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        for (k, v) in &self.extra_headers {
            req = req.header(k, v);
        }
        let chunks = post_stream(req).await?;
        let stream = async_stream::try_stream! {
            let mut decoder = SseDecoder::new();
            let mut assembler = OpenAiAssembler::default();
            tokio::pin!(chunks);
            while let Some(chunk) = chunks.next().await {
                let chunk = chunk?;
                for ev in decoder.push(&chunk) {
                    for ce in assembler.push_data(&ev.data) {
                        yield ce;
                    }
                }
            }
            for ev in decoder.finish() {
                for ce in assembler.push_data(&ev.data) {
                    yield ce;
                }
            }
            for ce in assembler.finish() {
                yield ce;
            }
        };
        Ok(Box::pin(stream))
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test openai`
Expected: `test result: ok. 8 passed`

- [ ] **Step 5: Commit**

```bash
cd /b/Jetbrains/projects/kimislop
git add src-tauri
git commit -m "feat(core): OpenAI + OpenAI-compatible provider"
```

---

### Task 6: Anthropic provider

**Files:**
- Create: `src-tauri/src/core/providers/anthropic.rs`
- Modify: `src-tauri/src/core/providers/mod.rs` (add `pub mod anthropic;`)

- [ ] **Step 1: Write the failing tests — create `src-tauri/src/core/providers/anthropic.rs` containing ONLY**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::providers::sse::SseEvent;
    use crate::core::types::*;

    fn ev(event: &str, data: &str) -> SseEvent {
        SseEvent { event: Some(event.to_string()), data: data.to_string() }
    }

    #[test]
    fn body_extracts_system_and_sets_max_tokens() {
        let msgs = vec![
            Message::text(Role::System, "be brief"),
            Message::text(Role::User, "hi"),
        ];
        let body = build_body("claude-x", &msgs, &[]);
        assert_eq!(body["system"], "be brief");
        assert_eq!(body["max_tokens"], 8096);
        assert_eq!(body["stream"], true);
        assert_eq!(body["messages"], serde_json::json!([{"role": "user", "content": [{"type": "text", "text": "hi"}]}]));
    }

    #[test]
    fn body_maps_tool_use_and_tool_result() {
        let msgs = vec![
            Message {
                role: Role::Assistant,
                parts: vec![
                    ContentPart::Text { text: "reading".into() },
                    ContentPart::ToolCall { id: "toolu_1".into(), name: "read_file".into(), args_json: "{\"path\":\"x\"}".into() },
                ],
            },
            Message {
                role: Role::Tool,
                parts: vec![ContentPart::ToolResult { tool_call_id: "toolu_1".into(), content: "body".into(), is_error: false }],
            },
        ];
        let body = build_body("m", &msgs, &[]);
        assert_eq!(
            body["messages"],
            serde_json::json!([
                {"role": "assistant", "content": [
                    {"type": "text", "text": "reading"},
                    {"type": "tool_use", "id": "toolu_1", "name": "read_file", "input": {"path": "x"}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_1", "content": "body", "is_error": false}
                ]}
            ])
        );
    }

    #[test]
    fn body_merges_consecutive_same_role_messages() {
        // A Tool message maps to role "user"; a following user text must merge into one user turn.
        let msgs = vec![
            Message {
                role: Role::Tool,
                parts: vec![ContentPart::ToolResult { tool_call_id: "t".into(), content: "r".into(), is_error: false }],
            },
            Message::text(Role::User, "now what?"),
        ];
        let body = build_body("m", &msgs, &[]);
        assert_eq!(
            body["messages"],
            serde_json::json!([{
                "role": "user",
                "content": [
                    {"type": "tool_result", "tool_use_id": "t", "content": "r", "is_error": false},
                    {"type": "text", "text": "now what?"}
                ]
            }])
        );
    }

    #[test]
    fn body_tool_specs_use_input_schema() {
        let tools = vec![ToolSpec {
            name: "run_shell".into(),
            description: "run".into(),
            params_schema: serde_json::json!({"type": "object"}),
        }];
        let body = build_body("m", &[Message::text(Role::User, "x")], &tools);
        assert_eq!(
            body["tools"],
            serde_json::json!([{"name": "run_shell", "description": "run", "input_schema": {"type": "object"}}])
        );
    }

    #[test]
    fn assembler_text_stream() {
        let mut a = AnthropicAssembler::default();
        assert!(a.push(&ev("message_start", r#"{"message":{"usage":{"input_tokens":25}}}"#)).is_empty());
        assert!(a.push(&ev("content_block_start", r#"{"index":0,"content_block":{"type":"text","text":""}}"#)).is_empty());
        let evs = a.push(&ev("content_block_delta", r#"{"index":0,"delta":{"type":"text_delta","text":"Hello"}}"#));
        assert_eq!(evs, vec![ChatEvent::TextDelta("Hello".into())]);
        assert!(a.push(&ev("message_delta", r#"{"usage":{"output_tokens":7}}"#)).is_empty());
        let evs = a.push(&ev("message_stop", r#"{}"#));
        assert_eq!(evs, vec![
            ChatEvent::Usage { input_tokens: 25, output_tokens: 7 },
            ChatEvent::Done,
        ]);
    }

    #[test]
    fn assembler_tool_use_stream() {
        let mut a = AnthropicAssembler::default();
        assert!(a.push(&ev("content_block_start", r#"{"index":1,"content_block":{"type":"tool_use","id":"toolu_9","name":"write_file"}}"#)).is_empty());
        assert!(a.push(&ev("content_block_delta", r#"{"index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#)).is_empty());
        assert!(a.push(&ev("content_block_delta", r#"{"index":1,"delta":{"type":"input_json_delta","partial_json":"\"a\"}"}}"#)).is_empty());
        let evs = a.push(&ev("content_block_stop", r#"{"index":1}"#));
        assert_eq!(
            evs,
            vec![ChatEvent::ToolCall { id: "toolu_9".into(), name: "write_file".into(), args_json: "{\"path\":\"a\"}".into() }]
        );
    }

    #[test]
    fn assembler_tolerates_unknown_events_and_malformed_data() {
        let mut a = AnthropicAssembler::default();
        assert!(a.push(&ev("ping", r#"{}"#)).is_empty());
        assert!(a.push(&ev("content_block_delta", "{broken")).is_empty());
        assert!(a.finish().is_empty());
    }
}
```

Add to `src-tauri/src/core/providers/mod.rs`:

```rust
pub mod anthropic;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test anthropic`
Expected: compile errors — `build_body`, `AnthropicAssembler` not found.

- [ ] **Step 3: Implement the provider (prepend to `src-tauri/src/core/providers/anthropic.rs`)**

```rust
use crate::core::error::Result;
use crate::core::types::{ChatEvent, ContentPart, Message, Role, ToolSpec};
use futures::{Stream, StreamExt};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::pin::Pin;

use super::http::post_stream;
use super::sse::{SseDecoder, SseEvent};
use super::Provider;

const DEFAULT_MAX_TOKENS: u32 = 8096;

/// Anthropic Messages API backend.
pub struct AnthropicProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl AnthropicProvider {
    pub fn new(base_url: Option<&str>, api_key: String) -> Self {
        AnthropicProvider {
            client: reqwest::Client::new(),
            base_url: base_url.unwrap_or("https://api.anthropic.com").trim_end_matches('/').to_string(),
            api_key,
        }
    }
}

fn text_block(text: &str) -> Value {
    json!({"type": "text", "text": text})
}

fn message_blocks(m: &Message) -> Option<(String, Vec<Value>)> {
    match m.role {
        Role::System => None,
        Role::User => {
            let blocks: Vec<Value> = m
                .parts
                .iter()
                .filter_map(|p| match p {
                    ContentPart::Text { text } => Some(text_block(text)),
                    _ => None,
                })
                .collect();
            Some(("user".into(), blocks))
        }
        Role::Assistant => {
            let blocks: Vec<Value> = m
                .parts
                .iter()
                .map(|p| match p {
                    ContentPart::Text { text } => text_block(text),
                    ContentPart::ToolCall { id, name, args_json } => json!({
                        "type": "tool_use",
                        "id": id,
                        "name": name,
                        "input": serde_json::from_str::<Value>(args_json).unwrap_or_else(|_| json!({})),
                    }),
                    ContentPart::ToolResult { .. } => json!(null),
                })
                .filter(|b| !b.is_null())
                .collect();
            Some(("assistant".into(), blocks))
        }
        Role::Tool => {
            let blocks: Vec<Value> = m
                .parts
                .iter()
                .filter_map(|p| match p {
                    ContentPart::ToolResult { tool_call_id, content, is_error } => Some(json!({
                        "type": "tool_result",
                        "tool_use_id": tool_call_id,
                        "content": content,
                        "is_error": is_error,
                    })),
                    _ => None,
                })
                .collect();
            Some(("user".into(), blocks))
        }
    }
}

pub fn build_body(model: &str, messages: &[Message], tools: &[ToolSpec]) -> Value {
    let system: String = messages
        .iter()
        .filter(|m| m.role == Role::System)
        .flat_map(|m| m.parts.iter())
        .filter_map(|p| match p {
            ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    let mut msgs: Vec<Value> = Vec::new();
    for m in messages {
        if let Some((role, blocks)) = message_blocks(m) {
            // Anthropic requires strictly alternating roles — merge consecutive same-role turns.
            if let Some(last) = msgs.last_mut() {
                if last["role"].as_str() == Some(role.as_str()) {
                    if let (Some(arr), new_blocks) = (last["content"].as_array_mut(), blocks) {
                        arr.extend(new_blocks);
                        continue;
                    }
                }
            }
            msgs.push(json!({"role": role, "content": blocks}));
        }
    }

    let mut body = json!({
        "model": model,
        "max_tokens": DEFAULT_MAX_TOKENS,
        "stream": true,
        "messages": msgs,
    });
    if !system.is_empty() {
        body["system"] = json!(system);
    }
    if !tools.is_empty() {
        body["tools"] = tools
            .iter()
            .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.params_schema}))
            .collect::<Vec<_>>()
            .into();
    }
    body
}

#[derive(Default)]
struct BlockBuf {
    id: String,
    name: String,
    args: String,
    is_tool: bool,
}

/// Assembles Anthropic SSE events into [`ChatEvent`]s.
#[derive(Default)]
pub struct AnthropicAssembler {
    blocks: BTreeMap<u64, BlockBuf>,
    input_tokens: u64,
    output_tokens: u64,
    done_emitted: bool,
}

impl AnthropicAssembler {
    pub fn push(&mut self, ev: &SseEvent) -> Vec<ChatEvent> {
        let mut out = Vec::new();
        let v: Value = match serde_json::from_str(&ev.data) {
            Ok(v) => v,
            Err(_) => return out,
        };
        match ev.event.as_deref().unwrap_or("") {
            "message_start" => {
                self.input_tokens = v["message"]["usage"]["input_tokens"].as_u64().unwrap_or(0);
            }
            "content_block_start" => {
                let idx = v["index"].as_u64().unwrap_or(0);
                let cb = &v["content_block"];
                if cb["type"].as_str() == Some("tool_use") {
                    self.blocks.insert(
                        idx,
                        BlockBuf {
                            id: cb["id"].as_str().unwrap_or("").to_string(),
                            name: cb["name"].as_str().unwrap_or("").to_string(),
                            args: String::new(),
                            is_tool: true,
                        },
                    );
                }
            }
            "content_block_delta" => {
                let idx = v["index"].as_u64().unwrap_or(0);
                let delta = &v["delta"];
                match delta["type"].as_str() {
                    Some("text_delta") => {
                        if let Some(t) = delta["text"].as_str().filter(|s| !s.is_empty()) {
                            out.push(ChatEvent::TextDelta(t.to_string()));
                        }
                    }
                    Some("input_json_delta") => {
                        if let Some(buf) = self.blocks.get_mut(&idx) {
                            if let Some(pj) = delta["partial_json"].as_str() {
                                buf.args.push_str(pj);
                            }
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let idx = v["index"].as_u64().unwrap_or(0);
                if let Some(buf) = self.blocks.remove(&idx) {
                    if buf.is_tool {
                        out.push(ChatEvent::ToolCall {
                            id: buf.id,
                            name: buf.name,
                            args_json: if buf.args.is_empty() { "{}".to_string() } else { buf.args },
                        });
                    }
                }
            }
            "message_delta" => {
                self.output_tokens = v["usage"]["output_tokens"].as_u64().unwrap_or(self.output_tokens);
            }
            "message_stop" => {
                self.done_emitted = true;
                out.push(ChatEvent::Usage { input_tokens: self.input_tokens, output_tokens: self.output_tokens });
                out.push(ChatEvent::Done);
            }
            _ => {}
        }
        out
    }

    pub fn finish(&mut self) -> Vec<ChatEvent> {
        if self.done_emitted {
            vec![]
        } else {
            self.done_emitted = true;
            vec![ChatEvent::Done]
        }
    }
}

#[async_trait::async_trait]
impl Provider for AnthropicProvider {
    async fn stream_chat(
        &self,
        model: &str,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatEvent>> + Send>>> {
        let url = format!("{}/v1/messages", self.base_url);
        let req = self
            .client
            .post(url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&build_body(model, messages, tools));
        let chunks = post_stream(req).await?;
        let stream = async_stream::try_stream! {
            let mut decoder = SseDecoder::new();
            let mut assembler = AnthropicAssembler::default();
            tokio::pin!(chunks);
            while let Some(chunk) = chunks.next().await {
                let chunk = chunk?;
                for ev in decoder.push(&chunk) {
                    for ce in assembler.push(&ev) {
                        yield ce;
                    }
                }
            }
            for ev in decoder.finish() {
                for ce in assembler.push(&ev) {
                    yield ce;
                }
            }
            for ce in assembler.finish() {
                yield ce;
            }
        };
        Ok(Box::pin(stream))
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test anthropic`
Expected: `test result: ok. 7 passed`

- [ ] **Step 5: Commit**

```bash
cd /b/Jetbrains/projects/kimislop
git add src-tauri
git commit -m "feat(core): Anthropic Messages provider"
```

---

### Task 7: Gemini provider

Gemini tool calls carry no ids — the assembler synthesizes `gemini-N`. Tool results are serialized as `functionResponse` parts, with the function name recovered from the preceding assistant `functionCall` in the history.

**Files:**
- Create: `src-tauri/src/core/providers/gemini.rs`
- Modify: `src-tauri/src/core/providers/mod.rs` (add `pub mod gemini;`)

- [ ] **Step 1: Write the failing tests — create `src-tauri/src/core/providers/gemini.rs` containing ONLY**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::*;

    #[test]
    fn body_has_system_instruction_and_contents() {
        let msgs = vec![
            Message::text(Role::System, "be brief"),
            Message::text(Role::User, "hi"),
        ];
        let body = build_body(&msgs, &[]);
        assert_eq!(body["systemInstruction"], serde_json::json!({"parts": [{"text": "be brief"}]}));
        assert_eq!(body["contents"], serde_json::json!([{"role": "user", "parts": [{"text": "hi"}]}]));
    }

    #[test]
    fn body_maps_function_call_and_response() {
        let msgs = vec![
            Message {
                role: Role::Assistant,
                parts: vec![
                    ContentPart::Text { text: "reading".into() },
                    ContentPart::ToolCall { id: "gemini-0".into(), name: "read_file".into(), args_json: "{\"path\":\"x\"}".into() },
                ],
            },
            Message {
                role: Role::Tool,
                parts: vec![ContentPart::ToolResult { tool_call_id: "gemini-0".into(), content: "body".into(), is_error: false }],
            },
        ];
        let body = build_body(&msgs, &[]);
        assert_eq!(
            body["contents"],
            serde_json::json!([
                {"role": "model", "parts": [
                    {"text": "reading"},
                    {"functionCall": {"name": "read_file", "args": {"path": "x"}}}
                ]},
                {"role": "user", "parts": [
                    {"functionResponse": {"name": "read_file", "response": {"result": "body"}}}
                ]}
            ])
        );
    }

    #[test]
    fn body_tool_specs_wrapped_in_function_declarations() {
        let tools = vec![ToolSpec {
            name: "grep".into(),
            description: "search".into(),
            params_schema: serde_json::json!({"type": "object"}),
        }];
        let body = build_body(&[Message::text(Role::User, "x")], &tools);
        assert_eq!(
            body["tools"],
            serde_json::json!([{"functionDeclarations": [{"name": "grep", "description": "search", "parameters": {"type": "object"}}]}])
        );
    }

    #[test]
    fn assembler_text_and_usage() {
        let mut a = GeminiAssembler::default();
        let evs = a.push_data(r#"{"candidates":[{"content":{"parts":[{"text":"Hello"}]}}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":2}}"#);
        assert_eq!(evs, vec![
            ChatEvent::TextDelta("Hello".into()),
            ChatEvent::Usage { input_tokens: 5, output_tokens: 2 },
        ]);
    }

    #[test]
    fn assembler_function_call_gets_synthesized_id() {
        let mut a = GeminiAssembler::default();
        let evs = a.push_data(r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"write_file","args":{"path":"a"}}}]}}]}"#);
        assert_eq!(
            evs,
            vec![ChatEvent::ToolCall { id: "gemini-0".into(), name: "write_file".into(), args_json: "{\"path\":\"a\"}".into() }]
        );
        let evs = a.push_data(r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"read_file","args":{}}}]}}]}"#);
        assert_eq!(evs[0], ChatEvent::ToolCall { id: "gemini-1".into(), name: "read_file".into(), args_json: "{}".into() });
    }

    #[test]
    fn assembler_finish_emits_done_once() {
        let mut a = GeminiAssembler::default();
        assert!(a.push_data("{broken").is_empty());
        assert_eq!(a.finish(), vec![ChatEvent::Done]);
        assert!(a.finish().is_empty());
    }
}
```

Add to `src-tauri/src/core/providers/mod.rs`:

```rust
pub mod gemini;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test gemini`
Expected: compile errors — `build_body`, `GeminiAssembler` not found.

- [ ] **Step 3: Implement the provider (prepend to `src-tauri/src/core/providers/gemini.rs`)**

```rust
use crate::core::error::Result;
use crate::core::types::{ChatEvent, ContentPart, Message, Role, ToolSpec};
use futures::{Stream, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::pin::Pin;

use super::http::post_stream;
use super::sse::SseDecoder;
use super::Provider;

/// Google Gemini (Generative Language API) backend.
pub struct GeminiProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl GeminiProvider {
    pub fn new(base_url: Option<&str>, api_key: String) -> Self {
        GeminiProvider {
            client: reqwest::Client::new(),
            base_url: base_url
                .unwrap_or("https://generativelanguage.googleapis.com")
                .trim_end_matches('/')
                .to_string(),
            api_key,
        }
    }
}

pub fn build_body(messages: &[Message], tools: &[ToolSpec]) -> Value {
    // Gemini functionResponse parts need the function name; recover it from the
    // assistant ToolCall part that shares the tool_call_id.
    let mut id_to_name: HashMap<&str, &str> = HashMap::new();
    for m in messages {
        for p in &m.parts {
            if let ContentPart::ToolCall { id, name, .. } = p {
                id_to_name.insert(id.as_str(), name.as_str());
            }
        }
    }

    let system: String = messages
        .iter()
        .filter(|m| m.role == Role::System)
        .flat_map(|m| m.parts.iter())
        .filter_map(|p| match p {
            ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    let mut contents: Vec<Value> = Vec::new();
    for m in messages {
        match m.role {
            Role::System => {}
            Role::User => {
                let parts: Vec<Value> = m
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        ContentPart::Text { text } => Some(json!({"text": text})),
                        _ => None,
                    })
                    .collect();
                contents.push(json!({"role": "user", "parts": parts}));
            }
            Role::Assistant => {
                let parts: Vec<Value> = m
                    .parts
                    .iter()
                    .map(|p| match p {
                        ContentPart::Text { text } => json!({"text": text}),
                        ContentPart::ToolCall { name, args_json, .. } => json!({
                            "functionCall": {
                                "name": name,
                                "args": serde_json::from_str::<Value>(args_json).unwrap_or_else(|_| json!({})),
                            }
                        }),
                        ContentPart::ToolResult { .. } => json!(null),
                    })
                    .filter(|p| !p.is_null())
                    .collect();
                contents.push(json!({"role": "model", "parts": parts}));
            }
            Role::Tool => {
                let parts: Vec<Value> = m
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        ContentPart::ToolResult { tool_call_id, content, .. } => {
                            let name = id_to_name.get(tool_call_id.as_str()).copied().unwrap_or("unknown");
                            Some(json!({"functionResponse": {"name": name, "response": {"result": content}}}))
                        }
                        _ => None,
                    })
                    .collect();
                contents.push(json!({"role": "user", "parts": parts}));
            }
        }
    }

    let mut body = json!({"contents": contents});
    if !system.is_empty() {
        body["systemInstruction"] = json!({"parts": [{"text": system}]});
    }
    if !tools.is_empty() {
        body["tools"] = json!([{
            "functionDeclarations": tools
                .iter()
                .map(|t| json!({"name": t.name, "description": t.description, "parameters": t.params_schema}))
                .collect::<Vec<_>>()
        }]);
    }
    body
}

/// Assembles Gemini SSE `data:` payloads into [`ChatEvent`]s.
#[derive(Default)]
pub struct GeminiAssembler {
    call_counter: usize,
    done_emitted: bool,
}

impl GeminiAssembler {
    pub fn push_data(&mut self, data: &str) -> Vec<ChatEvent> {
        let mut out = Vec::new();
        let v: Value = match serde_json::from_str(data.trim()) {
            Ok(v) => v,
            Err(_) => return out,
        };
        if let Some(candidates) = v["candidates"].as_array() {
            for cand in candidates {
                if let Some(parts) = cand["content"]["parts"].as_array() {
                    for part in parts {
                        if let Some(t) = part["text"].as_str().filter(|s| !s.is_empty()) {
                            out.push(ChatEvent::TextDelta(t.to_string()));
                        }
                        if let Some(fc) = part.get("functionCall") {
                            let id = format!("gemini-{}", self.call_counter);
                            self.call_counter += 1;
                            out.push(ChatEvent::ToolCall {
                                id,
                                name: fc["name"].as_str().unwrap_or("").to_string(),
                                args_json: fc.get("args").map(|a| a.to_string()).unwrap_or_else(|| "{}".into()),
                            });
                        }
                    }
                }
            }
        }
        if let Some(u) = v.get("usageMetadata") {
            out.push(ChatEvent::Usage {
                input_tokens: u["promptTokenCount"].as_u64().unwrap_or(0),
                output_tokens: u["candidatesTokenCount"].as_u64().unwrap_or(0),
            });
        }
        out
    }

    pub fn finish(&mut self) -> Vec<ChatEvent> {
        if self.done_emitted {
            vec![]
        } else {
            self.done_emitted = true;
            vec![ChatEvent::Done]
        }
    }
}

#[async_trait::async_trait]
impl Provider for GeminiProvider {
    async fn stream_chat(
        &self,
        model: &str,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatEvent>> + Send>>> {
        let url = format!("{}/v1beta/models/{}:streamGenerateContent?alt=sse", self.base_url, model);
        let req = self
            .client
            .post(url)
            .header("x-goog-api-key", &self.api_key)
            .json(&build_body(messages, tools));
        let chunks = post_stream(req).await?;
        let stream = async_stream::try_stream! {
            let mut decoder = SseDecoder::new();
            let mut assembler = GeminiAssembler::default();
            tokio::pin!(chunks);
            while let Some(chunk) = chunks.next().await {
                let chunk = chunk?;
                for ev in decoder.push(&chunk) {
                    for ce in assembler.push_data(&ev.data) {
                        yield ce;
                    }
                }
            }
            for ev in decoder.finish() {
                for ce in assembler.push_data(&ev.data) {
                    yield ce;
                }
            }
            for ce in assembler.finish() {
                yield ce;
            }
        };
        Ok(Box::pin(stream))
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test gemini`
Expected: `test result: ok. 6 passed`

- [ ] **Step 5: Commit**

```bash
cd /b/Jetbrains/projects/kimislop
git add src-tauri
git commit -m "feat(core): Gemini provider"
```

---

### Task 8: Ollama provider and provider factory

**Files:**
- Create: `src-tauri/src/core/providers/ollama.rs`
- Modify: `src-tauri/src/core/providers/mod.rs` (add `pub mod ollama;` + `build_provider`)

- [ ] **Step 1: Write the failing tests — create `src-tauri/src/core/providers/ollama.rs` containing ONLY**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::*;

    #[test]
    fn body_shape() {
        let msgs = vec![
            Message::text(Role::System, "s"),
            Message::text(Role::User, "hi"),
        ];
        let body = build_body("qwen3", &msgs, &[]);
        assert_eq!(body["model"], "qwen3");
        assert_eq!(body["stream"], true);
        assert_eq!(
            body["messages"],
            serde_json::json!([{"role": "system", "content": "s"}, {"role": "user", "content": "hi"}])
        );
    }

    #[test]
    fn body_tool_calls_have_object_arguments() {
        let msgs = vec![
            Message {
                role: Role::Assistant,
                parts: vec![ContentPart::ToolCall { id: "ollama-0".into(), name: "read_file".into(), args_json: "{\"path\":\"x\"}".into() }],
            },
            Message {
                role: Role::Tool,
                parts: vec![ContentPart::ToolResult { tool_call_id: "ollama-0".into(), content: "body".into(), is_error: false }],
            },
        ];
        let body = build_body("m", &msgs, &[]);
        assert_eq!(
            body["messages"][0]["tool_calls"],
            serde_json::json!([{"type": "function", "function": {"name": "read_file", "arguments": {"path": "x"}}}])
        );
        assert_eq!(body["messages"][1], serde_json::json!({"role": "tool", "content": "body"}));
    }

    #[test]
    fn assembler_text_line() {
        let mut a = OllamaAssembler::default();
        let evs = a.push_line(r#"{"message":{"role":"assistant","content":"Hel"},"done":false}"#);
        assert_eq!(evs, vec![ChatEvent::TextDelta("Hel".into())]);
    }

    #[test]
    fn assembler_tool_call_line() {
        let mut a = OllamaAssembler::default();
        let evs = a.push_line(r#"{"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"grep","arguments":{"pattern":"foo"}}}]},"done":false}"#);
        assert_eq!(
            evs,
            vec![ChatEvent::ToolCall { id: "ollama-0".into(), name: "grep".into(), args_json: "{\"pattern\":\"foo\"}".into() }]
        );
    }

    #[test]
    fn assembler_done_line_with_usage() {
        let mut a = OllamaAssembler::default();
        let evs = a.push_line(r#"{"message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":11,"eval_count":6}"#);
        assert_eq!(evs, vec![
            ChatEvent::Usage { input_tokens: 11, output_tokens: 6 },
            ChatEvent::Done,
        ]);
        assert!(a.finish().is_empty());
    }

    #[test]
    fn assembler_skips_blank_and_malformed() {
        let mut a = OllamaAssembler::default();
        assert!(a.push_line("").is_empty());
        assert!(a.push_line("{nope").is_empty());
        assert_eq!(a.finish(), vec![ChatEvent::Done]);
    }
}
```

Add to `src-tauri/src/core/providers/mod.rs`:

```rust
pub mod ollama;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test ollama`
Expected: compile errors — `build_body`, `OllamaAssembler` not found.

- [ ] **Step 3: Implement the provider (prepend to `src-tauri/src/core/providers/ollama.rs`)**

```rust
use crate::core::error::Result;
use crate::core::types::{ChatEvent, ContentPart, Message, Role, ToolSpec};
use futures::{Stream, StreamExt};
use serde_json::{json, Value};
use std::pin::Pin;

use super::http::post_stream;
use super::sse::LineDecoder;
use super::Provider;

/// Ollama local backend (`/api/chat`, NDJSON streaming, no API key).
pub struct OllamaProvider {
    client: reqwest::Client,
    base_url: String,
}

impl OllamaProvider {
    pub fn new(base_url: Option<&str>) -> Self {
        OllamaProvider {
            client: reqwest::Client::new(),
            base_url: base_url.unwrap_or("http://localhost:11434").trim_end_matches('/').to_string(),
        }
    }
}

fn concat_text(m: &Message) -> String {
    m.parts
        .iter()
        .filter_map(|p| match p {
            ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

fn ollama_messages(messages: &[Message]) -> Vec<Value> {
    let mut out = Vec::new();
    for m in messages {
        match m.role {
            Role::System => out.push(json!({"role": "system", "content": concat_text(m)})),
            Role::User => out.push(json!({"role": "user", "content": concat_text(m)})),
            Role::Assistant => {
                let mut msg = json!({"role": "assistant", "content": concat_text(m)});
                let calls: Vec<Value> = m
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        ContentPart::ToolCall { name, args_json, .. } => Some(json!({
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": serde_json::from_str::<Value>(args_json).unwrap_or_else(|_| json!({})),
                            }
                        })),
                        _ => None,
                    })
                    .collect();
                if !calls.is_empty() {
                    msg["tool_calls"] = json!(calls);
                }
                out.push(msg);
            }
            Role::Tool => {
                for p in &m.parts {
                    if let ContentPart::ToolResult { content, .. } = p {
                        out.push(json!({"role": "tool", "content": content}));
                    }
                }
            }
        }
    }
    out
}

pub fn build_body(model: &str, messages: &[Message], tools: &[ToolSpec]) -> Value {
    let mut body = json!({
        "model": model,
        "stream": true,
        "messages": ollama_messages(messages),
    });
    if !tools.is_empty() {
        body["tools"] = tools
            .iter()
            .map(|t| json!({
                "type": "function",
                "function": {"name": t.name, "description": t.description, "parameters": t.params_schema}
            }))
            .collect::<Vec<_>>()
            .into();
    }
    body
}

/// Assembles Ollama NDJSON lines into [`ChatEvent`]s.
#[derive(Default)]
pub struct OllamaAssembler {
    call_counter: usize,
    done_emitted: bool,
}

impl OllamaAssembler {
    pub fn push_line(&mut self, line: &str) -> Vec<ChatEvent> {
        let mut out = Vec::new();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return out;
        }
        let v: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => return out,
        };
        if let Some(content) = v["message"]["content"].as_str().filter(|s| !s.is_empty()) {
            out.push(ChatEvent::TextDelta(content.to_string()));
        }
        if let Some(calls) = v["message"]["tool_calls"].as_array() {
            for call in calls {
                let f = &call["function"];
                let id = format!("ollama-{}", self.call_counter);
                self.call_counter += 1;
                out.push(ChatEvent::ToolCall {
                    id,
                    name: f["name"].as_str().unwrap_or("").to_string(),
                    args_json: f.get("arguments").map(|a| a.to_string()).unwrap_or_else(|| "{}".into()),
                });
            }
        }
        if v["done"].as_bool() == Some(true) {
            self.done_emitted = true;
            out.push(ChatEvent::Usage {
                input_tokens: v["prompt_eval_count"].as_u64().unwrap_or(0),
                output_tokens: v["eval_count"].as_u64().unwrap_or(0),
            });
            out.push(ChatEvent::Done);
        }
        out
    }

    pub fn finish(&mut self) -> Vec<ChatEvent> {
        if self.done_emitted {
            vec![]
        } else {
            self.done_emitted = true;
            vec![ChatEvent::Done]
        }
    }
}

#[async_trait::async_trait]
impl Provider for OllamaProvider {
    async fn stream_chat(
        &self,
        model: &str,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatEvent>> + Send>>> {
        let url = format!("{}/api/chat", self.base_url);
        let req = self.client.post(url).json(&build_body(model, messages, tools));
        let chunks = post_stream(req).await?;
        let stream = async_stream::try_stream! {
            let mut decoder = LineDecoder::new();
            let mut assembler = OllamaAssembler::default();
            tokio::pin!(chunks);
            while let Some(chunk) = chunks.next().await {
                let chunk = chunk?;
                for line in decoder.push(&chunk) {
                    for ce in assembler.push_line(&line) {
                        yield ce;
                    }
                }
            }
            if let Some(line) = decoder.finish() {
                for ce in assembler.push_line(&line) {
                    yield ce;
                }
            }
            for ce in assembler.finish() {
                yield ce;
            }
        };
        Ok(Box::pin(stream))
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test ollama`
Expected: `test result: ok. 6 passed`

- [ ] **Step 5: Add the provider factory test — append to the test module in `src-tauri/src/core/providers/mod.rs` (create the test module if absent)**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{ProviderConfig, ProviderKind};

    fn cfg(kind: ProviderKind) -> ProviderConfig {
        ProviderConfig {
            id: "p".into(),
            label: "P".into(),
            kind,
            base_url: None,
            has_key: false,
            models: vec![],
            extra_headers: vec![],
        }
    }

    #[test]
    fn factory_builds_all_kinds() {
        assert!(build_provider(&cfg(ProviderKind::OpenAi), Some("k".into())).is_ok());
        assert!(build_provider(&cfg(ProviderKind::OpenAiCompatible), Some("k".into())).is_ok());
        assert!(build_provider(&cfg(ProviderKind::Anthropic), Some("k".into())).is_ok());
        assert!(build_provider(&cfg(ProviderKind::Gemini), Some("k".into())).is_ok());
        assert!(build_provider(&cfg(ProviderKind::Ollama), None).is_ok());
    }

    #[test]
    fn factory_requires_key_for_anthropic_and_gemini() {
        assert!(build_provider(&cfg(ProviderKind::Anthropic), None).is_err());
        assert!(build_provider(&cfg(ProviderKind::Gemini), None).is_err());
    }

    #[test]
    fn factory_openai_compatible_uses_custom_base_url() {
        let mut c = cfg(ProviderKind::OpenAiCompatible);
        c.base_url = Some("https://api.groq.com/openai/v1".into());
        assert!(build_provider(&c, Some("k".into())).is_ok());
    }
}
```

- [ ] **Step 6: Run factory tests to verify they fail**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test factory`
Expected: compile error — `build_provider` not found.

- [ ] **Step 7: Implement the factory — append to `src-tauri/src/core/providers/mod.rs` (outside the test module)**

```rust
use crate::core::error::Error;
use crate::core::types::ProviderConfig;
use crate::core::types::ProviderKind;

/// Build a provider backend from its persisted config. `api_key` is looked up
/// from the OS keychain by the caller; Anthropic and Gemini require one.
pub fn build_provider(cfg: &ProviderConfig, api_key: Option<String>) -> Result<Box<dyn Provider>> {
    match cfg.kind {
        ProviderKind::OpenAi | ProviderKind::OpenAiCompatible => Ok(Box::new(openai::OpenAiProvider::new(
            cfg.base_url.as_deref(),
            api_key,
            cfg.extra_headers.clone(),
        ))),
        ProviderKind::Anthropic => {
            let key = api_key.ok_or_else(|| Error::Config(format!("provider '{}' requires an API key", cfg.id)))?;
            Ok(Box::new(anthropic::AnthropicProvider::new(cfg.base_url.as_deref(), key)))
        }
        ProviderKind::Gemini => {
            let key = api_key.ok_or_else(|| Error::Config(format!("provider '{}' requires an API key", cfg.id)))?;
            Ok(Box::new(gemini::GeminiProvider::new(cfg.base_url.as_deref(), key)))
        }
        ProviderKind::Ollama => Ok(Box::new(ollama::OllamaProvider::new(cfg.base_url.as_deref()))),
    }
}
```

- [ ] **Step 8: Run tests to verify they pass**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test`
Expected: `test result: ok. 51 passed`

- [ ] **Step 9: Commit**

```bash
cd /b/Jetbrains/projects/kimislop
git add src-tauri
git commit -m "feat(core): Ollama provider and provider factory"
```

---

### Task 9: Tool trait, path sandbox, fs tools

**Files:**
- Create: `src-tauri/src/core/tools/mod.rs`
- Create: `src-tauri/src/core/tools/fs.rs`
- Modify: `src-tauri/src/core/mod.rs` (add `pub mod tools;`)

- [ ] **Step 1: Write the failing tests — create `src-tauri/src/core/tools/mod.rs` containing ONLY**

```rust
pub mod fs;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn sandbox_accepts_relative_paths() {
        let root = Path::new("/ws");
        assert_eq!(resolve_in_workspace(root, "src/main.rs").unwrap(), PathBuf::from("/ws/src/main.rs"));
        assert_eq!(resolve_in_workspace(root, ".").unwrap(), PathBuf::from("/ws"));
    }

    #[test]
    fn sandbox_normalizes_dot_segments() {
        let root = Path::new("/ws");
        assert_eq!(resolve_in_workspace(root, "a/../b").unwrap(), PathBuf::from("/ws/b"));
    }

    #[test]
    fn sandbox_rejects_parent_escape() {
        let root = Path::new("/ws");
        assert!(resolve_in_workspace(root, "../outside").is_err());
        assert!(resolve_in_workspace(root, "a/../../outside").is_err());
    }

    #[test]
    fn sandbox_rejects_absolute_escape() {
        let root = if cfg!(windows) { Path::new("C:\\ws") } else { Path::new("/ws") };
        let evil = if cfg!(windows) { "D:\\other\\x" } else { "/etc/passwd" };
        assert!(resolve_in_workspace(root, evil).is_err());
    }

    #[test]
    fn truncate_output_short_strings_unchanged() {
        assert_eq!(truncate_output("hello", 100), "hello");
    }

    #[test]
    fn truncate_output_long_strings_get_note() {
        let s = "x".repeat(100);
        let out = truncate_output(&s, 10);
        assert!(out.starts_with(&"x".repeat(10)));
        assert!(out.contains("truncated"), "{out}");
    }
}
```

Modify `src-tauri/src/core/mod.rs` — add:

```rust
pub mod tools;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test tools`
Expected: compile errors — `resolve_in_workspace`, `truncate_output` not found. (Note: `tools/fs.rs` does not exist yet; create it as an empty file `pub` so `pub mod fs;` compiles — an empty file is fine.)

- [ ] **Step 3: Implement the sandbox and trait (prepend to `src-tauri/src/core/tools/mod.rs`)**

```rust
use crate::core::error::{Error, Result};
use crate::core::types::ToolSpec;
use std::path::{Component, Path, PathBuf};

/// Shared execution context for tools.
pub struct ToolContext {
    pub workspace_root: PathBuf,
}

/// A capability the agent can invoke via provider tool calls.
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    /// Tools returning true require user approval in `Manual` approval mode.
    fn needs_approval(&self) -> bool {
        false
    }
    async fn execute(&self, ctx: &ToolContext, args_json: &str) -> Result<String>;
}

/// The v1 tool set given to every agent run.
pub fn default_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(fs::ReadFileTool),
        Box::new(fs::WriteFileTool),
        Box::new(fs::ListDirTool),
    ]
}

/// Truncate tool output to `max_bytes` (on a char boundary), noting the cut.
pub fn truncate_output(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n…[truncated {} bytes]", &s[..end], s.len() - end)
}

/// Lexically normalize a path (resolve `.` and `..` without touching the fs).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Resolve `p` inside the workspace root. Rejects paths that escape the root
/// (`..` traversal or absolute paths outside it). Note: this is a lexical
/// check; symlinks inside the workspace pointing outside are not resolved.
pub fn resolve_in_workspace(root: &Path, p: &str) -> Result<PathBuf> {
    let root_n = normalize(root);
    let candidate = Path::new(p);
    let resolved = if candidate.is_absolute() {
        normalize(candidate)
    } else {
        normalize(&root_n.join(candidate))
    };
    if resolved.starts_with(&root_n) {
        Ok(resolved)
    } else {
        Err(Error::Tool(format!("path escapes workspace: {p}")))
    }
}
```

(Create `src-tauri/src/core/tools/fs.rs` as an empty file in this step so the module compiles.)

- [ ] **Step 4: Run sandbox tests to verify they pass**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test tools::tests`
Expected: `test result: ok. 6 passed`

- [ ] **Step 5: Write the failing fs tool tests — set `src-tauri/src/core/tools/fs.rs` to contain ONLY**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tools::{Tool, ToolContext};

    fn ctx() -> (tempfile::TempDir, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let ctx = ToolContext { workspace_root: root.clone() };
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.txt"), "line1\nline2\nline3\nline4\n").unwrap();
        std::fs::write(root.join("top.txt"), "top\n").unwrap();
        (dir, ctx)
    }

    #[tokio::test]
    async fn read_file_full() {
        let (_d, ctx) = ctx();
        let out = ReadFileTool.execute(&ctx, r#"{"path": "src/a.txt"}"#).await.unwrap();
        assert_eq!(out, "line1\nline2\nline3\nline4\n");
    }

    #[tokio::test]
    async fn read_file_offset_limit() {
        let (_d, ctx) = ctx();
        let out = ReadFileTool.execute(&ctx, r#"{"path": "src/a.txt", "offset": 2, "limit": 2}"#).await.unwrap();
        assert!(out.starts_with("line2\nline3"), "{out}");
        assert!(out.contains("more lines"), "{out}");
    }

    #[tokio::test]
    async fn read_file_missing_is_error() {
        let (_d, ctx) = ctx();
        assert!(ReadFileTool.execute(&ctx, r#"{"path": "nope.txt"}"#).await.is_err());
    }

    #[tokio::test]
    async fn read_file_escape_rejected() {
        let (_d, ctx) = ctx();
        assert!(ReadFileTool.execute(&ctx, r#"{"path": "../outside.txt"}"#).await.is_err());
    }

    #[tokio::test]
    async fn write_file_create_and_overwrite() {
        let (_d, ctx) = ctx();
        let out = WriteFileTool.execute(&ctx, r#"{"path": "new/b.txt", "content": "hello", "mode": "create"}"#).await.unwrap();
        assert!(out.contains("5 bytes"), "{out}");
        assert_eq!(std::fs::read_to_string(ctx.workspace_root.join("new/b.txt")).unwrap(), "hello");
        // create fails when file exists
        assert!(WriteFileTool.execute(&ctx, r#"{"path": "new/b.txt", "content": "x", "mode": "create"}"#).await.is_err());
        // overwrite replaces
        WriteFileTool.execute(&ctx, r#"{"path": "new/b.txt", "content": "bye", "mode": "overwrite"}"#).await.unwrap();
        assert_eq!(std::fs::read_to_string(ctx.workspace_root.join("new/b.txt")).unwrap(), "bye");
    }

    #[tokio::test]
    async fn write_file_append() {
        let (_d, ctx) = ctx();
        WriteFileTool.execute(&ctx, r#"{"path": "top.txt", "content": "more\n", "mode": "append"}"#).await.unwrap();
        assert_eq!(std::fs::read_to_string(ctx.workspace_root.join("top.txt")).unwrap(), "top\nmore\n");
    }

    #[tokio::test]
    async fn write_file_needs_approval() {
        assert!(WriteFileTool.needs_approval());
        assert!(!ReadFileTool.needs_approval());
        assert!(!ListDirTool.needs_approval());
    }

    #[tokio::test]
    async fn list_dir_depth() {
        let (_d, ctx) = ctx();
        let out = ListDirTool.execute(&ctx, r#"{"path": ".", "depth": 2}"#).await.unwrap();
        assert!(out.contains("src/"), "{out}");
        assert!(out.contains("a.txt"), "{out}");
        assert!(out.contains("top.txt"), "{out}");
        let shallow = ListDirTool.execute(&ctx, r#"{"path": ".", "depth": 1}"#).await.unwrap();
        assert!(!shallow.contains("a.txt"), "{shallow}");
    }
}
```

- [ ] **Step 6: Run fs tests to verify they fail**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test tools::fs`
Expected: compile errors — `ReadFileTool` etc. not found.

- [ ] **Step 7: Implement the fs tools (prepend to `src-tauri/src/core/tools/fs.rs`)**

```rust
use crate::core::error::{Error, Result};
use crate::core::types::ToolSpec;
use serde::Deserialize;
use serde_json::json;

use super::{resolve_in_workspace, truncate_output, Tool, ToolContext};

const MAX_OUTPUT: usize = 50 * 1024;
const DEFAULT_LINE_LIMIT: usize = 2000;
const MAX_LIST_ENTRIES: usize = 500;

pub struct ReadFileTool;

#[derive(Deserialize)]
struct ReadFileArgs {
    path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

#[async_trait::async_trait]
impl Tool for ReadFileTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_file".into(),
            description: "Read a UTF-8 text file in the workspace. Returns lines with optional 1-based offset and limit.".into(),
            params_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path relative to the workspace root"},
                    "offset": {"type": "integer", "description": "1-based line number to start from"},
                    "limit": {"type": "integer", "description": "Max lines to return (default 2000)"}
                },
                "required": ["path"]
            }),
        }
    }

    async fn execute(&self, ctx: &ToolContext, args_json: &str) -> Result<String> {
        let args: ReadFileArgs = serde_json::from_str(args_json)?;
        let path = resolve_in_workspace(&ctx.workspace_root, &args.path)?;
        let bytes = std::fs::read(&path)
            .map_err(|e| Error::Tool(format!("cannot read {}: {e}", path.display())))?;
        let text = String::from_utf8_lossy(&bytes);
        let offset = args.offset.unwrap_or(1).max(1);
        let limit = args.limit.unwrap_or(DEFAULT_LINE_LIMIT);
        let lines: Vec<&str> = text.lines().collect();
        let total = lines.len();
        let slice: Vec<&str> = lines.iter().skip(offset - 1).take(limit).copied().collect();
        let mut out = slice.join("\n");
        let shown_up_to = offset - 1 + slice.len();
        if shown_up_to < total {
            out.push_str(&format!("\n…[{} more lines]", total - shown_up_to));
        }
        Ok(truncate_output(&out, MAX_OUTPUT))
    }
}

pub struct WriteFileTool;

#[derive(Deserialize)]
struct WriteFileArgs {
    path: String,
    content: String,
    mode: Option<String>,
}

#[async_trait::async_trait]
impl Tool for WriteFileTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write_file".into(),
            description: "Write text to a file in the workspace. mode: create (fail if exists), overwrite (default), append.".into(),
            params_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"},
                    "mode": {"type": "string", "enum": ["create", "overwrite", "append"]}
                },
                "required": ["path", "content"]
            }),
        }
    }

    fn needs_approval(&self) -> bool {
        true
    }

    async fn execute(&self, ctx: &ToolContext, args_json: &str) -> Result<String> {
        let args: WriteFileArgs = serde_json::from_str(args_json)?;
        let path = resolve_in_workspace(&ctx.workspace_root, &args.path)?;
        let mode = args.mode.as_deref().unwrap_or("overwrite");
        if mode == "create" && path.exists() {
            return Err(Error::Tool(format!("file already exists: {}", path.display())));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match mode {
            "create" | "overwrite" => std::fs::write(&path, &args.content)?,
            "append" => {
                use std::io::Write;
                let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
                f.write_all(args.content.as_bytes())?;
            }
            other => return Err(Error::Tool(format!("unknown write mode: {other}"))),
        }
        Ok(format!("wrote {} bytes to {}", args.content.len(), path.display()))
    }
}

pub struct ListDirTool;

#[derive(Deserialize)]
struct ListDirArgs {
    path: Option<String>,
    depth: Option<usize>,
}

#[async_trait::async_trait]
impl Tool for ListDirTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list_dir".into(),
            description: "List files and directories under a workspace path, indented by depth.".into(),
            params_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Default \".\""},
                    "depth": {"type": "integer", "description": "Recursion depth (default 1)"}
                }
            }),
        }
    }

    async fn execute(&self, ctx: &ToolContext, args_json: &str) -> Result<String> {
        let args: ListDirArgs = serde_json::from_str(args_json)?;
        let base = resolve_in_workspace(&ctx.workspace_root, args.path.as_deref().unwrap_or("."))?;
        let depth = args.depth.unwrap_or(1);
        let mut out = Vec::new();
        list_recursive(&base, depth, 0, &mut out);
        if out.len() >= MAX_LIST_ENTRIES {
            out.push(format!("…[capped at {MAX_LIST_ENTRIES} entries]"));
        }
        Ok(out.join("\n"))
    }
}

fn list_recursive(dir: &std::path::Path, depth: usize, level: usize, out: &mut Vec<String>) {
    if out.len() >= MAX_LIST_ENTRIES {
        return;
    }
    let mut entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
        Err(_) => return,
    };
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        if out.len() >= MAX_LIST_ENTRIES {
            return;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let name = entry.file_name().to_string_lossy().to_string();
        let indent = "  ".repeat(level);
        out.push(format!("{indent}{}{}", name, if is_dir { "/" } else { "" }));
        if is_dir && level + 1 < depth {
            list_recursive(&entry.path(), depth, level + 1, out);
        }
    }
}
```

- [ ] **Step 8: Run tests to verify they pass**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test tools`
Expected: `test result: ok. 14 passed`

- [ ] **Step 9: Commit**

```bash
cd /b/Jetbrains/projects/kimislop
git add src-tauri
git commit -m "feat(core): tool trait, workspace path sandbox, fs tools"
```

---

### Task 10: Search tools (grep, glob)

**Files:**
- Create: `src-tauri/src/core/tools/search.rs`
- Modify: `src-tauri/src/core/tools/mod.rs` (add `pub mod search;` and register both tools in `default_tools`)

- [ ] **Step 1: Write the failing tests — create `src-tauri/src/core/tools/search.rs` containing ONLY**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tools::{Tool, ToolContext};

    fn ctx() -> (tempfile::TempDir, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {\n    let needle = 1;\n}\n").unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn helper() {}\n").unwrap();
        std::fs::write(root.join("notes.md"), "a needle in markdown\n").unwrap();
        (dir, ToolContext { workspace_root: root })
    }

    #[tokio::test]
    async fn grep_finds_matches_with_locations() {
        let (_d, ctx) = ctx();
        let out = GrepTool.execute(&ctx, r#"{"pattern": "needle"}"#).await.unwrap();
        assert!(out.contains("src/main.rs:2:"), "{out}");
        assert!(out.contains("notes.md:1:"), "{out}");
        assert!(!out.contains("lib.rs"), "{out}");
    }

    #[tokio::test]
    async fn grep_glob_filter() {
        let (_d, ctx) = ctx();
        let out = GrepTool.execute(&ctx, r#"{"pattern": "needle", "glob": "*.rs"}"#).await.unwrap();
        assert!(out.contains("src/main.rs:2:"), "{out}");
        assert!(!out.contains("notes.md"), "{out}");
    }

    #[tokio::test]
    async fn grep_no_matches() {
        let (_d, ctx) = ctx();
        let out = GrepTool.execute(&ctx, r#"{"pattern": "zzz"}"#).await.unwrap();
        assert!(out.contains("no matches"), "{out}");
    }

    #[tokio::test]
    async fn grep_bad_regex_is_error() {
        let (_d, ctx) = ctx();
        assert!(GrepTool.execute(&ctx, r#"{"pattern": "(["}"#).await.is_err());
    }

    #[tokio::test]
    async fn glob_finds_files() {
        let (_d, ctx) = ctx();
        let out = GlobTool.execute(&ctx, r#"{"pattern": "**/*.rs"}"#).await.unwrap();
        assert!(out.contains("src/main.rs"), "{out}");
        assert!(out.contains("src/lib.rs"), "{out}");
        assert!(!out.contains("notes.md"), "{out}");
    }

    #[tokio::test]
    async fn glob_no_matches() {
        let (_d, ctx) = ctx();
        let out = GlobTool.execute(&ctx, r#"{"pattern": "**/*.xyz"}"#).await.unwrap();
        assert!(out.contains("no matches"), "{out}");
    }
}
```

- [ ] **Step 2: Wire the module — modify `src-tauri/src/core/tools/mod.rs`**

Add at the top:

```rust
pub mod search;
```

Replace `default_tools` with:

```rust
pub fn default_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(fs::ReadFileTool),
        Box::new(fs::WriteFileTool),
        Box::new(fs::ListDirTool),
        Box::new(search::GrepTool),
        Box::new(search::GlobTool),
    ]
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test search`
Expected: compile errors — `GrepTool`, `GlobTool` not found.

- [ ] **Step 4: Implement the search tools (prepend to `src-tauri/src/core/tools/search.rs`)**

```rust
use crate::core::error::{Error, Result};
use crate::core::types::ToolSpec;
use serde::Deserialize;
use serde_json::json;

use super::{resolve_in_workspace, Tool, ToolContext};

const MAX_MATCHES: usize = 200;
const MAX_GLOB_RESULTS: usize = 500;
const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;

pub struct GrepTool;

#[derive(Deserialize)]
struct GrepArgs {
    pattern: String,
    path: Option<String>,
    glob: Option<String>,
}

#[async_trait::async_trait]
impl Tool for GrepTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "grep".into(),
            description: "Regex-search file contents under a workspace path. Output: relpath:line: text.".into(),
            params_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Rust regex"},
                    "path": {"type": "string", "description": "Default \".\""},
                    "glob": {"type": "string", "description": "Filename filter like \"*.rs\""}
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn execute(&self, ctx: &ToolContext, args_json: &str) -> Result<String> {
        let args: GrepArgs = serde_json::from_str(args_json)?;
        let re = regex::Regex::new(&args.pattern)?;
        let base = resolve_in_workspace(&ctx.workspace_root, args.path.as_deref().unwrap_or("."))?;
        let file_glob = args
            .glob
            .as_deref()
            .map(glob::Pattern::new)
            .transpose()
            .map_err(|e| Error::Tool(format!("bad glob: {e}")))?;
        let mut out: Vec<String> = Vec::new();
        for entry in walkdir::WalkDir::new(&base).into_iter().filter_map(|e| e.ok()) {
            if out.len() >= MAX_MATCHES {
                break;
            }
            if !entry.file_type().is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy();
            if let Some(pat) = &file_glob {
                if !pat.matches(&name) {
                    continue;
                }
            }
            if entry.metadata().map(|m| m.len() > MAX_FILE_BYTES).unwrap_or(true) {
                continue;
            }
            let bytes = match std::fs::read(entry.path()) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let text = String::from_utf8_lossy(&bytes);
            let rel = entry.path().strip_prefix(&ctx.workspace_root).unwrap_or(entry.path());
            for (i, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    out.push(format!("{}:{}: {}", rel.display(), i + 1, line.trim_end()));
                    if out.len() >= MAX_MATCHES {
                        break;
                    }
                }
            }
        }
        if out.is_empty() {
            return Ok("no matches".into());
        }
        if out.len() >= MAX_MATCHES {
            out.push(format!("…[capped at {MAX_MATCHES} matches]"));
        }
        Ok(out.join("\n"))
    }
}

pub struct GlobTool;

#[derive(Deserialize)]
struct GlobArgs {
    pattern: String,
}

#[async_trait::async_trait]
impl Tool for GlobTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "glob".into(),
            description: "Find workspace files matching a glob pattern like \"**/*.rs\".".into(),
            params_schema: json!({
                "type": "object",
                "properties": {"pattern": {"type": "string"}},
                "required": ["pattern"]
            }),
        }
    }

    async fn execute(&self, ctx: &ToolContext, args_json: &str) -> Result<String> {
        let args: GlobArgs = serde_json::from_str(args_json)?;
        let full_pattern = ctx.workspace_root.join(&args.pattern);
        let pattern_str = full_pattern.to_string_lossy().replace('\\', "/");
        let paths = glob::glob(&pattern_str).map_err(|e| Error::Tool(format!("bad glob: {e}")))?;
        let mut out: Vec<String> = Vec::new();
        for p in paths.flatten() {
            if !p.starts_with(&ctx.workspace_root) {
                continue;
            }
            let rel = p.strip_prefix(&ctx.workspace_root).unwrap_or(&p);
            out.push(rel.to_string_lossy().replace('\\', "/"));
            if out.len() >= MAX_GLOB_RESULTS {
                break;
            }
        }
        out.sort();
        if out.is_empty() {
            return Ok("no matches".into());
        }
        Ok(out.join("\n"))
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test search`
Expected: `test result: ok. 6 passed`

- [ ] **Step 6: Commit**

```bash
cd /b/Jetbrains/projects/kimislop
git add src-tauri
git commit -m "feat(core): grep and glob tools"
```

---

### Task 11: Shell tool

**Files:**
- Create: `src-tauri/src/core/tools/shell.rs`
- Modify: `src-tauri/src/core/tools/mod.rs` (add `pub mod shell;` and register `shell::RunShellTool` in `default_tools`)

- [ ] **Step 1: Write the failing tests — create `src-tauri/src/core/tools/shell.rs` containing ONLY**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tools::{Tool, ToolContext};

    fn ctx() -> (tempfile::TempDir, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        (dir, ToolContext { workspace_root: root })
    }

    #[tokio::test]
    async fn runs_command_and_captures_output() {
        let (_d, ctx) = ctx();
        let cmd = if cfg!(windows) { "echo hello-sg" } else { "echo hello-sg" };
        let out = RunShellTool.execute(&ctx, &format!(r#"{{"command": "{cmd}"}}"#)).await.unwrap();
        assert!(out.contains("hello-sg"), "{out}");
    }

    #[tokio::test]
    async fn reports_nonzero_exit() {
        let (_d, ctx) = ctx();
        let cmd = if cfg!(windows) { "exit 3" } else { "exit 3" };
        let out = RunShellTool.execute(&ctx, &format!(r#"{{"command": "{cmd}"}}"#)).await.unwrap();
        assert!(out.contains("exit code"), "{out}");
    }

    #[tokio::test]
    async fn times_out_and_kills() {
        let (_d, ctx) = ctx();
        let cmd = if cfg!(windows) { "ping -n 6 127.0.0.1 >nul" } else { "sleep 5" };
        let args = serde_json::json!({"command": cmd, "timeout_secs": 1}).to_string();
        let out = RunShellTool.execute(&ctx, &args).await.unwrap();
        assert!(out.contains("timed out"), "{out}");
    }

    #[tokio::test]
    async fn needs_approval() {
        assert!(RunShellTool.needs_approval());
    }
}
```

- [ ] **Step 2: Wire the module — modify `src-tauri/src/core/tools/mod.rs`**

Add at the top:

```rust
pub mod shell;
```

Replace `default_tools` with:

```rust
pub fn default_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(fs::ReadFileTool),
        Box::new(fs::WriteFileTool),
        Box::new(fs::ListDirTool),
        Box::new(search::GrepTool),
        Box::new(search::GlobTool),
        Box::new(shell::RunShellTool),
    ]
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test shell`
Expected: compile error — `RunShellTool` not found.

- [ ] **Step 4: Implement the shell tool (prepend to `src-tauri/src/core/tools/shell.rs`)**

```rust
use crate::core::error::Result;
use crate::core::types::ToolSpec;
use serde::Deserialize;
use serde_json::json;
use std::process::Stdio;
use std::time::Duration;

use super::{truncate_output, Tool, ToolContext};

const MAX_OUTPUT: usize = 50 * 1024;
const DEFAULT_TIMEOUT_SECS: u64 = 60;
const MAX_TIMEOUT_SECS: u64 = 300;

pub struct RunShellTool;

#[derive(Deserialize)]
struct RunShellArgs {
    command: String,
    timeout_secs: Option<u64>,
}

#[async_trait::async_trait]
impl Tool for RunShellTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "run_shell".into(),
            description: "Run a shell command in the workspace root (cmd /C on Windows, sh -c elsewhere). Captures stdout+stderr.".into(),
            params_schema: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"},
                    "timeout_secs": {"type": "integer", "description": "Default 60, max 300"}
                },
                "required": ["command"]
            }),
        }
    }

    fn needs_approval(&self) -> bool {
        true
    }

    async fn execute(&self, ctx: &ToolContext, args_json: &str) -> Result<String> {
        let args: RunShellArgs = serde_json::from_str(args_json)?;
        let timeout = Duration::from_secs(
            args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS).min(MAX_TIMEOUT_SECS).max(1),
        );
        let child = if cfg!(windows) {
            tokio::process::Command::new("cmd").args(["/C", &args.command])
        } else {
            tokio::process::Command::new("sh").args(["-c", &args.command])
        }
        .current_dir(&ctx.workspace_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

        match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(Ok(output)) => {
                let mut out = String::new();
                out.push_str(&String::from_utf8_lossy(&output.stdout));
                if !output.stderr.is_empty() {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str("[stderr]\n");
                    out.push_str(&String::from_utf8_lossy(&output.stderr));
                }
                if !output.status.success() {
                    out.push_str(&format!("\n[exit code {}]", output.status.code().unwrap_or(-1)));
                }
                if out.is_empty() {
                    out = "[no output]".to_string();
                }
                Ok(truncate_output(&out, MAX_OUTPUT))
            }
            Ok(Err(e)) => Err(e.into()),
            Err(_) => Ok(format!(
                "command timed out after {}s and was killed: {}",
                timeout.as_secs(),
                args.command
            )),
        }
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test shell`
Expected: `test result: ok. 4 passed`

- [ ] **Step 6: Commit**

```bash
cd /b/Jetbrains/projects/kimislop
git add src-tauri
git commit -m "feat(core): shell tool with timeout and kill"
```

---

### Task 12: Approval broker

**Files:**
- Create: `src-tauri/src/core/approvals.rs`
- Modify: `src-tauri/src/core/mod.rs` (add `pub mod approvals;`)

- [ ] **Step 1: Write the failing tests — create `src-tauri/src/core/approvals.rs` containing ONLY**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{AgentEvent, ApprovalMode};
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn auto_mode_approves_immediately_without_event() {
        let (tx, mut rx) = mpsc::channel(8);
        let broker = ApprovalBroker::new(ApprovalMode::Auto, tx);
        let ok = broker.check("call1", "write_file", "{}").await.unwrap();
        assert!(ok);
        assert!(rx.try_recv().is_err(), "no event in auto mode");
    }

    #[tokio::test]
    async fn manual_mode_emits_request_and_waits_for_allow() {
        let (tx, mut rx) = mpsc::channel(8);
        let broker = std::sync::Arc::new(ApprovalBroker::new(ApprovalMode::Manual, tx));
        let b2 = broker.clone();
        let handle = tokio::spawn(async move { b2.check("call1", "run_shell", "{\"command\":\"ls\"}").await });
        let ev = rx.recv().await.unwrap();
        let request_id = match ev {
            AgentEvent::ApprovalRequested { request_id, tool_call_id, name, .. } => {
                assert_eq!(tool_call_id, "call1");
                assert_eq!(name, "run_shell");
                request_id
            }
            other => panic!("unexpected event: {other:?}"),
        };
        broker.resolve(&request_id, true).unwrap();
        assert!(handle.await.unwrap().unwrap());
    }

    #[tokio::test]
    async fn manual_mode_deny_returns_false() {
        let (tx, mut rx) = mpsc::channel(8);
        let broker = std::sync::Arc::new(ApprovalBroker::new(ApprovalMode::Manual, tx));
        let b2 = broker.clone();
        let handle = tokio::spawn(async move { b2.check("c", "write_file", "{}").await });
        let request_id = match rx.recv().await.unwrap() {
            AgentEvent::ApprovalRequested { request_id, .. } => request_id,
            other => panic!("unexpected: {other:?}"),
        };
        broker.resolve(&request_id, false).unwrap();
        assert!(!handle.await.unwrap().unwrap());
    }

    #[tokio::test]
    async fn resolve_unknown_request_errors() {
        let (tx, _rx) = mpsc::channel(8);
        let broker = ApprovalBroker::new(ApprovalMode::Manual, tx);
        assert!(broker.resolve("nope", true).is_err());
    }

    #[tokio::test]
    async fn mode_switch_takes_effect_on_next_check() {
        let (tx, _rx) = mpsc::channel(8);
        let broker = ApprovalBroker::new(ApprovalMode::Manual, tx);
        assert_eq!(broker.mode(), ApprovalMode::Manual);
        broker.set_mode(ApprovalMode::Auto);
        assert_eq!(broker.mode(), ApprovalMode::Auto);
        assert!(broker.check("c", "write_file", "{}").await.unwrap());
    }
}
```

Modify `src-tauri/src/core/mod.rs` — add:

```rust
pub mod approvals;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test approvals`
Expected: compile error — `ApprovalBroker` not found.

- [ ] **Step 3: Implement the broker (prepend to `src-tauri/src/core/approvals.rs`)**

```rust
use crate::core::error::{Error, Result};
use crate::core::types::{AgentEvent, ApprovalMode};
use std::collections::HashMap;
use std::sync::{Mutex, RwLock};
use tokio::sync::{mpsc, oneshot};

/// Gates approval-requiring tool calls on the user's decision.
/// In `Auto` mode every check passes immediately. In `Manual` mode the broker
/// emits `AgentEvent::ApprovalRequested` and blocks until `resolve` is called.
pub struct ApprovalBroker {
    mode: RwLock<ApprovalMode>,
    pending: Mutex<HashMap<String, oneshot::Sender<bool>>>,
    events: mpsc::Sender<AgentEvent>,
}

impl ApprovalBroker {
    pub fn new(mode: ApprovalMode, events: mpsc::Sender<AgentEvent>) -> Self {
        ApprovalBroker { mode: RwLock::new(mode), pending: Mutex::new(HashMap::new()), events }
    }

    pub fn mode(&self) -> ApprovalMode {
        *self.mode.read().unwrap()
    }

    pub fn set_mode(&self, mode: ApprovalMode) {
        *self.mode.write().unwrap() = mode;
    }

    /// Returns true when the call may proceed. Mode is read per check, so a
    /// mid-run mode switch applies to the next tool call.
    pub async fn check(&self, tool_call_id: &str, name: &str, args_json: &str) -> Result<bool> {
        if self.mode() == ApprovalMode::Auto {
            return Ok(true);
        }
        let request_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(request_id.clone(), tx);
        self.events
            .send(AgentEvent::ApprovalRequested {
                request_id,
                tool_call_id: tool_call_id.to_string(),
                name: name.to_string(),
                args_json: args_json.to_string(),
            })
            .await
            .map_err(|_| Error::ApprovalClosed)?;
        rx.await.map_err(|_| Error::ApprovalClosed)
    }

    /// Resolve a pending request (called from the UI via the bridge).
    pub fn resolve(&self, request_id: &str, allow: bool) -> Result<()> {
        let tx = self
            .pending
            .lock()
            .unwrap()
            .remove(request_id)
            .ok_or_else(|| Error::Tool(format!("unknown approval request: {request_id}")))?;
        tx.send(allow).map_err(|_| Error::ApprovalClosed)
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test approvals`
Expected: `test result: ok. 5 passed`

- [ ] **Step 5: Commit**

```bash
cd /b/Jetbrains/projects/kimislop
git add src-tauri
git commit -m "feat(core): approval broker with manual/auto modes"
```

---

### Task 13: Agent loop

**Files:**
- Create: `src-tauri/src/core/agent.rs`
- Modify: `src-tauri/src/core/mod.rs` (add `pub mod agent;`)

- [ ] **Step 1: Write the failing tests — create `src-tauri/src/core/agent.rs` containing ONLY**

```rust
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
        let provider_script_len = 2;
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
        let _ = provider_script_len;
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
}
```

Modify `src-tauri/src/core/mod.rs` — add:

```rust
pub mod agent;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test agent`
Expected: compile errors — `AgentRequest`, `run` not found.

- [ ] **Step 3: Implement the agent loop (prepend to `src-tauri/src/core/agent.rs`)**

```rust
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
                        match req.approvals.check(&id, &name, &args_json).await {
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test agent`
Expected: `test result: ok. 6 passed`

- [ ] **Step 5: Commit**

```bash
cd /b/Jetbrains/projects/kimislop
git add src-tauri
git commit -m "feat(core): agent tool-call loop with approvals and cancellation"
```

---

### Task 14: SQLite store

**Files:**
- Create: `src-tauri/src/core/store.rs`
- Modify: `src-tauri/src/core/mod.rs` (add `pub mod store;`)

- [ ] **Step 1: Write the failing tests — create `src-tauri/src/core/store.rs` containing ONLY**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{ApprovalMode, ContentPart, Message, ProviderConfig, ProviderKind, Role};

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    #[test]
    fn migrations_set_user_version() {
        let s = store();
        let v: i64 = s.conn.lock().unwrap().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 1);
    }

    #[test]
    fn workspace_crud() {
        let s = store();
        let id = s.add_workspace("proj", "/tmp/proj").unwrap();
        let list = s.list_workspaces().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "proj");
        assert_eq!(list[0].path, "/tmp/proj");
        s.remove_workspace(&id).unwrap();
        assert!(s.list_workspaces().unwrap().is_empty());
    }

    #[test]
    fn workspace_path_unique() {
        let s = store();
        s.add_workspace("a", "/tmp/x").unwrap();
        assert!(s.add_workspace("b", "/tmp/x").is_err());
    }

    #[test]
    fn conversation_lifecycle() {
        let s = store();
        let ws = s.add_workspace("proj", "/tmp/proj").unwrap();
        let cid = s.create_conversation(&ws, "Fix bug", "openai", "gpt-5", ApprovalMode::Manual).unwrap();
        let conv = s.get_conversation(&cid).unwrap();
        assert_eq!(conv.title, "Fix bug");
        assert_eq!(conv.approval_mode, ApprovalMode::Manual);
        s.rename_conversation(&cid, "Fix login bug").unwrap();
        s.set_approval_mode(&cid, ApprovalMode::Auto).unwrap();
        let conv = s.get_conversation(&cid).unwrap();
        assert_eq!(conv.title, "Fix login bug");
        assert_eq!(conv.approval_mode, ApprovalMode::Auto);
        let list = s.list_conversations(&ws).unwrap();
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn messages_roundtrip_with_tool_parts() {
        let s = store();
        let ws = s.add_workspace("proj", "/tmp/proj").unwrap();
        let cid = s.create_conversation(&ws, "c", "openai", "m", ApprovalMode::Auto).unwrap();
        let msgs = vec![
            Message::text(Role::User, "read x"),
            Message {
                role: Role::Assistant,
                parts: vec![
                    ContentPart::Text { text: "reading".into() },
                    ContentPart::ToolCall { id: "c1".into(), name: "read_file".into(), args_json: "{}".into() },
                ],
            },
            Message {
                role: Role::Tool,
                parts: vec![ContentPart::ToolResult { tool_call_id: "c1".into(), content: "data".into(), is_error: false }],
            },
        ];
        for m in &msgs {
            s.append_message(&cid, m).unwrap();
        }
        let back = s.get_messages(&cid).unwrap();
        assert_eq!(back, msgs);
    }

    #[test]
    fn workspace_delete_cascades() {
        let s = store();
        let ws = s.add_workspace("proj", "/tmp/proj").unwrap();
        let cid = s.create_conversation(&ws, "c", "openai", "m", ApprovalMode::Auto).unwrap();
        s.append_message(&cid, &Message::text(Role::User, "hi")).unwrap();
        s.remove_workspace(&ws).unwrap();
        assert!(s.list_conversations(&ws).unwrap().is_empty());
        assert!(s.get_messages(&cid).unwrap().is_empty());
    }

    #[test]
    fn provider_upsert_list_delete() {
        let s = store();
        let cfg = ProviderConfig {
            id: "openai".into(),
            label: "OpenAI".into(),
            kind: ProviderKind::OpenAi,
            base_url: None,
            has_key: true,
            models: vec!["gpt-5".into()],
            extra_headers: vec![],
        };
        s.upsert_provider(&cfg).unwrap();
        let mut updated = cfg.clone();
        updated.has_key = false;
        updated.models = vec!["gpt-5".into(), "gpt-5-mini".into()];
        s.upsert_provider(&updated).unwrap();
        let list = s.list_providers().unwrap();
        assert_eq!(list.len(), 1, "upsert must not duplicate");
        assert_eq!(list[0], updated);
        s.delete_provider("openai").unwrap();
        assert!(s.list_providers().unwrap().is_empty());
    }
}
```

Modify `src-tauri/src/core/mod.rs` — add:

```rust
pub mod store;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test store`
Expected: compile errors — `Store`, `open_in_memory` not found.

- [ ] **Step 3: Implement the store (prepend to `src-tauri/src/core/store.rs`)**

```rust
use crate::core::error::Result;
use crate::core::types::{ApprovalMode, Message, ProviderConfig, ProviderKind, Role};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceRow {
    pub id: String,
    pub name: String,
    pub path: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationRow {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
    pub provider_id: String,
    pub model: String,
    pub approval_mode: ApprovalMode,
    pub created_at: i64,
    pub updated_at: i64,
}

pub struct Store {
    pub(crate) conn: Mutex<Connection>,
}

fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn mode_str(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Manual => "manual",
        ApprovalMode::Auto => "auto",
    }
}

fn str_mode(s: &str) -> ApprovalMode {
    match s {
        "auto" => ApprovalMode::Auto,
        _ => ApprovalMode::Manual,
    }
}

fn kind_str(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::OpenAi => "open_ai",
        ProviderKind::Anthropic => "anthropic",
        ProviderKind::Gemini => "gemini",
        ProviderKind::Ollama => "ollama",
        ProviderKind::OpenAiCompatible => "open_ai_compatible",
    }
}

fn str_kind(s: &str) -> ProviderKind {
    match s {
        "anthropic" => ProviderKind::Anthropic,
        "gemini" => ProviderKind::Gemini,
        "ollama" => ProviderKind::Ollama,
        "open_ai_compatible" => ProviderKind::OpenAiCompatible,
        _ => ProviderKind::OpenAi,
    }
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        let conn = Connection::open(path)?;
        let store = Store { conn: Mutex::new(conn) };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Store> {
        let conn = Connection::open_in_memory()?;
        let store = Store { conn: Mutex::new(conn) };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS workspaces(
               id TEXT PRIMARY KEY,
               name TEXT NOT NULL,
               path TEXT NOT NULL UNIQUE,
               created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS conversations(
               id TEXT PRIMARY KEY,
               workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
               title TEXT NOT NULL,
               provider_id TEXT NOT NULL,
               model TEXT NOT NULL,
               approval_mode TEXT NOT NULL CHECK(approval_mode IN ('manual','auto')),
               created_at INTEGER NOT NULL,
               updated_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS messages(
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
               role TEXT NOT NULL,
               parts_json TEXT NOT NULL,
               created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS providers(
               id TEXT PRIMARY KEY,
               label TEXT NOT NULL,
               kind TEXT NOT NULL,
               base_url TEXT,
               has_key INTEGER NOT NULL DEFAULT 0,
               models_json TEXT NOT NULL,
               extra_headers_json TEXT NOT NULL DEFAULT '[]'
             );
             PRAGMA user_version = 1;",
        )?;
        Ok(())
    }

    pub fn add_workspace(&self, name: &str, path: &str) -> Result<String> {
        let id = new_id();
        self.conn.lock().unwrap().execute(
            "INSERT INTO workspaces(id, name, path, created_at) VALUES(?1, ?2, ?3, ?4)",
            params![id, name, path, now_ts()],
        )?;
        Ok(id)
    }

    pub fn list_workspaces(&self) -> Result<Vec<WorkspaceRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, name, path, created_at FROM workspaces ORDER BY created_at")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(WorkspaceRow { id: r.get(0)?, name: r.get(1)?, path: r.get(2)?, created_at: r.get(3)? })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn remove_workspace(&self, id: &str) -> Result<()> {
        self.conn.lock().unwrap().execute("DELETE FROM workspaces WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn create_conversation(
        &self,
        workspace_id: &str,
        title: &str,
        provider_id: &str,
        model: &str,
        mode: ApprovalMode,
    ) -> Result<String> {
        let id = new_id();
        let now = now_ts();
        self.conn.lock().unwrap().execute(
            "INSERT INTO conversations(id, workspace_id, title, provider_id, model, approval_mode, created_at, updated_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, workspace_id, title, provider_id, model, mode_str(mode), now, now],
        )?;
        Ok(id)
    }

    pub fn get_conversation(&self, id: &str) -> Result<ConversationRow> {
        let conn = self.conn.lock().unwrap();
        let row = conn.query_row(
            "SELECT id, workspace_id, title, provider_id, model, approval_mode, created_at, updated_at
             FROM conversations WHERE id = ?1",
            params![id],
            |r| {
                Ok(ConversationRow {
                    id: r.get(0)?,
                    workspace_id: r.get(1)?,
                    title: r.get(2)?,
                    provider_id: r.get(3)?,
                    model: r.get(4)?,
                    approval_mode: str_mode(&r.get::<_, String>(5)?),
                    created_at: r.get(6)?,
                    updated_at: r.get(7)?,
                })
            },
        )?;
        Ok(row)
    }

    pub fn list_conversations(&self, workspace_id: &str) -> Result<Vec<ConversationRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, title, provider_id, model, approval_mode, created_at, updated_at
             FROM conversations WHERE workspace_id = ?1 ORDER BY updated_at DESC",
        )?;
        let rows = stmt
            .query_map(params![workspace_id], |r| {
                Ok(ConversationRow {
                    id: r.get(0)?,
                    workspace_id: r.get(1)?,
                    title: r.get(2)?,
                    provider_id: r.get(3)?,
                    model: r.get(4)?,
                    approval_mode: str_mode(&r.get::<_, String>(5)?),
                    created_at: r.get(6)?,
                    updated_at: r.get(7)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn rename_conversation(&self, id: &str, title: &str) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute("UPDATE conversations SET title = ?1, updated_at = ?2 WHERE id = ?3", params![title, now_ts(), id])?;
        Ok(())
    }

    pub fn set_approval_mode(&self, id: &str, mode: ApprovalMode) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE conversations SET approval_mode = ?1, updated_at = ?2 WHERE id = ?3",
            params![mode_str(mode), now_ts(), id],
        )?;
        Ok(())
    }

    pub fn append_message(&self, conversation_id: &str, msg: &Message) -> Result<()> {
        let role = match msg.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        };
        let parts_json = serde_json::to_string(&msg.parts)?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO messages(conversation_id, role, parts_json, created_at) VALUES(?1, ?2, ?3, ?4)",
            params![conversation_id, role, parts_json, now_ts()],
        )?;
        conn.execute("UPDATE conversations SET updated_at = ?1 WHERE id = ?2", params![now_ts(), conversation_id])?;
        Ok(())
    }

    pub fn get_messages(&self, conversation_id: &str) -> Result<Vec<Message>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT role, parts_json FROM messages WHERE conversation_id = ?1 ORDER BY id")?;
        let rows = stmt
            .query_map(params![conversation_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut out = Vec::new();
        for (role, parts_json) in rows {
            let role = match role.as_str() {
                "system" => Role::System,
                "assistant" => Role::Assistant,
                "tool" => Role::Tool,
                _ => Role::User,
            };
            let parts = serde_json::from_str(&parts_json)?;
            out.push(Message { role, parts });
        }
        Ok(out)
    }

    pub fn upsert_provider(&self, cfg: &ProviderConfig) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO providers(id, label, kind, base_url, has_key, models_json, extra_headers_json)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET label=excluded.label, kind=excluded.kind,
               base_url=excluded.base_url, has_key=excluded.has_key,
               models_json=excluded.models_json, extra_headers_json=excluded.extra_headers_json",
            params![
                cfg.id,
                cfg.label,
                kind_str(cfg.kind),
                cfg.base_url,
                cfg.has_key as i64,
                serde_json::to_string(&cfg.models)?,
                serde_json::to_string(&cfg.extra_headers)?,
            ],
        )?;
        Ok(())
    }

    pub fn list_providers(&self) -> Result<Vec<ProviderConfig>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT id, label, kind, base_url, has_key, models_json, extra_headers_json FROM providers ORDER BY label")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut out = Vec::new();
        for (id, label, kind, base_url, has_key, models_json, extra_headers_json) in rows {
            out.push(ProviderConfig {
                id,
                label,
                kind: str_kind(&kind),
                base_url,
                has_key: has_key != 0,
                models: serde_json::from_str(&models_json)?,
                extra_headers: serde_json::from_str(&extra_headers_json)?,
            });
        }
        Ok(out)
    }

    pub fn delete_provider(&self, id: &str) -> Result<()> {
        self.conn.lock().unwrap().execute("DELETE FROM providers WHERE id = ?1", params![id])?;
        Ok(())
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test store`
Expected: `test result: ok. 7 passed`

- [ ] **Step 5: Commit**

```bash
cd /b/Jetbrains/projects/kimislop
git add src-tauri
git commit -m "feat(core): SQLite store with migrations"
```

---

### Task 15: Config and key stores

**Files:**
- Create: `src-tauri/src/core/config.rs`
- Modify: `src-tauri/src/core/mod.rs` (add `pub mod config;`)

- [ ] **Step 1: Write the failing tests — create `src-tauri/src/core/config.rs` containing ONLY**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mem_keystore_roundtrip() {
        let ks = MemKeyStore::new();
        assert_eq!(ks.get("openai").unwrap(), None);
        ks.set("openai", "sk-test").unwrap();
        assert_eq!(ks.get("openai").unwrap(), Some("sk-test".into()));
        ks.delete("openai").unwrap();
        assert_eq!(ks.get("openai").unwrap(), None);
    }

    #[test]
    fn mem_keystore_delete_missing_is_ok() {
        let ks = MemKeyStore::new();
        assert!(ks.delete("nope").is_ok());
    }

    #[test]
    fn app_config_save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let cfg = AppConfig { last_workspace_id: Some("w1".into()), last_conversation_id: Some("c1".into()) };
        cfg.save(&path).unwrap();
        let back = AppConfig::load(&path).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn app_config_missing_file_is_default() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = AppConfig::load(&dir.path().join("nope.toml")).unwrap();
        assert_eq!(cfg, AppConfig::default());
    }
}
```

Modify `src-tauri/src/core/mod.rs` — add:

```rust
pub mod config;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test config`
Expected: compile errors — `MemKeyStore`, `AppConfig` not found.

- [ ] **Step 3: Implement config (prepend to `src-tauri/src/core/config.rs`)**

```rust
use crate::core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const KEYRING_SERVICE: &str = "supergravity";

/// API-key storage. Keys are addressed by provider id.
pub trait KeyStore: Send + Sync {
    fn get(&self, provider_id: &str) -> Result<Option<String>>;
    fn set(&self, provider_id: &str, key: &str) -> Result<()>;
    fn delete(&self, provider_id: &str) -> Result<()>;
}

/// OS keychain-backed store (Windows Credential Manager / macOS Keychain / Secret Service).
pub struct OsKeyStore;

impl KeyStore for OsKeyStore {
    fn get(&self, provider_id: &str) -> Result<Option<String>> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, provider_id).map_err(|e| Error::Config(e.to_string()))?;
        match entry.get_password() {
            Ok(p) => Ok(Some(p)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(Error::Config(e.to_string())),
        }
    }

    fn set(&self, provider_id: &str, key: &str) -> Result<()> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, provider_id).map_err(|e| Error::Config(e.to_string()))?;
        entry.set_password(key).map_err(|e| Error::Config(e.to_string()))
    }

    fn delete(&self, provider_id: &str) -> Result<()> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, provider_id).map_err(|e| Error::Config(e.to_string()))?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(Error::Config(e.to_string())),
        }
    }
}

/// In-memory store for tests and headless development.
pub struct MemKeyStore {
    inner: Mutex<HashMap<String, String>>,
}

impl MemKeyStore {
    pub fn new() -> Self {
        MemKeyStore { inner: Mutex::new(HashMap::new()) }
    }
}

impl Default for MemKeyStore {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyStore for MemKeyStore {
    fn get(&self, provider_id: &str) -> Result<Option<String>> {
        Ok(self.inner.lock().unwrap().get(provider_id).cloned())
    }

    fn set(&self, provider_id: &str, key: &str) -> Result<()> {
        self.inner.lock().unwrap().insert(provider_id.to_string(), key.to_string());
        Ok(())
    }

    fn delete(&self, provider_id: &str) -> Result<()> {
        self.inner.lock().unwrap().remove(provider_id);
        Ok(())
    }
}

/// Non-secret app config persisted as TOML.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AppConfig {
    pub last_workspace_id: Option<String>,
    pub last_conversation_id: Option<String>,
}

impl AppConfig {
    pub fn load(path: &Path) -> Result<AppConfig> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| Error::Config(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AppConfig::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| Error::Config(e.to_string()))?;
        std::fs::write(path, text)?;
        Ok(())
    }
}

/// Platform app-data directory for supergravity (e.g. `%APPDATA%/supergravity`).
pub fn data_dir() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "supergravity")
        .ok_or_else(|| Error::Config("cannot determine app data dir".into()))?;
    Ok(dirs.data_dir().to_path_buf())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test config`
Expected: `test result: ok. 4 passed`

- [ ] **Step 5: Commit**

```bash
cd /b/Jetbrains/projects/kimislop
git add src-tauri
git commit -m "feat(core): app config and OS/mem key stores"
```

---

### Task 16: Final verification and cleanup

**Files:**
- Modify: `src-tauri/src/core/mod.rs` (final module list + re-exports)

- [ ] **Step 1: Finalize `src-tauri/src/core/mod.rs`**

Full content:

```rust
pub mod agent;
pub mod approvals;
pub mod config;
pub mod error;
pub mod providers;
pub mod store;
pub mod tools;
pub mod types;

pub use error::{Error, Result};
```

(Keep the `scaffold_smoke` test module if present, or remove it — either is fine; prefer removing it now that real tests exist. Delete the `mod tests` block.)

- [ ] **Step 2: Full test suite**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test`
Expected: all tests pass, 0 failed (≈96 passed; exact count may drift by ±1 if the scaffold smoke test was kept or removed)

- [ ] **Step 3: Clippy clean**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo clippy --all-targets -- -D warnings`
Expected: no warnings, exit 0. Fix any lints minimally (e.g. `needless_borrow`, `redundant_clone`).

- [ ] **Step 4: Format**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo fmt`
Then re-run `cargo test` to confirm still green.

- [ ] **Step 5: Commit**

```bash
cd /b/Jetbrains/projects/kimislop
git add src-tauri
git commit -m "chore(core): finalize module exports, clippy/fmt clean"
```

---

## Done criteria for this plan

- `cargo test` in `src-tauri/`: all green
- `cargo clippy --all-targets -- -D warnings`: clean
- No Tauri dependency anywhere in `src-tauri/src/core/` (verify: `grep -ri tauri src-tauri/src/core` returns nothing)
- Follow-up plan (`2026-07-17-supergravity-ui.md`) builds the Tauri bridge + vanilla JS UI on top of this core.
