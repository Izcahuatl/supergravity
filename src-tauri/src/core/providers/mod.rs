pub mod anthropic;
pub mod gemini;
pub mod http;
pub mod mock;
pub mod ollama;
pub mod openai;
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

use crate::core::error::Error;
use crate::core::types::ProviderConfig;
use crate::core::types::ProviderKind;

/// First-run provider presets: sensible defaults with starter model lists.
/// All user-editable later; model names drift — treat as starting points.
pub fn presets() -> Vec<ProviderConfig> {
    vec![
        ProviderConfig {
            id: "openai".into(),
            label: "OpenAI".into(),
            kind: ProviderKind::OpenAi,
            base_url: None,
            has_key: false,
            models: vec!["gpt-5".into(), "gpt-5-mini".into()],
            extra_headers: vec![],
        },
        ProviderConfig {
            id: "anthropic".into(),
            label: "Anthropic".into(),
            kind: ProviderKind::Anthropic,
            base_url: None,
            has_key: false,
            models: vec!["claude-sonnet-4-5".into(), "claude-opus-4-5".into()],
            extra_headers: vec![],
        },
        ProviderConfig {
            id: "gemini".into(),
            label: "Gemini".into(),
            kind: ProviderKind::Gemini,
            base_url: None,
            has_key: false,
            models: vec!["gemini-2.5-pro".into(), "gemini-2.5-flash".into()],
            extra_headers: vec![],
        },
        ProviderConfig {
            id: "ollama".into(),
            label: "Ollama (local)".into(),
            kind: ProviderKind::Ollama,
            base_url: None,
            has_key: false,
            models: vec![],
            extra_headers: vec![],
        },
    ]
}

/// Build a provider backend from its persisted config. `api_key` is looked up
/// from the OS keychain by the caller; Anthropic and Gemini require one.
pub fn build_provider(cfg: &ProviderConfig, api_key: Option<String>) -> Result<Box<dyn Provider>> {
    match cfg.kind {
        ProviderKind::OpenAi => Ok(Box::new(openai::OpenAiProvider::new(
            cfg.base_url.as_deref(),
            api_key,
            cfg.extra_headers.clone(),
        ))),
        ProviderKind::OpenAiCompatible => {
            let base = cfg.base_url.as_deref().ok_or_else(|| {
                Error::Config(format!("provider '{}' requires a base_url", cfg.id))
            })?;
            Ok(Box::new(openai::OpenAiProvider::new(
                Some(base),
                api_key,
                cfg.extra_headers.clone(),
            )))
        }
        ProviderKind::Anthropic => {
            let key = api_key.ok_or_else(|| {
                Error::Config(format!("provider '{}' requires an API key", cfg.id))
            })?;
            Ok(Box::new(anthropic::AnthropicProvider::new(
                cfg.base_url.as_deref(),
                key,
            )))
        }
        ProviderKind::Gemini => {
            let key = api_key.ok_or_else(|| {
                Error::Config(format!("provider '{}' requires an API key", cfg.id))
            })?;
            Ok(Box::new(gemini::GeminiProvider::new(
                cfg.base_url.as_deref(),
                key,
            )))
        }
        ProviderKind::Ollama => Ok(Box::new(ollama::OllamaProvider::new(
            cfg.base_url.as_deref(),
        ))),
    }
}

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
        let mut compat = cfg(ProviderKind::OpenAiCompatible);
        compat.base_url = Some("https://api.groq.com/openai/v1".into());
        assert!(build_provider(&compat, Some("k".into())).is_ok());
        assert!(build_provider(&cfg(ProviderKind::Anthropic), Some("k".into())).is_ok());
        assert!(build_provider(&cfg(ProviderKind::Gemini), Some("k".into())).is_ok());
        assert!(build_provider(&cfg(ProviderKind::Ollama), None).is_ok());
    }

    #[test]
    fn presets_cover_four_kinds() {
        let p = presets();
        assert_eq!(p.len(), 4);
        for kind in [
            ProviderKind::OpenAi,
            ProviderKind::Anthropic,
            ProviderKind::Gemini,
            ProviderKind::Ollama,
        ] {
            assert!(
                p.iter().any(|c| c.kind == kind),
                "missing preset for {kind:?}"
            );
        }
        assert!(p.iter().all(|c| !c.has_key && c.base_url.is_none()));
    }

    #[test]
    fn factory_rejects_compat_without_base_url() {
        let err = build_provider(&cfg(ProviderKind::OpenAiCompatible), Some("k".into()))
            .err()
            .expect("compat without base_url must fail");
        assert!(err.to_string().contains("base_url"), "{err}");
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
