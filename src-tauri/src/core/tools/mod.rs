use crate::core::error::{Error, Result};
use crate::core::types::ToolSpec;
use std::path::{Component, Path, PathBuf};

/// Shared execution context for tools.
pub struct ToolContext {
    /// MUST be an absolute path; a relative/empty root is rejected by
    /// `resolve_in_workspace` (it would otherwise match every path).
    pub workspace_root: PathBuf,
    /// Per-conversation scratch dir outside the workspace (the "Workshop").
    /// Tools may read/write here too. None disables it.
    pub workshop_root: Option<PathBuf>,
}

impl ToolContext {
    /// Resolve `p` for tool I/O: under the workspace root or, when set, under
    /// this conversation's workshop root. Relative paths anchor to the
    /// workspace; the workshop is reached via its absolute path.
    pub fn resolve(&self, p: &str) -> Result<PathBuf> {
        if let Ok(abs) = resolve_in_workspace(&self.workspace_root, p) {
            return Ok(abs);
        }
        if let Some(w) = &self.workshop_root {
            return resolve_in_workspace(w, p);
        }
        Err(Error::Tool(format!("path escapes workspace: {p}")))
    }
}

/// A capability the agent can invoke via provider tool calls.
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    /// Tools returning true require user approval in `Manual` approval mode.
    fn needs_approval(&self) -> bool {
        false
    }
    async fn execute(&self, ctx: &ToolContext, args_json: &str) -> Result<String>;
}

/// The v1 tool set given to every agent run.
pub fn default_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(fs::ReadFileTool),
        Box::new(fs::WriteFileTool),
        Box::new(fs::EditFileTool),
        Box::new(fs::ListDirTool),
        Box::new(search::GrepTool),
        Box::new(search::GlobTool),
        Box::new(shell::RunShellTool),
        Box::new(plan::UpdatePlanTool),
        Box::new(fs::ListExternalDirTool),
        Box::new(fs::ReadExternalFileTool),
        Box::new(fs::WriteExternalFileTool),
    ]
}

/// Truncate tool output to `max_bytes` (on a char boundary), noting the cut.
pub fn truncate_output(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n…[truncated {} bytes]", &s[..end], s.len() - end)
}

/// Lexically normalize a path (resolve `.` and `..` without touching the fs).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Resolve `p` inside the workspace root. Rejects paths that escape the root
/// (`..` traversal or absolute paths outside it). Note: this is a lexical
/// check; symlinks inside the workspace pointing outside are not resolved.
pub fn resolve_in_workspace(root: &Path, p: &str) -> Result<PathBuf> {
    if !root.is_absolute() {
        // A relative root would anchor the sandbox to an unpredictable CWD, and
        // an empty one would starts_with EVERY path - both must be rejected.
        return Err(Error::Tool(
            "workspace root must be an absolute path".into(),
        ));
    }
    let root_n = normalize(root);
    let candidate = Path::new(p);
    let resolved = if candidate.is_absolute() {
        normalize(candidate)
    } else {
        normalize(&root_n.join(candidate))
    };
    if resolved.starts_with(&root_n) {
        Ok(resolved)
    } else {
        Err(Error::Tool(format!("path escapes workspace: {p}")))
    }
}

pub mod fs;
pub mod plan;
pub mod search;
pub mod shell;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// Test root that is absolute on both Windows and Unix.
    fn abs_root() -> &'static Path {
        if cfg!(windows) {
            Path::new("C:\\ws")
        } else {
            Path::new("/ws")
        }
    }

    fn abs(path: &str) -> PathBuf {
        abs_root().join(path.replace('/', std::path::MAIN_SEPARATOR_STR))
    }

    #[test]
    fn ctx_resolve_accepts_workspace_and_workshop() {
        let shop = PathBuf::from(if cfg!(windows) { "C:\\workshop" } else { "/workshop" });
        let ctx = ToolContext {
            workspace_root: abs_root().to_path_buf(),
            workshop_root: Some(shop.clone()),
        };
        // Workspace-relative and workshop-absolute both resolve.
        assert_eq!(ctx.resolve("src/a.rs").unwrap(), abs("src/a.rs"));
        let w = shop.to_string_lossy().replace('\\', "/");
        assert_eq!(
            ctx.resolve(&format!("{w}/scratch.py")).unwrap(),
            shop.join("scratch.py")
        );
        // Absolute paths outside both roots are still rejected.
        assert!(ctx.resolve(if cfg!(windows) { "D:\\elsewhere\\x" } else { "/elsewhere/x" }).is_err());
        // Without a workshop root, the workshop path is rejected too.
        let ctx2 = ToolContext {
            workspace_root: abs_root().to_path_buf(),
            workshop_root: None,
        };
        assert!(ctx2.resolve(&format!("{w}/scratch.py")).is_err());
    }

    #[test]
    fn sandbox_accepts_relative_paths() {
        let root = abs_root();
        assert_eq!(
            resolve_in_workspace(root, "src/main.rs").unwrap(),
            abs("src/main.rs")
        );
        assert_eq!(resolve_in_workspace(root, ".").unwrap(), abs_root());
    }

    #[test]
    fn sandbox_normalizes_dot_segments() {
        let root = abs_root();
        assert_eq!(resolve_in_workspace(root, "a/../b").unwrap(), abs("b"));
    }

    #[test]
    fn sandbox_rejects_parent_escape() {
        let root = abs_root();
        assert!(resolve_in_workspace(root, "../outside").is_err());
        assert!(resolve_in_workspace(root, "a/../../outside").is_err());
    }

    #[test]
    fn sandbox_rejects_absolute_escape() {
        let root = if cfg!(windows) {
            Path::new("C:\\ws")
        } else {
            Path::new("/ws")
        };
        let evil = if cfg!(windows) {
            "D:\\other\\x"
        } else {
            "/etc/passwd"
        };
        assert!(resolve_in_workspace(root, evil).is_err());
    }

    #[test]
    fn sandbox_rejects_relative_or_empty_root() {
        assert!(resolve_in_workspace(Path::new("."), "anything").is_err());
        assert!(resolve_in_workspace(Path::new(""), "anything").is_err());
        assert!(resolve_in_workspace(Path::new("ws"), "anything").is_err());
    }

    #[test]
    fn truncate_output_short_strings_unchanged() {
        assert_eq!(truncate_output("hello", 100), "hello");
    }

    #[test]
    fn truncate_output_long_strings_get_note() {
        let s = "x".repeat(100);
        let out = truncate_output(&s, 10);
        assert!(out.starts_with(&"x".repeat(10)));
        assert!(out.contains("truncated"), "{out}");
    }
}
