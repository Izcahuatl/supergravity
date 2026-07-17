use crate::core::error::{Error, Result};
use crate::core::types::ToolSpec;
use serde::Deserialize;
use serde_json::json;

use super::{resolve_in_workspace, truncate_output, Tool, ToolContext};

const MAX_OUTPUT: usize = 50 * 1024;
const DEFAULT_LINE_LIMIT: usize = 2000;
const MAX_LIST_ENTRIES: usize = 500;

pub struct ReadFileTool;

#[derive(Deserialize)]
struct ReadFileArgs {
    path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

#[async_trait::async_trait]
impl Tool for ReadFileTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_file".into(),
            description: "Read a UTF-8 text file in the workspace. Returns lines with optional 1-based offset and limit.".into(),
            params_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path relative to the workspace root"},
                    "offset": {"type": "integer", "description": "1-based line number to start from"},
                    "limit": {"type": "integer", "description": "Max lines to return (default 2000)"}
                },
                "required": ["path"]
            }),
        }
    }

    async fn execute(&self, ctx: &ToolContext, args_json: &str) -> Result<String> {
        let args: ReadFileArgs = serde_json::from_str(args_json)?;
        let path = resolve_in_workspace(&ctx.workspace_root, &args.path)?;
        let bytes = std::fs::read(&path)
            .map_err(|e| Error::Tool(format!("cannot read {}: {e}", path.display())))?;
        let text = String::from_utf8_lossy(&bytes);
        let offset = args.offset.unwrap_or(1).max(1);
        let limit = args.limit.unwrap_or(DEFAULT_LINE_LIMIT).max(1);
        let lines: Vec<&str> = text.lines().collect();
        let total = lines.len();
        let slice: Vec<&str> = lines.iter().skip(offset - 1).take(limit).copied().collect();
        if slice.is_empty() && offset > total {
            return Ok(format!("[offset {offset} past end of file: {total} lines]"));
        }
        let mut out = slice.join("\n");
        let shown_up_to = offset - 1 + slice.len();
        if shown_up_to < total {
            out.push_str(&format!("\n…[{} more lines]", total - shown_up_to));
        } else if !slice.is_empty() && text.ends_with('\n') {
            // The slice reached EOF; preserve the file's trailing newline.
            out.push('\n');
        }
        Ok(truncate_output(&out, MAX_OUTPUT))
    }
}

pub struct WriteFileTool;

#[derive(Deserialize)]
struct WriteFileArgs {
    path: String,
    content: String,
    mode: Option<String>,
}

#[async_trait::async_trait]
impl Tool for WriteFileTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write_file".into(),
            description: "Write text to a file in the workspace. mode: create (fail if exists), overwrite (default), append.".into(),
            params_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"},
                    "mode": {"type": "string", "enum": ["create", "overwrite", "append"]}
                },
                "required": ["path", "content"]
            }),
        }
    }

    fn needs_approval(&self) -> bool {
        true
    }

    async fn execute(&self, ctx: &ToolContext, args_json: &str) -> Result<String> {
        let args: WriteFileArgs = serde_json::from_str(args_json)?;
        let path = resolve_in_workspace(&ctx.workspace_root, &args.path)?;
        let mode = args.mode.as_deref().unwrap_or("overwrite");
        if !matches!(mode, "create" | "overwrite" | "append") {
            return Err(Error::Tool(format!("unknown write mode: {mode}")));
        }
        if mode == "create" && path.exists() {
            return Err(Error::Tool(format!(
                "file already exists: {}",
                path.display()
            )));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match mode {
            "create" | "overwrite" => std::fs::write(&path, &args.content)?,
            "append" => {
                use std::io::Write;
                let mut f = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)?;
                f.write_all(args.content.as_bytes())?;
            }
            _ => unreachable!(),
        }
        Ok(format!(
            "wrote {} bytes to {}",
            args.content.len(),
            path.display()
        ))
    }
}

pub struct ListDirTool;

#[derive(Deserialize)]
struct ListDirArgs {
    path: Option<String>,
    depth: Option<usize>,
}

#[async_trait::async_trait]
impl Tool for ListDirTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list_dir".into(),
            description: "List files and directories under a workspace path, indented by depth."
                .into(),
            params_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Default \".\""},
                    "depth": {"type": "integer", "description": "Recursion depth (default 1)"}
                }
            }),
        }
    }

    async fn execute(&self, ctx: &ToolContext, args_json: &str) -> Result<String> {
        let args: ListDirArgs = serde_json::from_str(args_json)?;
        let base = resolve_in_workspace(&ctx.workspace_root, args.path.as_deref().unwrap_or("."))?;
        if !base.is_dir() {
            return Err(Error::Tool(format!("not a directory: {}", base.display())));
        }
        let depth = args.depth.unwrap_or(1);
        let mut out = Vec::new();
        list_recursive(&base, depth, 0, &mut out);
        if out.len() >= MAX_LIST_ENTRIES {
            out.push(format!("…[capped at {MAX_LIST_ENTRIES} entries]"));
        }
        Ok(out.join("\n"))
    }
}

