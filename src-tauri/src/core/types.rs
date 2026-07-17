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
    Text {
        text: String,
    },
    ToolCall {
        id: String,
        name: String,
        args_json: String,
    },
    ToolResult {
        tool_call_id: String,
        content: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub parts: Vec<ContentPart>,
}

impl Message {
    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Message {
            role,
            parts: vec![ContentPart::Text { text: text.into() }],
        }
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
    ToolCall {
        id: String,
        name: String,
        args_json: String,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
    /// Server-sent error frame (Anthropic `error` events, OpenAI error payloads).
    Error(String),
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    Manual,
    Auto,
}

/// One event from the agent loop toward the UI.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum AgentEvent {
    TextDelta(String),
    ToolCallProposed {
        tool_call_id: String,
        name: String,
        args_json: String,
    },
    ApprovalRequested {
        request_id: String,
        tool_call_id: String,
        name: String,
        args_json: String,
    },
    ToolCallFinished {
        tool_call_id: String,
        ok: bool,
        summary: String,
    },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_part_serde_roundtrip() {
        let parts = vec![
            ContentPart::Text {
                text: "hello".into(),
            },
            ContentPart::ToolCall {
                id: "c1".into(),
                name: "read_file".into(),
                args_json: "{}".into(),
            },
            ContentPart::ToolResult {
                tool_call_id: "c1".into(),
                content: "data".into(),
                is_error: false,
            },
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
        assert_eq!(
            serde_json::to_string(&Role::Assistant).unwrap(),
            "\"assistant\""
        );
    }

    #[test]
    fn approval_mode_snake_case() {
        assert_eq!(
            serde_json::to_string(&ApprovalMode::Manual).unwrap(),
            "\"manual\""
        );
        assert_eq!(
            serde_json::to_string(&ApprovalMode::Auto).unwrap(),
            "\"auto\""
        );
    }

    #[test]
    fn provider_kind_snake_case() {
        assert_eq!(
            serde_json::to_string(&ProviderKind::OpenAiCompatible).unwrap(),
            "\"open_ai_compatible\""
        );
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

    #[test]
    fn agent_event_serde_shape() {
        let json = serde_json::to_value(AgentEvent::TextDelta("hi".into())).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"kind": "text_delta", "data": "hi"})
        );
        let json = serde_json::to_value(AgentEvent::ToolCallFinished {
            tool_call_id: "c".into(),
            ok: true,
            summary: "s".into(),
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"kind": "tool_call_finished", "data": {"tool_call_id": "c", "ok": true, "summary": "s"}})
        );
    }
}
