use crate::core::error::{Error, Result};
use crate::core::types::ToolSpec;
use serde::Deserialize;
use serde_json::json;
use std::process::Stdio;
use std::time::Duration;

use super::{truncate_output, Tool, ToolContext};

const MAX_OUTPUT: usize = 50 * 1024;
const DEFAULT_TIMEOUT_SECS: u64 = 60;
const MAX_TIMEOUT_SECS: u64 = 300;

pub struct RunShellTool;

#[derive(Deserialize)]
struct RunShellArgs {
    command: String,
    timeout_secs: Option<u64>,
}

#[async_trait::async_trait]
impl Tool for RunShellTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "run_shell".into(),
            description: "Run a shell command in the workspace root (cmd /C on Windows, sh -c elsewhere). Captures stdout+stderr.".into(),
            params_schema: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"},
                    "timeout_secs": {"type": "integer", "description": "Default 60, max 300"}
                },
                "required": ["command"]
            }),
        }
    }

    fn needs_approval(&self) -> bool {
        true
    }

    async fn execute(&self, ctx: &ToolContext, args_json: &str) -> Result<String> {
        let args: RunShellArgs = serde_json::from_str(args_json)?;
        let timeout = Duration::from_secs(
            args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS).clamp(1, MAX_TIMEOUT_SECS),
        );
        let mut cmd = if cfg!(windows) {
            let mut c = tokio::process::Command::new("cmd");
            c.args(["/C", &args.command]);
            c
        } else {
            let mut c = tokio::process::Command::new("sh");
            c.args(["-c", &args.command]);
            c
        };
        let child = cmd
            .current_dir(&ctx.workspace_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // NOTE: kill_on_drop kills the direct child only — on timeout,
            // grandchildren (e.g. ping, build subprocesses) keep running.
            // Process-tree kill (Job Objects / killpg) is a hardening follow-up.
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| Error::Tool(format!("cannot spawn in workspace root {}: {e}", ctx.workspace_root.display())))?;

        match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(Ok(output)) => {
                let mut out = String::new();
                out.push_str(&String::from_utf8_lossy(&output.stdout));
                if !output.stderr.is_empty() {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str("[stderr]\n");
                    out.push_str(&String::from_utf8_lossy(&output.stderr));
                }
                if !output.status.success() {
                    out.push_str(&format!("\n[exit code {}]", output.status.code().unwrap_or(-1)));
                }
                if out.is_empty() {
                    out = "[no output]".to_string();
                }
                Ok(truncate_output(&out, MAX_OUTPUT))
            }
            Ok(Err(e)) => Err(e.into()),
            Err(_) => Ok(format!(
                "command timed out after {}s and was killed: {}",
                timeout.as_secs(),
                args.command
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tools::{Tool, ToolContext};

    fn ctx() -> (tempfile::TempDir, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        (dir, ToolContext { workspace_root: root })
    }

    #[tokio::test]
    async fn runs_command_and_captures_output() {
        let (_d, ctx) = ctx();
        let cmd = "echo hello-sg";
        let out = RunShellTool.execute(&ctx, &format!(r#"{{"command": "{cmd}"}}"#)).await.unwrap();
        assert!(out.contains("hello-sg"), "{out}");
    }

    #[tokio::test]
    async fn reports_nonzero_exit() {
        let (_d, ctx) = ctx();
        let cmd = "exit 3";
        let out = RunShellTool.execute(&ctx, &format!(r#"{{"command": "{cmd}"}}"#)).await.unwrap();
        assert!(out.contains("exit code"), "{out}");
    }

    #[tokio::test]
    async fn times_out_and_kills() {
        let (_d, ctx) = ctx();
        let cmd = if cfg!(windows) { "ping -n 6 127.0.0.1 >nul" } else { "sleep 5" };
        let args = serde_json::json!({"command": cmd, "timeout_secs": 1}).to_string();
        let out = RunShellTool.execute(&ctx, &args).await.unwrap();
        assert!(out.contains("timed out"), "{out}");
    }

    #[tokio::test]
    async fn needs_approval() {
        assert!(RunShellTool.needs_approval());
    }
}
