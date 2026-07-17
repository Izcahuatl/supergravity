use crate::core::error::{Error, Result};
use crate::core::types::{AgentEvent, ApprovalMode};
use std::collections::HashMap;
use std::sync::{Mutex, RwLock};
use tokio::sync::{mpsc, oneshot};

/// Gates approval-requiring tool calls on the user's decision.
/// In `Auto` mode every check passes immediately. In `Manual` mode the broker
/// emits `AgentEvent::ApprovalRequested` and blocks until `resolve` is called.
///
/// Lifetime invariant: one broker per agent run. A cancelled run may leave an
/// unresolved pending entry behind (~100 bytes); the broker is dropped with the
/// run, so it never accumulates.
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
        if self
            .events
            .send(AgentEvent::ApprovalRequested {
                request_id: request_id.clone(),
                tool_call_id: tool_call_id.to_string(),
                name: name.to_string(),
                args_json: args_json.to_string(),
            })
            .await
            .is_err()
        {
            // Receiver gone — don't leak the pending entry.
            self.pending.lock().unwrap().remove(&request_id);
            return Err(Error::ApprovalClosed);
        }
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
