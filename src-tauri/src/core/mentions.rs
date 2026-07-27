//! `@path` mention expansion: user messages can reference workspace files.
//! Before the message is persisted, each resolvable mention gets its contents
//! appended as an `<attached>` block so follow-up turns keep the context.

use std::path::Path;

/// Per-file attachment cap; larger files are truncated with a note.
const MAX_ATTACH: usize = 50 * 1024;

pub fn expand(text: &str, workspace_root: &Path) -> String {
    let mut attachments: Vec<(String, String)> = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'@' {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && !bytes[end].is_ascii_whitespace() {
                end += 1;
            }
            let token = &text[start..end];
            if !token.is_empty() && !attachments.iter().any(|(p, _)| p == token) {
                if let Some(content) = read_attachment(workspace_root, token) {
                    attachments.push((token.to_string(), content));
                }
            }
            i = end;
        } else {
            i += 1;
        }
    }
    if attachments.is_empty() {
        return text.to_string();
    }
    let mut out = text.to_string();
    for (path, content) in attachments {
        out.push_str(&format!("\n\n<attached path=\"{path}\">\n{content}\n</attached>"));
    }
    out
}

/// Read a mention target. Unresolvable/non-file paths return None (the token
/// stays literal text - e.g. email addresses). Binary/oversized files attach
/// a note instead of failing the send.
fn read_attachment(root: &Path, rel: &str) -> Option<String> {
    let abs = crate::core::tools::resolve_in_workspace(root, rel).ok()?;
    let meta = std::fs::metadata(&abs).ok()?;
    if !meta.is_file() {
        return None;
    }
    let bytes = std::fs::read(&abs).ok()?;
    if bytes.contains(&0) {
        return Some("[binary file - contents not attached]".into());
    }
    if bytes.len() > MAX_ATTACH {
        let mut end = MAX_ATTACH;
        while end > 0 && std::str::from_utf8(&bytes[..end]).is_err() {
            end -= 1;
        }
        let text = String::from_utf8_lossy(&bytes[..end]);
        return Some(format!(
            "{text}\n…[truncated: {} of {} bytes shown]",
            end,
            bytes.len()
        ));
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_mentions_and_keeps_plain_text() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello file").unwrap();
        let out = expand("look at @a.txt please", dir.path());
        assert!(out.starts_with("look at @a.txt please"), "{out}");
        assert!(
            out.contains("<attached path=\"a.txt\">\nhello file\n</attached>"),
            "{out}"
        );
    }

    #[test]
    fn unknown_tokens_stay_literal_and_email_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let out = expand("mail me at a@b.com about @missing.txt", dir.path());
        assert_eq!(out, "mail me at a@b.com about @missing.txt");
    }

    #[test]
    fn duplicate_mentions_attach_once() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        let out = expand("@a.txt and @a.txt", dir.path());
        assert_eq!(out.matches("<attached").count(), 1, "{out}");
    }

    #[test]
    fn binary_file_attaches_note() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.bin"), [0u8, 159, 146, 150]).unwrap();
        let out = expand("@b.bin", dir.path());
        assert!(out.contains("[binary file - contents not attached]"), "{out}");
    }

    #[test]
    fn traversal_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let out = expand("@../outside.txt", dir.path());
        assert!(!out.contains("<attached"), "{out}");
    }
}
