use crate::core::error::{Error, Result};
use crate::core::types::ToolSpec;
use serde::Deserialize;
use serde_json::json;

use super::{resolve_in_workspace, Tool, ToolContext};

const MAX_MATCHES: usize = 200;
const MAX_GLOB_RESULTS: usize = 500;
const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;

pub struct GrepTool;

#[derive(Deserialize)]
struct GrepArgs {
    pattern: String,
    path: Option<String>,
    glob: Option<String>,
}

#[async_trait::async_trait]
impl Tool for GrepTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "grep".into(),
            description: "Regex-search file contents under a workspace path. Output: relpath:line: text.".into(),
            params_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Rust regex"},
                    "path": {"type": "string", "description": "Default \".\""},
                    "glob": {"type": "string", "description": "Filename filter like \"*.rs\""}
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn execute(&self, ctx: &ToolContext, args_json: &str) -> Result<String> {
        let args: GrepArgs = serde_json::from_str(args_json)?;
        let re = regex::Regex::new(&args.pattern)?;
        let base = resolve_in_workspace(&ctx.workspace_root, args.path.as_deref().unwrap_or("."))?;
        let file_glob = args
            .glob
            .as_deref()
            .map(glob::Pattern::new)
            .transpose()
            .map_err(|e| Error::Tool(format!("bad glob: {e}")))?;
        let mut out: Vec<String> = Vec::new();
        for entry in walkdir::WalkDir::new(&base).into_iter().filter_map(|e| e.ok()) {
            if out.len() >= MAX_MATCHES {
                break;
            }
            if !entry.file_type().is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy();
            if let Some(pat) = &file_glob {
                if !pat.matches(&name) {
                    continue;
                }
            }
            if entry.metadata().map(|m| m.len() > MAX_FILE_BYTES).unwrap_or(true) {
                continue;
            }
            let bytes = match std::fs::read(entry.path()) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let text = String::from_utf8_lossy(&bytes);
            let rel = entry.path().strip_prefix(&ctx.workspace_root).unwrap_or(entry.path());
            for (i, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    out.push(format!("{}:{}: {}", rel.display(), i + 1, line.trim_end()));
                    if out.len() >= MAX_MATCHES {
                        break;
                    }
                }
            }
        }
        if out.is_empty() {
            return Ok("no matches".into());
        }
        if out.len() >= MAX_MATCHES {
            out.push(format!("…[capped at {MAX_MATCHES} matches]"));
        }
        Ok(out.join("\n"))
    }
}

pub struct GlobTool;

#[derive(Deserialize)]
struct GlobArgs {
    pattern: String,
}

#[async_trait::async_trait]
impl Tool for GlobTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "glob".into(),
            description: "Find workspace files matching a glob pattern like \"**/*.rs\".".into(),
            params_schema: json!({
                "type": "object",
                "properties": {"pattern": {"type": "string"}},
                "required": ["pattern"]
            }),
        }
    }

    async fn execute(&self, ctx: &ToolContext, args_json: &str) -> Result<String> {
        let args: GlobArgs = serde_json::from_str(args_json)?;
        let full_pattern = ctx.workspace_root.join(&args.pattern);
        let pattern_str = full_pattern.to_string_lossy().replace('\\', "/");
        let paths = glob::glob(&pattern_str).map_err(|e| Error::Tool(format!("bad glob: {e}")))?;
        let mut out: Vec<String> = Vec::new();
        for p in paths.flatten() {
            if !p.starts_with(&ctx.workspace_root) {
                continue;
            }
            let rel = p.strip_prefix(&ctx.workspace_root).unwrap_or(&p);
            out.push(rel.to_string_lossy().replace('\\', "/"));
            if out.len() >= MAX_GLOB_RESULTS {
                break;
            }
        }
        out.sort();
        if out.is_empty() {
            return Ok("no matches".into());
        }
        Ok(out.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tools::{Tool, ToolContext};

    fn ctx() -> (tempfile::TempDir, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {\n    let needle = 1;\n}\n").unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn helper() {}\n").unwrap();
        std::fs::write(root.join("notes.md"), "a needle in markdown\n").unwrap();
        (dir, ToolContext { workspace_root: root })
    }

    #[tokio::test]
    async fn grep_finds_matches_with_locations() {
        let (_d, ctx) = ctx();
        let out = GrepTool.execute(&ctx, r#"{"pattern": "needle"}"#).await.unwrap();
        assert!(out.contains("src/main.rs:2:"), "{out}");
        assert!(out.contains("notes.md:1:"), "{out}");
        assert!(!out.contains("lib.rs"), "{out}");
    }

    #[tokio::test]
    async fn grep_glob_filter() {
        let (_d, ctx) = ctx();
        let out = GrepTool.execute(&ctx, r#"{"pattern": "needle", "glob": "*.rs"}"#).await.unwrap();
        assert!(out.contains("src/main.rs:2:"), "{out}");
        assert!(!out.contains("notes.md"), "{out}");
    }

    #[tokio::test]
    async fn grep_no_matches() {
        let (_d, ctx) = ctx();
        let out = GrepTool.execute(&ctx, r#"{"pattern": "zzz"}"#).await.unwrap();
        assert!(out.contains("no matches"), "{out}");
    }

    #[tokio::test]
    async fn grep_bad_regex_is_error() {
        let (_d, ctx) = ctx();
        assert!(GrepTool.execute(&ctx, r#"{"pattern": "(["}"#).await.is_err());
    }

    #[tokio::test]
    async fn glob_finds_files() {
        let (_d, ctx) = ctx();
        let out = GlobTool.execute(&ctx, r#"{"pattern": "**/*.rs"}"#).await.unwrap();
        assert!(out.contains("src/main.rs"), "{out}");
        assert!(out.contains("src/lib.rs"), "{out}");
        assert!(!out.contains("notes.md"), "{out}");
    }

    #[tokio::test]
    async fn glob_no_matches() {
        let (_d, ctx) = ctx();
        let out = GlobTool.execute(&ctx, r#"{"pattern": "**/*.xyz"}"#).await.unwrap();
        assert!(out.contains("no matches"), "{out}");
    }
}
