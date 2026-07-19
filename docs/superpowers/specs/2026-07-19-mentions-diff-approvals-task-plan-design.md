# @ Mentions, Diff Approvals, Task Plan — Design

Date: 2026-07-19. Three user-picked features, all touching the same composer → bridge → agent path.

## 1. @ File Mentions

**Goal:** make the composer's advertised "@ to mention" real.

**Flow:**
- Typing `@` in either composer (`#input`, `#center-input`) opens a fuzzy autocomplete popup of workspace files, anchored at the caret token. Picking one inserts the workspace-relative path as plain text (`@src/main.rs`).
- On send, the Rust side expands `@path` tokens into attachment blocks before the user message is persisted:

  ```
  <attached path="src/main.rs">
  ...file contents...
  </attached>
  ```

- Attachments are persisted as part of the user message so follow-up turns keep the context. The UI renders each `<attached>` block as a small collapsed chip (icon + path), not raw contents.

**Bridge commands:**
- `search_workspace_files(conversation_id, query)` → up to 50 workspace-relative paths, fuzzy-matched, skipping `.git`, `target`, `node_modules`; walk capped at 5000 entries. Sandbox-checked via `resolve_in_workspace`.
- Expansion happens in `prepare_run` (single implementation; UI stays thin; rewind/resend consistent). Files larger than 50 KB are truncated with a note; unreadable/binary files expand to a one-line error note inside the block instead of failing the send.

**UI details:**
- Popup reuses `.dropdown-popup` styling; keyboard: ↑/↓ to move, Enter/Tab to pick, Esc to close.
- Mention tokens are matched as `@` + non-space path characters. Tokens that don't resolve to a workspace file are left as literal text.

## 2. Diff Approvals

**Goal:** approve write/edit from a real diff instead of raw JSON args.

**Bridge command:**
- `preview_tool_diff(conversation_id, name, args_json)` → `{ path, old, new } | null`.
  - `write_file`: `old` = current file content (empty for create), `new` = `content` arg (respecting `mode`: overwrite/create → replace, append → old + content).
  - `edit_file`: applies the replacement in memory (same `expected_replacements` semantics, no disk write).
  - Returns `null` for other tools or unresolvable args; errors (e.g. old_string not found) returned as an error string shown in the card.

**UI:**
- Approval cards for `write_file`/`edit_file` fetch the preview on render and show a collapsed "Preview changes" section: mini diff using the existing diffview renderer (same add/del row styling as the Review panel).
- No diff for `run_shell` (untrackable) — unchanged behavior.

## 3. Task Plan Panel

**Goal:** AG-style visible checklist the agent maintains during multi-step work.

**Tool:**
- New built-in tool `update_plan`, no approval needed. Args: `{ "steps": [{ "text": string, "status": "pending" | "in_progress" | "done" }] }` (full replacement each call). Returns a short confirmation.
- System prompt instructs: for any multi-step task, create a plan first; keep exactly one step `in_progress`; update as work completes; mark all `done` before the final answer.

**Rendering:**
- Live: on `tool_call_finished` for `update_plan`, the UI updates (or creates) a plan card for the current run — checklist with icons per status (done ✓, in_progress ▸, pending ○). The card is inserted in the message flow at first use and mutated in place after.
- History: per run, the last `update_plan` call wins and renders as one plan card (same component), placed at the top of the run's items.

## Error handling

- Attachment read failures degrade to an in-block note, never a failed send.
- Diff preview failures show a one-line dim note in the approval card; Approve/Deny still work.
- `update_plan` with bad args returns a normal tool error the model can correct.

## Testing

- Rust: expansion (resolves, skips unknown tokens, truncates big files, binary note), `preview_tool_diff` (write create/overwrite/append, edit success + old_string-missing error), `update_plan` tool execution and validation, `search_workspace_files` (skips ignored dirs, caps results).
- UI (manual/CDP): autocomplete popup flow, chip rendering, approval card diff, plan card live updates.
- Existing suites must stay green; `cargo clippy --all-targets -- -D warnings` clean.