fn list_recursive(dir: &std::path::Path, depth: usize, level: usize, out: &mut Vec<String>) {
    if out.len() >= MAX_LIST_ENTRIES {
        return;
    }
    let mut entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
        Err(_) => return,
    };
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        if out.len() >= MAX_LIST_ENTRIES {
            return;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let name = entry.file_name().to_string_lossy().to_string();
        let indent = "  ".repeat(level);
        out.push(format!("{indent}{}{}", name, if is_dir { "/" } else { "" }));
        if is_dir && level + 1 < depth {
            list_recursive(&entry.path(), depth, level + 1, out);
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
        let ctx = ToolContext {
            workspace_root: root.clone(),
        };
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.txt"), "line1\nline2\nline3\nline4\n").unwrap();
        std::fs::write(root.join("top.txt"), "top\n").unwrap();
        (dir, ctx)
    }

    #[tokio::test]
    async fn read_file_full() {
        let (_d, ctx) = ctx();
        let out = ReadFileTool
            .execute(&ctx, r#"{"path": "src/a.txt"}"#)
            .await
            .unwrap();
        assert_eq!(out, "line1\nline2\nline3\nline4\n");
    }

    #[tokio::test]
    async fn read_file_offset_limit() {
        let (_d, ctx) = ctx();
        let out = ReadFileTool
            .execute(&ctx, r#"{"path": "src/a.txt", "offset": 2, "limit": 2}"#)
            .await
            .unwrap();
        assert!(out.starts_with("line2\nline3"), "{out}");
        assert!(out.contains("more lines"), "{out}");
    }

    #[tokio::test]
    async fn read_file_missing_is_error() {
        let (_d, ctx) = ctx();
        assert!(ReadFileTool
            .execute(&ctx, r#"{"path": "nope.txt"}"#)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn read_file_escape_rejected() {
        let (_d, ctx) = ctx();
        assert!(ReadFileTool
            .execute(&ctx, r#"{"path": "../outside.txt"}"#)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn write_file_create_and_overwrite() {
        let (_d, ctx) = ctx();
        let out = WriteFileTool
            .execute(
                &ctx,
                r#"{"path": "new/b.txt", "content": "hello", "mode": "create"}"#,
            )
            .await
            .unwrap();
        assert!(out.contains("5 bytes"), "{out}");
        assert_eq!(
            std::fs::read_to_string(ctx.workspace_root.join("new/b.txt")).unwrap(),
            "hello"
        );
        // create fails when file exists
        assert!(WriteFileTool
            .execute(
                &ctx,
                r#"{"path": "new/b.txt", "content": "x", "mode": "create"}"#
            )
            .await
            .is_err());
        // overwrite replaces
        WriteFileTool
            .execute(
                &ctx,
                r#"{"path": "new/b.txt", "content": "bye", "mode": "overwrite"}"#,
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(ctx.workspace_root.join("new/b.txt")).unwrap(),
            "bye"
        );
    }

    #[tokio::test]
    async fn write_file_append() {
        let (_d, ctx) = ctx();
        WriteFileTool
            .execute(
                &ctx,
                r#"{"path": "top.txt", "content": "more\n", "mode": "append"}"#,
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(ctx.workspace_root.join("top.txt")).unwrap(),
            "top\nmore\n"
        );
    }

    #[tokio::test]
    async fn write_file_needs_approval() {
        assert!(WriteFileTool.needs_approval());
        assert!(!ReadFileTool.needs_approval());
        assert!(!ListDirTool.needs_approval());
    }

    #[tokio::test]
    async fn list_dir_depth() {
        let (_d, ctx) = ctx();
        let out = ListDirTool
            .execute(&ctx, r#"{"path": ".", "depth": 2}"#)
            .await
            .unwrap();
        assert!(out.contains("src/"), "{out}");
        assert!(out.contains("a.txt"), "{out}");
        assert!(out.contains("top.txt"), "{out}");
        let shallow = ListDirTool
            .execute(&ctx, r#"{"path": ".", "depth": 1}"#)
            .await
            .unwrap();
        assert!(!shallow.contains("a.txt"), "{shallow}");
    }

    #[tokio::test]
    async fn read_file_offset_past_eof() {
        let (_d, ctx) = ctx();
        let out = ReadFileTool
            .execute(&ctx, r#"{"path": "src/a.txt", "offset": 99}"#)
            .await
            .unwrap();
        assert!(out.contains("past end of file"), "{out}");
    }

    #[tokio::test]
    async fn write_file_unknown_mode_has_no_side_effects() {
        let (_d, ctx) = ctx();
        assert!(WriteFileTool
            .execute(
                &ctx,
                r#"{"path": "new2/b.txt", "content": "x", "mode": "bogus"}"#
            )
            .await
            .is_err());
        assert!(
            !ctx.workspace_root.join("new2").exists(),
            "no dirs created on invalid mode"
        );
    }

    #[tokio::test]
    async fn list_dir_on_file_is_error() {
        let (_d, ctx) = ctx();
        assert!(ListDirTool
            .execute(&ctx, r#"{"path": "top.txt"}"#)
            .await
            .is_err());
    }
}
