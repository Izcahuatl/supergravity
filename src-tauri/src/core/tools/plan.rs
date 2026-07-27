use crate::core::error::{Error, Result};
use crate::core::types::ToolSpec;
use serde::Deserialize;
use serde_json::json;

use super::{Tool, ToolContext};

/// The agent-maintained task plan, rendered as a live checklist in the UI.
/// The tool itself only validates and echoes — the UI picks the steps up from
/// the tool-call events/history.
pub struct UpdatePlanTool;

#[derive(Deserialize)]
struct Step {
    text: String,
    status: String,
}

#[derive(Deserialize)]
struct Args {
    steps: Vec<Step>,
}

#[async_trait::async_trait]
impl Tool for UpdatePlanTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "update_plan".into(),
            description: "Maintain the visible task plan. For multi-step work: call FIRST with your plan (exactly one step in_progress), call again as steps complete, and mark every step done before your final answer. Always pass the FULL step list — it replaces the previous plan.".into(),
            params_schema: json!({
                "type": "object",
                "properties": {
                    "steps": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "text": {"type": "string", "description": "Short step description"},
                                "status": {"type": "string", "enum": ["pending", "in_progress", "done"]}
                            },
                            "required": ["text", "status"]
                        }
                    }
                },
                "required": ["steps"]
            }),
        }
    }

    async fn execute(&self, _ctx: &ToolContext, args_json: &str) -> Result<String> {
        let args: Args = serde_json::from_str(args_json)?;
        if args.steps.is_empty() {
            return Err(Error::Tool("steps must not be empty".into()));
        }
        for s in &args.steps {
            if s.text.trim().is_empty() {
                return Err(Error::Tool("step text must not be empty".into()));
            }
            if !matches!(s.status.as_str(), "pending" | "in_progress" | "done") {
                return Err(Error::Tool(format!(
                    "unknown step status: {} (use pending | in_progress | done)",
                    s.status
                )));
            }
        }
        Ok(format!("plan updated: {} steps", args.steps.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ToolContext {
        ToolContext {
            workspace_root: std::path::PathBuf::from(if cfg!(windows) { "C:\\w" } else { "/w" }),
            workshop_root: None,
        }
    }

    #[tokio::test]
    async fn accepts_valid_plan() {
        let out = UpdatePlanTool
            .execute(&ctx(), r#"{"steps":[{"text":"a","status":"in_progress"},{"text":"b","status":"pending"}]}"#)
            .await
            .unwrap();
        assert!(out.contains("2 steps"), "{out}");
    }

    #[tokio::test]
    async fn rejects_empty_and_bad_status() {
        assert!(UpdatePlanTool.execute(&ctx(), r#"{"steps":[]}"#).await.is_err());
        assert!(UpdatePlanTool
            .execute(&ctx(), r#"{"steps":[{"text":"a","status":"doing"}]}"#)
            .await
            .is_err());
        assert!(UpdatePlanTool
            .execute(&ctx(), r#"{"steps":[{"text":" ","status":"done"}]}"#)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn needs_no_approval() {
        assert!(!UpdatePlanTool.needs_approval());
    }
}
