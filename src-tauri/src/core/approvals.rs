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
    external_policy: RwLock<ExternalPolicy>,
    workshop_full_access: RwLock<bool>,
    project_files_no_ask: RwLock<bool>,
    project_shell_no_ask: RwLock<bool>,
    pending: Mutex<HashMap<String, oneshot::Sender<bool>>>,
    events: mpsc::Sender<AgentEvent>,
}

/// Tools that ALWAYS prompt the user, even in Auto mode — they cross the
/// workspace sandbox boundary, so auto-approving them would defeat the point.
const ALWAYS_ASK: [&str; 3] = ["list_external_dir", "read_external_file", "write_external_file"];

/// How external (sandbox-crossing) tools are gated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalPolicy {
    /// Prompt every time (default).
    Ask,
    /// Allow without prompting.
    Allow,
    /// Don't offer external tools at all (filtered out at the runner).
    Block,
}

impl ExternalPolicy {
    pub fn from_config(s: Option<&str>) -> Self {
        match s {
            Some("allow") => ExternalPolicy::Allow,
            Some("block") => ExternalPolicy::Block,
            _ => ExternalPolicy::Ask,
        }
    }
}

/// True when the call's `path` arg is an ABSOLUTE path inside the Workshop —
/// workshop file access stays inside the sandbox, so no prompt. Relative
/// paths anchor to the workspace (ToolContext::resolve contract), so they
/// follow normal approval rules.
fn resolves_in_workshop(workshop_root: &Option<std::path::PathBuf>, args_json: &str) -> bool {
    let Some(w) = workshop_root else { return false };
    let path = serde_json::from_str::<serde_json::Value>(args_json)
        .ok()
        .and_then(|v| v.get("path")?.as_str().map(str::to_string));
    let Some(p) = path else { return false };
    if !std::path::Path::new(&p).is_absolute() {
        return false;
    }
    crate::core::tools::resolve_in_workspace(w, &p).is_ok()
}

/// True when the command is clearly workshop-scoped: an explicit `cwd` arg
/// inside the Workshop, or a single `python <workshop>/script.py` invocation
/// (no chaining, pipes, or redirection).
fn shell_in_workshop(workshop_root: &Option<std::path::PathBuf>, args_json: &str) -> bool {
    let Some(w) = workshop_root else { return false };
    let args = serde_json::from_str::<serde_json::Value>(args_json).ok();
    // cwd arg anchored in the Workshop — anything run there is workshop-local.
    if let Some(cwd) = args
        .as_ref()
        .and_then(|v| v.get("cwd"))
        .and_then(|c| c.as_str())
    {
        if std::path::Path::new(cwd).is_absolute()
            && crate::core::tools::resolve_in_workspace(w, cwd).is_ok()
        {
            return true;
        }
    }
    let Some(cmd) = args
        .and_then(|v| v.get("command")?.as_str().map(str::to_string))
    else {
        return false;
    };
    let trimmed = cmd.trim_start();
    if !["python ", "python3 ", "py "].iter().any(|p| trimmed.starts_with(p)) {
        return false;
    }
    let shop = w.to_string_lossy().replace('\\', "/");
    let norm = cmd.replace('\\', "/");
    if !norm.contains(&shop) {
        return false;
    }
    !["&&", ";", "|", ">", "<", "`", "$("]
        .iter()
        .any(|op| norm.contains(op))
}

impl ApprovalBroker {
    pub fn new(mode: ApprovalMode, events: mpsc::Sender<AgentEvent>) -> Self {
        ApprovalBroker {
            mode: RwLock::new(mode),
            external_policy: RwLock::new(ExternalPolicy::Ask),
            workshop_full_access: RwLock::new(true),
            project_files_no_ask: RwLock::new(false),
            project_shell_no_ask: RwLock::new(false),
            pending: Mutex::new(HashMap::new()),
            events,
        }
    }

    pub fn mode(&self) -> ApprovalMode {
        *self.mode.read().unwrap()
    }

    pub fn set_mode(&self, mode: ApprovalMode) {
        *self.mode.write().unwrap() = mode;
    }

    /// Permission policy for this run (from app config).
    pub fn set_permissions(
        &self,
        external_policy: ExternalPolicy,
        workshop_full_access: bool,
        project_files_no_ask: bool,
        project_shell_no_ask: bool,
    ) {
        *self.external_policy.write().unwrap() = external_policy;
        *self.workshop_full_access.write().unwrap() = workshop_full_access;
        *self.project_files_no_ask.write().unwrap() = project_files_no_ask;
        *self.project_shell_no_ask.write().unwrap() = project_shell_no_ask;
    }

    pub fn external_policy(&self) -> ExternalPolicy {
        *self.external_policy.read().unwrap()
    }

    /// Should this tool call prompt the user in Manual mode? With Full
    /// Workshop Access, anything scoped to the Workshop (files, or a shell
    /// command with cwd inside it) runs free; external tools follow the
    /// external policy; everything else follows the per-type project policies.
    /// (Auto mode short-circuits in `check`.)
    pub fn should_prompt(
        &self,
        name: &str,
        args_json: &str,
        workshop_root: &Option<std::path::PathBuf>,
    ) -> bool {
        let in_workshop = resolves_in_workshop(workshop_root, args_json);
        if ALWAYS_ASK.contains(&name) {
            if *self.workshop_full_access.read().unwrap() && in_workshop {
                return false;
            }
            return *self.external_policy.read().unwrap() == ExternalPolicy::Ask;
        }
        if *self.workshop_full_access.read().unwrap() {
            if in_workshop {
                return false;
            }
            if name == "run_shell" && shell_in_workshop(workshop_root, args_json) {
                return false;
            }
        }
        match name {
            "run_shell" => !*self.project_shell_no_ask.read().unwrap(),
            "write_file" | "edit_file" => !*self.project_files_no_ask.read().unwrap(),
            _ => true,
        }
    }

