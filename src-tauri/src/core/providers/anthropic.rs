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
            // Anthropic rejects empty content arrays — skip block-less messages.
            if blocks.is_empty() {
                continue;
            }
            // Anthropic requires strictly alternating roles — merge consecutive same-role turns.
            if let Some(last) = msgs.last_mut() {
                if last["role"].as_str() == Some(role.as_str()) {
                    if let Some(arr) = last["content"].as_array_mut() {
                        arr.extend(blocks);
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
    started: bool,
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
                self.started = true;
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
                if self.done_emitted {
                    return out;
                }
                self.done_emitted = true;
                out.push(ChatEvent::Usage { input_tokens: self.input_tokens, output_tokens: self.output_tokens });
                out.push(ChatEvent::Done);
            }
            "error" => {
                // Server-side failure (e.g. overloaded) — ends the stream; no Done after this.
                self.done_emitted = true;
                let msg = v["error"]["message"].as_str().unwrap_or("unknown anthropic error");
                out.push(ChatEvent::Error(msg.to_string()));
            }
            _ => {}
        }
        out
    }

    /// Emit `Done` when a started stream ended without `message_stop`.
    pub fn finish(&mut self) -> Vec<ChatEvent> {
        if self.started && !self.done_emitted {
            self.done_emitted = true;
            vec![ChatEvent::Done]
        } else {
            vec![]
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

    #[test]
    fn body_skips_messages_with_no_blocks() {
        let msgs = vec![
            Message { role: Role::User, parts: vec![] },
            Message::text(Role::User, "real"),
        ];
        let body = build_body("m", &msgs, &[]);
        assert_eq!(
            body["messages"],
            serde_json::json!([{"role": "user", "content": [{"type": "text", "text": "real"}]}])
        );
    }

    #[test]
    fn assembler_truncated_stream_finish_emits_done_once() {
        let mut a = AnthropicAssembler::default();
        assert!(a.push(&ev("message_start", r#"{"message":{"usage":{"input_tokens":5}}}"#)).is_empty());
        assert_eq!(a.push(&ev("content_block_delta", r#"{"index":0,"delta":{"type":"text_delta","text":"Hi"}}"#)).len(), 1);
        assert_eq!(a.finish(), vec![ChatEvent::Done]);
        assert!(a.finish().is_empty());
    }

    #[test]
    fn assembler_error_event() {
        let mut a = AnthropicAssembler::default();
        let evs = a.push(&ev("error", r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#));
        assert_eq!(evs, vec![ChatEvent::Error("Overloaded".into())]);
        assert!(a.finish().is_empty(), "no Done after an error frame");
    }
}
