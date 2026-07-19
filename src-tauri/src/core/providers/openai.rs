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
    pub fn new(
        base_url: Option<&str>,
        api_key: Option<String>,
        extra_headers: Vec<(String, String)>,
    ) -> Self {
        OpenAiProvider {
            client: reqwest::Client::new(),
            base_url: base_url
                .unwrap_or("https://api.openai.com/v1")
                .trim_end_matches('/')
                .to_string(),
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
                msg["content"] = if text.is_empty() {
                    Value::Null
                } else {
                    json!(text)
                };
                let calls: Vec<Value> = m
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        ContentPart::ToolCall {
                            id,
                            name,
                            args_json,
                        } => Some(json!({
                            "id": id,
                            "type": "function",
                            // History may contain malformed args from older runs;
                            // llama.cpp-style templates hard-fail on those.
                            "function": {"name": name, "arguments": crate::core::types::sanitize_args_json(args_json)}
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
                    if let ContentPart::ToolResult {
                        tool_call_id,
                        content,
                        ..
                    } = p
                    {
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
        "stream_options": {"include_usage": true},
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
            if !self.done_emitted {
                self.done_emitted = true;
                out.push(ChatEvent::Done);
            }
            return out;
        }
        let v: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => return out,
        };
        if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
            let msg = err["message"].as_str().unwrap_or("unknown provider error");
            out.push(ChatEvent::Error(msg.to_string()));
            return out;
        }
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
                        out.push(ChatEvent::ToolCall {
                            id: buf.id,
                            name: buf.name,
                            args_json: buf.args,
                        });
                    }
                }
            }
        }
        out
    }

    /// Call when the byte stream ends; flushes any buffered tool calls (some
    /// OpenAI-compatible servers never send `finish_reason: "tool_calls"`) and
    /// emits `Done` if `[DONE]` never arrived.
    pub fn finish(&mut self) -> Vec<ChatEvent> {
        let mut out = Vec::new();
        for (_, buf) in std::mem::take(&mut self.tool_calls) {
            out.push(ChatEvent::ToolCall {
                id: buf.id,
                name: buf.name,
                args_json: buf.args,
            });
        }
        if !self.done_emitted {
            self.done_emitted = true;
            out.push(ChatEvent::Done);
        }
        out
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
        let mut req = self
            .client
            .post(url)
            .json(&build_body(model, messages, tools));
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
        assert_eq!(
            body["messages"],
            serde_json::json!([{"role": "user", "content": "hi"}])
        );
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
                    ContentPart::Text {
                        text: "reading".into(),
                    },
                    ContentPart::ToolCall {
                        id: "call_1".into(),
                        name: "read_file".into(),
                        args_json: "{\"path\":\"x\"}".into(),
                    },
                ],
            },
            Message {
                role: Role::Tool,
                parts: vec![ContentPart::ToolResult {
                    tool_call_id: "call_1".into(),
                    content: "file body".into(),
                    is_error: false,
                }],
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
    fn body_sanitizes_malformed_tool_arguments() {
        // Old history may hold fused/truncated args; they must never reach the
        // provider raw (llama.cpp templates hard-fail on them).
        let msgs = vec![Message {
            role: Role::Assistant,
            parts: vec![ContentPart::ToolCall {
                id: "call_1".into(),
                name: "read_file".into(),
                args_json: r#"{"limit":20}{"limit":30}"#.into(),
            }],
        }];
        let body = build_body("m", &msgs, &[]);
        assert_eq!(
            body["messages"][0]["tool_calls"][0]["function"]["arguments"],
            serde_json::json!(r#"{"limit":20}"#)
        );
    }

    #[test]
    fn assistant_without_text_has_null_content() {
        let msgs = vec![Message {
            role: Role::Assistant,
            parts: vec![ContentPart::ToolCall {
                id: "c".into(),
                name: "n".into(),
                args_json: "{}".into(),
            }],
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
            vec![ChatEvent::ToolCall {
                id: "call_9".into(),
                name: "read_file".into(),
                args_json: "{\"path\":\"x\"}".into()
            }]
        );
    }

    #[test]
    fn assembler_usage_and_malformed_tolerance() {
        let mut a = OpenAiAssembler::default();
        let evs =
            a.push_data(r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":4}}"#);
        assert_eq!(
            evs,
            vec![ChatEvent::Usage {
                input_tokens: 10,
                output_tokens: 4
            }]
        );
        assert!(
            a.push_data("{not json").is_empty(),
            "malformed chunks are skipped"
        );
    }

    #[test]
    fn assembler_finish_emits_done_when_stream_ends_silently() {
        let mut a = OpenAiAssembler::default();
        a.push_data(r#"{"choices":[{"delta":{"content":"x"},"finish_reason":"stop"}]}"#);
        assert_eq!(a.finish(), vec![ChatEvent::Done]);
    }

    #[test]
    fn assembler_finish_flushes_pending_tool_calls() {
        // Some OpenAI-compatible servers end tool-call streams with "stop" or
        // without any finish_reason — buffered calls must not be lost.
        let mut a = OpenAiAssembler::default();
        a.push_data(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read_file","arguments":"{}"}}]},"finish_reason":"stop"}]}"#);
        let evs = a.finish();
        assert_eq!(
            evs,
            vec![
                ChatEvent::ToolCall {
                    id: "call_1".into(),
                    name: "read_file".into(),
                    args_json: "{}".into()
                },
                ChatEvent::Done,
            ]
        );
    }

    #[test]
    fn assembler_ignores_duplicate_done() {
        let mut a = OpenAiAssembler::default();
        assert_eq!(a.push_data("[DONE]"), vec![ChatEvent::Done]);
        assert!(a.push_data("[DONE]").is_empty());
        assert!(a.finish().is_empty());
    }

    #[test]
    fn assembler_error_payload() {
        let mut a = OpenAiAssembler::default();
        let evs = a.push_data(r#"{"error":{"message":"rate limited","type":"rate_limit_error"}}"#);
        assert_eq!(evs, vec![ChatEvent::Error("rate limited".into())]);
    }
}