    /// Returns true when the call may proceed. Mode is read per check, so a
    /// mid-run mode switch applies to the next tool call.
    pub async fn check(&self, tool_call_id: &str, name: &str, args_json: &str) -> Result<bool> {
        if self.mode() == ApprovalMode::Auto && !ALWAYS_ASK.contains(&name) {
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

    #[test]
    fn should_prompt_rules() {
        let (tx, _rx) = mpsc::channel(8);
        let broker = ApprovalBroker::new(ApprovalMode::Manual, tx);
        let shop = Some(std::path::PathBuf::from(if cfg!(windows) { "C:\\shop" } else { "/shop" }));
        let w = shop.as_ref().unwrap().to_string_lossy().replace('\\', "/");
        let outside = if cfg!(windows) { r#"{"path":"D:\\x"}"# } else { r#"{"path":"/etc/x"}"# };
        // Full workshop access (default): files, python, and cwd-scoped shell run free.
        assert!(!broker.should_prompt("write_external_file", &format!(r#"{{"path":"{w}/a.py"}}"#), &shop));
        assert!(!broker.should_prompt("run_shell", &format!(r#"{{"command":"python {w}/a.py"}}"#), &shop));
        assert!(!broker.should_prompt("run_shell", &format!(r#"{{"command":"pip install black", "cwd":"{w}"}}"#), &shop));
        // Chained python command does NOT count as a single invocation, but is
        // still covered if cwd is the workshop — without cwd it prompts.
        assert!(broker.should_prompt("run_shell", &format!(r#"{{"command":"python {w}/a.py && echo hi"}}"#), &shop));
        // External tools prompt under the default Ask policy…
        assert!(broker.should_prompt("write_external_file", outside, &shop));
        // …and not under Allow.
        broker.set_permissions(ExternalPolicy::Allow, true, false, false);
        assert!(!broker.should_prompt("write_external_file", outside, &shop));
        // Project per-type policies.
        broker.set_permissions(ExternalPolicy::Ask, true, false, false);
        assert!(broker.should_prompt("write_file", r#"{"path":"src/a"}"#, &shop));
        assert!(broker.should_prompt("run_shell", r#"{"command":"cargo build"}"#, &shop));
        broker.set_permissions(ExternalPolicy::Ask, true, true, true);
        assert!(!broker.should_prompt("write_file", r#"{"path":"src/a"}"#, &shop));
        assert!(!broker.should_prompt("run_shell", r#"{"command":"cargo build"}"#, &shop));
        // Full workshop access OFF: workshop paths follow project rules.
        broker.set_permissions(ExternalPolicy::Ask, false, false, false);
        assert!(broker.should_prompt("run_shell", &format!(r#"{{"command":"python {w}/a.py"}}"#), &shop));
        assert!(broker.should_prompt("write_file", &format!(r#"{{"path":"{w}/a.py"}}"#), &shop));
    }

    #[tokio::test]
    async fn auto_mode_approves_immediately_without_event() {
        let (tx, mut rx) = mpsc::channel(8);
        let broker = ApprovalBroker::new(ApprovalMode::Auto, tx);
        let ok = broker.check("call1", "write_file", "{}").await.unwrap();
        assert!(ok);
        assert!(rx.try_recv().is_err(), "no event in auto mode");
    }

    #[tokio::test]
    async fn always_ask_tools_prompt_even_in_auto_mode() {
        let (tx, mut rx) = mpsc::channel(8);
        let broker = std::sync::Arc::new(ApprovalBroker::new(ApprovalMode::Auto, tx));
        let b2 = broker.clone();
        let handle = tokio::spawn(async move {
            b2.check("call1", "list_external_dir", r#"{"path":"B:\\x"}"#).await
        });
        let ev = rx.recv().await.unwrap();
        let request_id = match ev {
            AgentEvent::ApprovalRequested { request_id, name, .. } => {
                assert_eq!(name, "list_external_dir");
                request_id
            }
            other => panic!("expected ApprovalRequested, got {other:?}"),
        };
        broker.resolve(&request_id, true).unwrap();
        assert!(handle.await.unwrap().unwrap());
    }

    #[tokio::test]
    async fn manual_mode_emits_request_and_waits_for_allow() {
        let (tx, mut rx) = mpsc::channel(8);
        let broker = std::sync::Arc::new(ApprovalBroker::new(ApprovalMode::Manual, tx));
        let b2 = broker.clone();
        let handle =
            tokio::spawn(
                async move { b2.check("call1", "run_shell", "{\"command\":\"ls\"}").await },
            );
        let ev = rx.recv().await.unwrap();
        let request_id = match ev {
            AgentEvent::ApprovalRequested {
                request_id,
                tool_call_id,
                name,
                ..
            } => {
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
