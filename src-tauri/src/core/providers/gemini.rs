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
                if parts.is_empty() {
                    continue;
                }
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
                if parts.is_empty() {
                    continue;
                }
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
                if parts.is_empty() {
                    continue;
                }
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
    done_emitted: bool,
}

impl GeminiAssembler {
    pub fn push_data(&mut self, data: &str) -> Vec<ChatEvent> {
        let mut out = Vec::new();
        let v: Value = match serde_json::from_str(data.trim()) {
            Ok(v) => v,
            Err(_) => return out,
        };
        if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
            // Server-side failure frame - ends the stream; no Done after this.
            self.done_emitted = true;
            let msg = err["message"].as_str().unwrap_or("unknown gemini error");
            out.push(ChatEvent::Error(msg.to_string()));
            return out;
        }
        if let Some(candidates) = v["candidates"].as_array() {
            for cand in candidates {
                if let Some(parts) = cand["content"]["parts"].as_array() {
                    for part in parts {
                        if let Some(t) = part["text"].as_str().filter(|s| !s.is_empty()) {
                            out.push(ChatEvent::TextDelta(t.to_string()));
                        }
                        if let Some(fc) = part.get("functionCall") {
                            // Unique per call - a per-turn counter would collide
                            // across turns and corrupt id→name history mapping.
                            let id = format!("gemini-{}", uuid::Uuid::new_v4());
                            out.push(ChatEvent::ToolCall {
                                id,
                                name: fc["name"].as_str().unwrap_or("").to_string(),
                                args_json: fc
                                    .get("args")
                                    .map(|a| a.to_string())
                                    .unwrap_or_else(|| "{}".into()),
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
        let url = format!(
            "{}/v1beta/models/{}:streamGenerateContent?alt=sse",
            self.base_url, model
        );
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
        assert_eq!(
            body["systemInstruction"],
            serde_json::json!({"parts": [{"text": "be brief"}]})
        );
        assert_eq!(
            body["contents"],
            serde_json::json!([{"role": "user", "parts": [{"text": "hi"}]}])
        );
    }

    #[test]
    fn body_maps_function_call_and_response() {
        let msgs = vec![
            Message {
                role: Role::Assistant,
                parts: vec![
                    ContentPart::Text {
                        text: "reading".into(),
                    },
                    ContentPart::ToolCall {
                        id: "gemini-0".into(),
                        name: "read_file".into(),
                        args_json: "{\"path\":\"x\"}".into(),
                    },
                ],
            },
            Message {
                role: Role::Tool,
                parts: vec![ContentPart::ToolResult {
                    tool_call_id: "gemini-0".into(),
                    content: "body".into(),
                    is_error: false,
                }],
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
        assert_eq!(
            evs,
            vec![
                ChatEvent::TextDelta("Hello".into()),
                ChatEvent::Usage {
                    input_tokens: 5,
                    output_tokens: 2
                },
            ]
        );
    }

    #[test]
    fn assembler_function_call_gets_unique_synthesized_ids() {
        // Ids must be unique across calls AND across turns (a per-turn counter
        // would collide in stored history and corrupt the id→name mapping).
        let mut a = GeminiAssembler::default();
        let evs = a.push_data(r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"write_file","args":{"path":"a"}}}]}}]}"#);
        let first_id = match &evs[0] {
            ChatEvent::ToolCall {
                id,
                name,
                args_json,
            } => {
                assert!(id.starts_with("gemini-"), "{id}");
                assert_eq!(name, "write_file");
                assert_eq!(args_json, "{\"path\":\"a\"}");
                id.clone()
            }
            other => panic!("expected ToolCall, got {other:?}"),
        };
        let evs = a.push_data(r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"read_file","args":{}}}]}}]}"#);
        match &evs[0] {
            ChatEvent::ToolCall {
                id,
                name,
                args_json,
            } => {
                assert!(id.starts_with("gemini-"), "{id}");
                assert_ne!(*id, first_id, "ids must be unique");
                assert_eq!(name, "read_file");
                assert_eq!(args_json, "{}");
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn assembler_finish_emits_done_once() {
        let mut a = GeminiAssembler::default();
        assert!(a.push_data("{broken").is_empty());
        assert_eq!(a.finish(), vec![ChatEvent::Done]);
        assert!(a.finish().is_empty());
    }

    #[test]
    fn assembler_error_payload() {
        let mut a = GeminiAssembler::default();
        let evs = a.push_data(
            r#"{"error":{"code":429,"message":"quota exceeded","status":"RESOURCE_EXHAUSTED"}}"#,
        );
        assert_eq!(evs, vec![ChatEvent::Error("quota exceeded".into())]);
        assert!(a.finish().is_empty(), "no Done after an error frame");
    }

    #[test]
    fn body_skips_empty_parts() {
        let msgs = vec![
            Message {
                role: Role::User,
                parts: vec![],
            },
            Message::text(Role::User, "real"),
        ];
        let body = build_body(&msgs, &[]);
        assert_eq!(
            body["contents"],
            serde_json::json!([{"role": "user", "parts": [{"text": "real"}]}])
        );
    }
}
