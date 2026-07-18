use crate::core::error::{Error, Result};
use crate::core::types::ToolSpec;
use serde::Deserialize;
use serde_json::json;

use super::{resolve_in_workspace, truncate_output, Tool, ToolContext};

const MAX_MATCHES: usize = 200;
const MAX_OUTPUT: usize = 50 * 1024;
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
            description:
                "Regex-search file contents under a workspace path. Output: relpath:line: text."
                    .into(),
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
        for entry in walkdir::WalkDir::new(&base)
            .into_iter()
            .filter_entry(|e| !is_skipped_dir(e))
            .filter_map(|e| e.ok())
        {
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
            if entry
                .metadata()
                .map(|m| m.len() > MAX_FILE_BYTES)
                .unwrap_or(true)
            {
                continue;
            }
            let bytes = match std::fs::read(entry.path()) {
                Ok(b) => b,
                Err(_) => continue,
            };
            if is_binary(&bytes) {
                continue;
            }
            let text = String::from_utf8_lossy(&bytes);
            let rel = entry
                .path()
                .strip_prefix(&ctx.workspace_root)
                .unwrap_or(entry.path());
            let rel = rel.to_string_lossy().replace('\\', "/");
            for (i, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    out.push(format!("{}:{}: {}", rel, i + 1, line.trim_end()));
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
        Ok(truncate_output(&out.join("\n"), MAX_OUTPUT))
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
            description: "Find workspace files matching a glob pattern like \"**/*.rs\". Use this (not read_file) when you know a pattern, not a concrete path.".into(),
            params_schema: json!({
                "type": "object",
                "properties": {"pattern": {"type": "string"}},
                "required": ["pattern"]
            }),
        }
    }

    async fn execute(&self, ctx: &ToolContext, args_json: &str) -> Result<String> {
        let args: GlobArgs = serde_json::from_str(args_json)?;
        // glob::glob passes `..` through literally, which would escape the root —
        // reject absolute patterns and any ParentDir component up front.
        let pattern_path = std::path::Path::new(&args.pattern);
        if pattern_path.is_absolute()
            || pattern_path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(Error::Tool(format!(
                "glob pattern escapes workspace: {}",
                args.pattern
            )));
        }
        let full_pattern = ctx.workspace_root.join(&args.pattern);
        let pattern_str = full_pattern.to_string_lossy().replace('\\', "/");
        let paths = glob::glob(&pattern_str).map_err(|e| Error::Tool(format!("bad glob: {e}")))?;
        let mut out: Vec<String> = Vec::new();
        for p in paths.flatten() {
            if !p.is_file() {
                continue;
            }
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
        if out.len() >= MAX_GLOB_RESULTS {
            out.push(format!("…[capped at {MAX_GLOB_RESULTS} results]"));
        }
        Ok(out.join("\n"))
    }
}

/// Directories grep never descends into: hidden dirs (.git, .idea, …) and
/// build/dependency output that swamps results.
fn is_skipped_dir(entry: &walkdir::DirEntry) -> bool {
    // The walk root itself (depth 0) is never skipped — its name may
    // legitimately start with '.' (tempfile's `.tmpXXX`) or be "target".
    if entry.depth() == 0 || !entry.file_type().is_dir() {
        return false;
    }
    let name = entry.file_name().to_string_lossy();
    name.starts_with('.') || name == "target" || name == "node_modules"
}

