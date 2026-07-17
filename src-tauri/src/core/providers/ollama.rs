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
        if let Some(err) = v.get("error").and_then(Value::as_str) {
            // Mid-stream server failure (model OOM, template error) — no Done after this.
            self.done_emitted = true;
            out.push(ChatEvent::Error(err.to_string()));
            return out;
        }
        if let Some(content) = v["message"]["content"].as_str().filter(|s| !s.is_empty()) {
            out.push(ChatEvent::TextDelta(content.to_string()));
        }
        if let Some(calls) = v["message"]["tool_calls"].as_array() {
            for call in calls {
                let f = &call["function"];
                // Unique per call for cross-turn consistency with other providers
                // (Ollama's wire format ignores ids, but stored history shouldn't collide).
                let id = format!("ollama-{}", uuid::Uuid::new_v4());
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
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            ChatEvent::ToolCall { id, name, args_json } => {
                assert!(id.starts_with("ollama-"), "{id}");
                assert_eq!(name, "grep");
                assert_eq!(args_json, "{\"pattern\":\"foo\"}");
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn assembler_error_line() {
        let mut a = OllamaAssembler::default();
        let evs = a.push_line(r#"{"error":"model requires more system memory than is available"}"#);
        assert_eq!(
            evs,
            vec![ChatEvent::Error("model requires more system memory than is available".into())]
        );
        assert!(a.finish().is_empty(), "no Done after an error line");
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
