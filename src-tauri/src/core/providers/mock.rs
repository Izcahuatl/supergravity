use crate::core::error::{Error, Result};
use crate::core::types::{ChatEvent, Message, ToolSpec};
use futures::Stream;
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Mutex;

use super::Provider;

/// One recorded `stream_chat` invocation: (model, messages, tools).
pub type RecordedCall = (String, Vec<Message>, Vec<ToolSpec>);

/// Scripted provider for tests and UI development without API keys.
/// Each `stream_chat` call pops one turn (a Vec of events) from the script.
pub struct MockProvider {
    pub calls: Mutex<Vec<RecordedCall>>,
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
        let err = p.stream_chat("m", &msgs, &[]).await.err().unwrap();
        assert!(err.to_string().contains("mock script exhausted"), "{err}");
    }
}