/// Binary heuristic: NUL byte in the first 8 KB (same rule git/grep use).
fn is_binary(bytes: &[u8]) -> bool {
    const SNIFF: usize = 8192;
    let head = &bytes[..bytes.len().min(SNIFF)];
    head.contains(&0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tools::{Tool, ToolContext};

    fn ctx() -> (tempfile::TempDir, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/main.rs"),
            "fn main() {\n    let needle = 1;\n}\n",
        )
        .unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn helper() {}\n").unwrap();
        std::fs::write(root.join("notes.md"), "a needle in markdown\n").unwrap();
        (
            dir,
            ToolContext {
                workspace_root: root,
            },
        )
    }

    #[tokio::test]
    async fn grep_finds_matches_with_locations() {
        let (_d, ctx) = ctx();
        let out = GrepTool
            .execute(&ctx, r#"{"pattern": "needle"}"#)
            .await
            .unwrap();
        assert!(out.contains("src/main.rs:2:"), "{out}");
        assert!(out.contains("notes.md:1:"), "{out}");
        assert!(!out.contains("lib.rs"), "{out}");
    }

    #[tokio::test]
    async fn grep_glob_filter() {
        let (_d, ctx) = ctx();
        let out = GrepTool
            .execute(&ctx, r#"{"pattern": "needle", "glob": "*.rs"}"#)
            .await
            .unwrap();
        assert!(out.contains("src/main.rs:2:"), "{out}");
        assert!(!out.contains("notes.md"), "{out}");
    }

    #[tokio::test]
    async fn grep_no_matches() {
        let (_d, ctx) = ctx();
        let out = GrepTool
            .execute(&ctx, r#"{"pattern": "zzz"}"#)
            .await
            .unwrap();
        assert!(out.contains("no matches"), "{out}");
    }

    #[tokio::test]
    async fn grep_bad_regex_is_error() {
        let (_d, ctx) = ctx();
        assert!(GrepTool
            .execute(&ctx, r#"{"pattern": "(["}"#)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn glob_finds_files() {
        let (_d, ctx) = ctx();
        let out = GlobTool
            .execute(&ctx, r#"{"pattern": "**/*.rs"}"#)
            .await
            .unwrap();
        assert!(out.contains("src/main.rs"), "{out}");
        assert!(out.contains("src/lib.rs"), "{out}");
        assert!(!out.contains("notes.md"), "{out}");
    }

    #[tokio::test]
    async fn glob_no_matches() {
        let (_d, ctx) = ctx();
        let out = GlobTool
            .execute(&ctx, r#"{"pattern": "**/*.xyz"}"#)
            .await
            .unwrap();
        assert!(out.contains("no matches"), "{out}");
    }

    #[tokio::test]
    async fn grep_skips_hidden_dirs_and_target() {
        let (dir, ctx) = ctx();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/config"), "needle-in-git\n").unwrap();
        std::fs::create_dir_all(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("target/out.txt"), "needle-in-target\n").unwrap();
        let out = GrepTool
            .execute(&ctx, r#"{"pattern": "needle"}"#)
            .await
            .unwrap();
        assert!(!out.contains(".git"), "{out}");
        assert!(!out.contains("target"), "{out}");
        assert!(out.contains("src/main.rs"), "{out}");
    }

    #[tokio::test]
    async fn grep_skips_binary_files() {
        let (dir, ctx) = ctx();
        std::fs::write(dir.path().join("blob.bin"), b"\x00\x01needle-binary\x02").unwrap();
        let out = GrepTool
            .execute(&ctx, r#"{"pattern": "needle"}"#)
            .await
            .unwrap();
        assert!(!out.contains("blob.bin"), "{out}");
    }

    #[tokio::test]
    async fn glob_rejects_parent_escape() {
        let (_d, ctx) = ctx();
        assert!(GlobTool
            .execute(&ctx, r#"{"pattern": "../../*"}"#)
            .await
            .is_err());
        assert!(GlobTool
            .execute(&ctx, r#"{"pattern": "**/../*"}"#)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn glob_lists_files_not_dirs() {
        let (_d, ctx) = ctx();
        let out = GlobTool
            .execute(&ctx, r#"{"pattern": "**/*"}"#)
            .await
            .unwrap();
        assert!(out.contains("src/main.rs"), "{out}");
        assert!(
            !out.lines().any(|l| l == "src"),
            "dirs must not be listed: {out}"
        );
    }
}
