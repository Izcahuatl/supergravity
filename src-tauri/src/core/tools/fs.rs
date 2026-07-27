use crate::core::error::{Error, Result};
use crate::core::types::ToolSpec;
use serde::Deserialize;
use serde_json::json;

use super::{truncate_output, Tool, ToolContext};

const MAX_FILE_READ: u64 = 10 * 1024 * 1024;
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
            description: "Read a UTF-8 text file in the workspace. Returns lines with optional 1-based offset and limit. The `path` must be a concrete file path - never a glob pattern.".into(),
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
        let path = ctx.resolve(&args.path)?;
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > MAX_FILE_READ {
                return Err(Error::Tool(format!(
                    "file too large ({} bytes, max {MAX_FILE_READ}): {} - try grep for targeted reads",
                    meta.len(),
                    path.display()
                )));
            }
        }
        let bytes = std::fs::read(&path)
            .map_err(|e| Error::Tool(format!("cannot read {}: {e}", path.display())))?;
        if bytes.len() as u64 > MAX_FILE_READ {
            return Err(Error::Tool(format!(
                "file too large ({} bytes, max {MAX_FILE_READ}): {} - try grep for targeted reads",
                bytes.len(),
                path.display()
            )));
        }
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
        let path = ctx.resolve(&args.path)?;
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
        let base = ctx.resolve(args.path.as_deref().unwrap_or("."))?;
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

/// List a directory OUTSIDE the workspace sandbox. Always gated by an explicit
/// user prompt (see `ALWAYS_ASK` in the approval broker), regardless of mode.
pub struct ListExternalDirTool;

#[derive(Deserialize)]
struct ListExternalDirArgs {
    path: String,
    depth: Option<usize>,
}

#[async_trait::async_trait]
impl Tool for ListExternalDirTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list_external_dir".into(),
            description: "List files in a directory OUTSIDE the workspace (absolute path). The user is prompted before this runs. Use ONLY when the user explicitly asks for something outside the project.".into(),
            params_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Absolute directory path"},
                    "depth": {"type": "integer", "description": "Recursion depth (default 1)"}
                },
                "required": ["path"]
            }),
        }
    }

    fn needs_approval(&self) -> bool {
        true
    }

    async fn execute(&self, _ctx: &ToolContext, args_json: &str) -> Result<String> {
        let args: ListExternalDirArgs = serde_json::from_str(args_json)?;
        let path = std::path::PathBuf::from(&args.path);
        if !path.is_absolute() {
            return Err(Error::Tool(format!(
                "external path must be absolute: {}",
                args.path
            )));
        }
        if !path.is_dir() {
            return Err(Error::Tool(format!("not a directory: {}", path.display())));
        }
        let depth = args.depth.unwrap_or(1);
        let mut out = Vec::new();
        list_recursive(&path, depth, 0, &mut out);
        if out.len() >= MAX_LIST_ENTRIES {
            out.push(format!("…[capped at {MAX_LIST_ENTRIES} entries]"));
        }
        Ok(out.join("\n"))
    }
}

/// Read a UTF-8 text file OUTSIDE the workspace (absolute path). Same output
/// shape as read_file; gated by an explicit user prompt regardless of mode.
pub struct ReadExternalFileTool;

#[derive(Deserialize)]
struct ReadExternalFileArgs {
    path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

#[async_trait::async_trait]
impl Tool for ReadExternalFileTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_external_file".into(),
            description: "Read a UTF-8 text file OUTSIDE the workspace (absolute path, optional 1-based offset and line limit). The user is prompted before this runs. Use ONLY when the user explicitly asks for a file outside the project.".into(),
            params_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Absolute file path"},
                    "offset": {"type": "integer", "description": "1-based line number to start from"},
                    "limit": {"type": "integer", "description": "Max lines to return (default 2000)"}
                },
                "required": ["path"]
            }),
        }
    }

    fn needs_approval(&self) -> bool {
        true
    }

    async fn execute(&self, _ctx: &ToolContext, args_json: &str) -> Result<String> {
        let args: ReadExternalFileArgs = serde_json::from_str(args_json)?;
        let path = std::path::PathBuf::from(&args.path);
        if !path.is_absolute() {
            return Err(Error::Tool(format!(
                "external path must be absolute: {}",
                args.path
            )));
        }
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > MAX_FILE_READ {
                return Err(Error::Tool(format!(
                    "file too large ({} bytes, max {MAX_FILE_READ}): {}",
                    meta.len(),
                    path.display()
                )));
            }
        }
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
            out.push('\n');
        }
        Ok(truncate_output(&out, MAX_OUTPUT))
    }
}

/// Write a text file OUTSIDE the workspace (absolute path). Gated by an
/// explicit user prompt regardless of mode. Not covered by Rewind checkpoints.
pub struct WriteExternalFileTool;

