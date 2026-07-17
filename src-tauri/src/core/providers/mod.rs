pub mod openai;
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