#[derive(Deserialize)]
struct WriteExternalFileArgs {
    path: String,
    content: String,
    mode: Option<String>,
}

#[async_trait::async_trait]
impl Tool for WriteExternalFileTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write_external_file".into(),
            description: "Write text to a file OUTSIDE the workspace (absolute path). mode: create (fail if exists), overwrite (default), append. The user is prompted before this runs. Use ONLY when the user explicitly asks to write outside the project.".into(),
            params_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Absolute file path"},
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

    async fn execute(&self, _ctx: &ToolContext, args_json: &str) -> Result<String> {
        let args: WriteExternalFileArgs = serde_json::from_str(args_json)?;
        let path = std::path::PathBuf::from(&args.path);
        if !path.is_absolute() {
            return Err(Error::Tool(format!(
                "external path must be absolute: {}",
                args.path
            )));
        }
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

pub struct EditFileTool;
#[derive(Deserialize)]
struct EditFileArgs {
    path: String,
    old_string: String,
    new_string: String,
    expected_replacements: Option<usize>,
}

#[async_trait::async_trait]
impl Tool for EditFileTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit_file".into(),
            description: "Replace an exact string in a file (surgical edit - prefer this over rewriting whole files). old_string must match exactly, including whitespace. By default it must be unique in the file; pass expected_replacements to replace N occurrences.".into(),
            params_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File to edit, relative to the workspace root"},
                    "old_string": {"type": "string", "description": "Exact text to find (must be unique unless expected_replacements is set)"},
                    "new_string": {"type": "string", "description": "Replacement text"},
                    "expected_replacements": {"type": "integer", "description": "Replace exactly N occurrences (default 1)"}
                },
                "required": ["path", "old_string", "new_string"]
            }),
        }
    }

    fn needs_approval(&self) -> bool {
        true
    }

    async fn execute(&self, ctx: &ToolContext, args_json: &str) -> Result<String> {
        let args: EditFileArgs = serde_json::from_str(args_json)?;
        if args.old_string.is_empty() {
            return Err(Error::Tool("old_string must not be empty".into()));
        }
        let path = ctx.resolve(&args.path)?;
        let bytes = std::fs::read(&path)
            .map_err(|e| Error::Tool(format!("cannot read {}: {e}", path.display())))?;
        let content = String::from_utf8_lossy(&bytes).into_owned();

        let count = content.matches(&args.old_string).count();
        let expected = args.expected_replacements.unwrap_or(1);
        if count == 0 {
            return Err(Error::Tool(format!(
                "old_string not found in {}",
                path.display()
            )));
        }
        if count != expected {
            return Err(Error::Tool(format!(
                "old_string occurs {count} times in {}, expected {expected} - make it more specific or adjust expected_replacements",
                path.display()
            )));
        }
        let updated = content.replacen(&args.old_string, &args.new_string, expected);
        std::fs::write(&path, updated)?;
        Ok(format!(
            "[edited {} - replaced {expected} occurrence{}]",
            path.display(),
            if expected == 1 { "" } else { "s" }
        ))
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
            workshop_root: None,
        };
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.txt"), "line1\nline2\nline3\nline4\n").unwrap();
        std::fs::write(root.join("top.txt"), "top\n").unwrap();
        (dir, ctx)
    }

    #[tokio::test]
    async fn edit_file_single_replacement() {
        let (_d, ctx) = ctx();
        let out = EditFileTool
            .execute(&ctx, r#"{"path": "src/a.txt", "old_string": "line2", "new_string": "CHANGED"}"#)
            .await
            .unwrap();
        assert!(out.contains("replaced 1"), "{out}");
        assert_eq!(
            std::fs::read_to_string(ctx.workspace_root.join("src/a.txt")).unwrap(),
            "line1\nCHANGED\nline3\nline4\n"
        );
    }

    #[tokio::test]
    async fn edit_file_multiline_replacement() {
        let (_d, ctx) = ctx();
        let args = r#"{"path": "src/a.txt", "old_string": "line2\nline3", "new_string": "A\nB\nC"}"#;
        EditFileTool.execute(&ctx, args).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(ctx.workspace_root.join("src/a.txt")).unwrap(),
            "line1\nA\nB\nC\nline4\n"
        );
    }

    #[tokio::test]
    async fn edit_file_not_found_is_error() {
        let (_d, ctx) = ctx();
        let err = EditFileTool
            .execute(&ctx, r#"{"path": "src/a.txt", "old_string": "nope", "new_string": "x"}"#)
            .await
            .err()
            .unwrap();
        assert!(err.to_string().contains("not found"), "{err}");
        // file unchanged
        assert_eq!(
            std::fs::read_to_string(ctx.workspace_root.join("src/a.txt")).unwrap(),
            "line1\nline2\nline3\nline4\n"
        );
    }

    #[tokio::test]
    async fn edit_file_ambiguous_requires_count() {
        let (dir, ctx) = ctx();
        std::fs::write(dir.path().join("dup.txt"), "x = 1;\nx = 2;\n").unwrap();
        let err = EditFileTool
            .execute(&ctx, r#"{"path": "dup.txt", "old_string": "x = ", "new_string": "y = "}"#)
            .await
            .err()
            .unwrap();
        assert!(err.to_string().contains("2 times"), "{err}");
        // with expected_replacements it works and replaces BOTH
        EditFileTool
            .execute(&ctx, r#"{"path": "dup.txt", "old_string": "x = ", "new_string": "y = ", "expected_replacements": 2}"#)
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("dup.txt")).unwrap(), "y = 1;\ny = 2;\n");
    }

    #[tokio::test]
    async fn edit_file_wrong_expected_count_is_error() {
        let (dir, ctx) = ctx();
        std::fs::write(dir.path().join("dup.txt"), "x\nx\nx\n").unwrap();
        let err = EditFileTool
            .execute(&ctx, r#"{"path": "dup.txt", "old_string": "x", "new_string": "y", "expected_replacements": 2}"#)
            .await
            .err()
            .unwrap();
        assert!(err.to_string().contains("expected 2"), "{err}");
    }

    #[tokio::test]
    async fn edit_file_missing_file_and_escape_rejected() {
        let (_d, ctx) = ctx();
        assert!(EditFileTool
            .execute(&ctx, r#"{"path": "nope.txt", "old_string": "a", "new_string": "b"}"#)
            .await
            .is_err());
        assert!(EditFileTool
            .execute(&ctx, r#"{"path": "../out.txt", "old_string": "a", "new_string": "b"}"#)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn edit_file_needs_approval() {
        assert!(EditFileTool.needs_approval());
    }

    #[tokio::test]
    async fn edit_file_empty_old_string_is_error() {
        let (_d, ctx) = ctx();
        assert!(EditFileTool
            .execute(&ctx, r#"{"path": "src/a.txt", "old_string": "", "new_string": "b"}"#)
            .await
            .is_err());
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
    async fn read_file_too_large_is_error() {
        let (_d, ctx) = ctx();
        let big = vec![b'x'; (MAX_FILE_READ + 1) as usize];
        std::fs::write(ctx.workspace_root.join("big.txt"), &big).unwrap();
        let err = ReadFileTool
            .execute(&ctx, r#"{"path": "big.txt"}"#)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("too large"), "{err}");
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

    #[tokio::test]
    async fn list_external_dir_rejects_relative_and_lists_absolute() {
        let (dir, ctx) = ctx();
        // Relative paths are rejected - the tool only takes absolute ones.
        assert!(ListExternalDirTool
            .execute(&ctx, r#"{"path": "src"}"#)
            .await
            .is_err());
        // Absolute path outside the workspace root is fine (the sandbox is
        // bypassed by design; the approval prompt gates it, not the path check).
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("out.txt"), "x").unwrap();
        let args = format!(r#"{{"path": "{}"}}"#, outside.path().to_string_lossy().replace('\\', "\\\\"));
        let out = ListExternalDirTool.execute(&ctx, &args).await.unwrap();
        assert!(out.contains("out.txt"), "{out}");
        assert!(ListExternalDirTool.needs_approval());
        let _ = dir;
    }

    #[tokio::test]
    async fn read_and_write_external_file() {
        let (_d, ctx) = ctx();
        let outside = tempfile::tempdir().unwrap();
        let file = outside.path().join("note.txt");
        let esc = |p: &std::path::Path| p.to_string_lossy().replace('\\', "\\\\");

        // write (create)
        let args = format!(r#"{{"path": "{}", "content": "l1\nl2\n", "mode": "create"}}"#, esc(&file));
        WriteExternalFileTool.execute(&ctx, &args).await.unwrap();
        // create again fails
        assert!(WriteExternalFileTool.execute(&ctx, &args).await.is_err());
        // read back
        let out = ReadExternalFileTool
            .execute(&ctx, &format!(r#"{{"path": "{}"}}"#, esc(&file)))
            .await
            .unwrap();
        assert_eq!(out, "l1\nl2\n");
        // relative rejected on both
        assert!(ReadExternalFileTool.execute(&ctx, r#"{"path": "x.txt"}"#).await.is_err());
        assert!(WriteExternalFileTool
            .execute(&ctx, r#"{"path": "x.txt", "content": "y"}"#)
            .await
            .is_err());
        assert!(ReadExternalFileTool.needs_approval());
        assert!(WriteExternalFileTool.needs_approval());
    }
}
