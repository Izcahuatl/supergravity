# Supergravity — Design Spec

Date: 2026-07-17
Status: Approved for implementation (user delegated: "move autonomously, I'll judge post-result")

## Summary

Supergravity is a desktop **agent mission-control** app, modeled on Google Antigravity's
agent-manager surface, but **model-agnostic**: the user configures any provider — OpenAI,
Anthropic, Gemini, Ollama, or any OpenAI-compatible endpoint (Groq, Mistral, OpenRouter,
llama.cpp, vLLM, …) — and picks a model per conversation.

It is **not** an IDE. It manages workspaces (project directories) and conversations in which
an agent performs coding tasks on those workspaces through a tool-call loop. It works
alongside the user's existing editor.

## Decisions locked during brainstorming

| Question | Decision |
|---|---|
| Scope | Agent mission control (no embedded editor/terminal panes) |
| Stack | Tauri: Rust core + vanilla HTML/JS/CSS webview, no frontend framework, no build step |
| Providers | Built-in presets (OpenAI, Anthropic, Gemini, Ollama) + generic OpenAI-compatible custom endpoint |
| Agent powers | Full tool-call loop with a per-conversation approval **mode selector**: `Manual` (approve each write/exec) / `Auto` (approve nothing) — like Antigravity's Manual Approval vs Auto Approve |
| Scheduled tasks | Out of scope for v1; data model must not preclude adding them later |
| Layout | Two-pane (option A): sidebar (workspaces, conversations, settings) + single chat pane; approvals inline in the chat stream |
| Architecture | One Tauri binary; pure-Rust `core` module with zero Tauri deps + thin Tauri `bridge` |
| Crate | Rename package `kimislop` → `supergravity`, edition 2021 (Tauri/ecosystem compat) |

## Architecture

```
┌─ UI (webview): vanilla HTML/JS/CSS — no bundler ─────────────────┐
│  sidebar · chat pane (streaming, inline approvals) · settings    │
└──────────▲───────────────────────────────────────▼───────────────┘
   events (agent-event)                        commands (invoke)
┌──────────┴───────────────────────────────────────▲───────────────┐
│ Bridge (thin Tauri layer)                        │               │
│  commands · event pump · task manager (cancel)   │               │
└──────────▲───────────────────────────────────────┘               │
           │ pure Rust calls                                       │
┌──────────┴─────────────────────────────────────────────────────┐
│ Core (pure Rust, no Tauri deps — unit-testable headless)       │
│  providers/  openai · anthropic · gemini · ollama · custom     │
│  agent.rs    tool-call loop, cancellation, event stream        │
│  tools/      fs read/write/list · grep/glob · shell exec       │
│  approvals.rs broker + Manual/Auto mode                        │
│  store.rs    SQLite (rusqlite, bundled)                        │
│  config.rs   TOML config + OS keychain (keyring) for API keys  │
└──────────▲─────────────────────────────────────────────────────┘
           │ HTTPS + SSE
   model APIs (any provider)      workspace dirs on disk
```

### Repo layout

Standard Tauri v2 structure: the Rust crate moves into `src-tauri/` (its own
`Cargo.toml` / `tauri.conf.json`), the no-build frontend lives in `ui/` at the repo root.
The existing root `Cargo.toml`/`src/` are removed (replaced by `src-tauri/`), and
`.gitignore` targets `/src-tauri/target`.

```
ui/
  index.html
  app.js                   # ES modules, no framework
  style.css
src-tauri/
  Cargo.toml               # package "supergravity", lib + bin targets
  tauri.conf.json          # frontendDist = "../ui"
  build.rs
  src/
    lib.rs                 # pub mod core (pure Rust, no Tauri) + bridge wiring
    core/
      mod.rs
      types.rs             # Message, Role, ContentPart, ToolCall, ToolSpec, ChatEvent, AgentEvent
      error.rs             # thiserror Error types
      providers/
        mod.rs             # Provider trait + registry/factory from ProviderConfig
        openai.rs          # also serves OpenAiCompatible (configurable base_url/headers)
        anthropic.rs
        gemini.rs
        ollama.rs
      agent.rs             # run loop
      tools/
        mod.rs             # Tool trait, ToolContext (workspace root, approval broker handle)
        fs.rs              # read_file, write_file, list_dir (path-sandboxed to workspace)
        search.rs          # grep, glob
        shell.rs           # run_shell (needs approval in Manual mode)
      approvals.rs         # ApprovalBroker, ApprovalMode { Manual, Auto }
      store.rs             # SQLite: workspaces, conversations, messages, providers
      config.rs            # AppConfig (TOML), keychain get/set/delete for keys
    main.rs                # Tauri entry: registers commands, builds app
    bridge/
      mod.rs
      commands.rs          # #[tauri::command] fns delegating to core
      events.rs            # core AgentEvent → webview "agent-event" emit
```

## Providers

Unified message model in `core/types.rs`:

```rust
enum Role { System, User, Assistant, Tool }
struct Message { role: Role, parts: Vec<ContentPart> }
enum ContentPart { Text(String), ToolCall(ToolCall), ToolResult { id: String, content: String, is_error: bool } }
struct ToolCall { id: String, name: String, args_json: String }
struct ToolSpec { name: String, description: String, params_schema: serde_json::Value } // JSON Schema
enum ChatEvent { TextDelta(String), ToolCall(ToolCall), Usage { input: u64, output: u64 }, Done, Error(String) }
```

`Provider` trait (async_trait):

```rust
#[async_trait]
trait Provider: Send + Sync {
    async fn stream_chat(
        &self,
        model: &str,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatEvent>> + Send>>>;
}
```

Implementations (reqwest + SSE line parsing):

- **openai.rs** — `POST {base}/chat/completions`, `stream: true`, `tools` array. Default
  base `https://api.openai.com/v1`. Also backs **Custom** endpoints: config supplies
  `base_url`, key, optional extra headers, and a model name list. Covers OpenRouter, Groq,
  Mistral, Together, llama.cpp, vLLM.
- **anthropic.rs** — `POST https://api.anthropic.com/v1/messages`, headers `x-api-key`,
  `anthropic-version: 2023-06-01`, SSE events `content_block_delta` /
  `content_block_start|stop` with `tool_use` blocks; system prompt passed as top-level
  `system` field.
- **gemini.rs** — `POST {base}/v1beta/models/{model}:streamGenerateContent?alt=sse&key=…`,
  `contents` (role `user`/`model`), `functionDeclarations`, `functionCall`/`functionResponse` parts.
- **ollama.rs** — `POST http://localhost:11434/api/chat`, `stream: true`, `tools`; NDJSON streaming.

`ProviderConfig` (persisted):

```rust
struct ProviderConfig {
    id: String,                 // slug, e.g. "openai", "my-groq"
    label: String,              // display name
    kind: ProviderKind,         // OpenAi | Anthropic | Gemini | Ollama | OpenAiCompatible
    base_url: Option<String>,   // override; required for OpenAiCompatible
    has_key: bool,              // key material lives in OS keychain, never in DB/TOML
    models: Vec<String>,        // user-editable model list shown in picker
    extra_headers: Vec<(String, String)>,
}
```

Presets seed sensible defaults (base URLs, well-known model lists) but everything is
user-editable. API keys are written to the OS keychain under `supergravity:{provider_id}`
via the `keyring` crate; the store only records `has_key`.

## Agent loop

`agent::run(workspace, conversation_id, provider, model, events_tx, cancel)`:

1. Load conversation history from store; append the new user message.
2. Build system prompt: identity ("Supergravity coding agent"), workspace root path,
   tool-usage guidance, approval-mode note.
3. Loop (max 50 iterations — runaway guard):
   - `provider.stream_chat(model, messages, tool_specs)`; forward `TextDelta`s as
     `AgentEvent::TextDelta`; collect complete `ToolCall`s.
   - If no tool calls → persist assistant message, emit `Done`, exit.
   - For each tool call: emit `ToolCallProposed`; consult `ApprovalBroker`
     (Auto → immediate allow; Manual → emit `ApprovalRequested`, await UI decision,
     `Denied` → synthetic tool result "user denied this action"); execute approved calls
     with output truncated to 50 KB; emit `ToolCallFinished { ok, summary }`; append tool
     results to messages.
4. Any provider/tool error → `AgentEvent::Error` and persist; conversation stays resumable.

Cancellation: `tokio_util::sync::CancellationToken`; bridge task manager holds tokens per
conversation; UI stop button cancels between iterations and mid-shell-exec (child kill).

## Tools (v1)

All file tools resolve paths against the workspace root and **reject** escapes
(`..` outside root, absolute paths outside root) — canonicalize + prefix check.

| Tool | Params | Approval in Manual mode |
|---|---|---|
| `read_file` | path, offset?, limit? | no |
| `list_dir` | path, depth? | no |
| `write_file` | path, content, mode (`create`/`overwrite`/`append`) | **yes** |
| `grep` | pattern, path?, glob? | no |
| `glob` | pattern | no |
| `run_shell` | command, timeout_secs (default 60, max 300) | **yes** |

Shell execution: `cmd /C` on Windows / `sh -c` elsewhere, working dir = workspace root,
stdout+stderr captured and truncated.

## Approvals

`ApprovalBroker` per running agent, holds the conversation's `ApprovalMode`
(persisted on the conversation, switchable from the composer). In `Manual` mode, write/exec
tools block on a oneshot channel; the bridge emits `ApprovalRequested` (tool name, args
preview — e.g. file path + diff-less content snippet, shell command line) and the UI renders
an inline Approve/Deny card in the chat stream. `resolve_approval(request_id, allow)`
resolves the oneshot. Mode changes take effect on the next tool call.

## Storage

SQLite (rusqlite, `bundled` feature) at the platform app-data dir
(`%APPDATA%/supergravity/supergravity.db` on Windows). Migration table `schema_version`.

Tables:

- `workspaces(id, name, path UNIQUE, created_at)`
- `conversations(id, workspace_id FK, title, provider_id, model, approval_mode TEXT
  CHECK IN ('manual','auto'), created_at, updated_at)`
- `messages(id, conversation_id FK, role, parts_json, created_at)` — `parts_json` is the
  serialized `Vec<ContentPart>`; keeps tool calls/results faithfully.
- `providers(id TEXT PK, label, kind, base_url NULL, has_key INTEGER, models_json,
  extra_headers_json)`

`scheduled_tasks` is deferred but this schema does not block it (a future table FK-ing
workspaces/conversations fits unchanged).

Non-secret app config (window size, last-selected conversation) in
`%APPDATA%/supergravity/config.toml`.

## UI

Two-pane layout (approved option A), dark theme matching the Antigravity reference:

- **Sidebar**: `+ New Conversation`, conversation history grouped under collapsible
  workspace folders, `Settings` at the bottom. Active conversation marked with a dot.
- **Chat pane**: workspace selector header; message stream (user bubbles, assistant
  markdown, tool-call cards showing status spinner → summary, inline approval cards);
  composer with `@` mention-free plain input, model picker (grouped by provider), and the
  Manual/Auto approval mode toggle; stop button while streaming.
- **Settings view**: provider list (add preset/custom, edit base URL, models, set/delete
  API key → keychain), workspace management (add/remove project folders).
- Markdown: minimal hand-rolled renderer for paragraphs, `code`/``` fences, lists, bold/italic —
  no external deps in v1. Syntax highlighting deferred.
- State: single `app.js` module graph; `window.__TAURI__` invoke/listen; renders from
  store snapshots + live event stream. No build step — `ui/` is the Tauri frontend dist
  (`frontendDist = "../ui"`).

## Event protocol (bridge → UI)

`agent-event` payload:

```json
{ "conversation_id": "…", "kind": "text_delta|tool_proposed|approval_requested|
  tool_finished|message_done|error|cancelled", "...": "kind-specific fields" }
```

Commands (invoke): `list_workspaces`, `add_workspace(path)`, `remove_workspace`,
`list_conversations(workspace_id)`, `create_conversation`, `rename_conversation`,
`get_messages(conversation_id)`, `send_message(conversation_id, text)`,
`cancel_agent(conversation_id)`, `resolve_approval(request_id, allow)`,
`set_approval_mode(conversation_id, mode)`, `list_providers`, `upsert_provider`,
`set_api_key(provider_id, key)`, `delete_api_key(provider_id)`, `list_models(provider_id)`.

## Error handling

- `core::error::Error` (thiserror): `Provider`, `Http`, `Sse`, `Tool`, `Denied`, `Store`,
  `Config`, `Io`, `Cancelled`.
- Provider HTTP errors (401/403/429/5xx) surface as in-chat error cards with the provider's
  message body snippet; conversation remains usable — next send retries from history.
- Malformed SSE lines / JSON are skipped-tolerated where safe, else abort the stream with
  an error event (never panic).
- Tool execution failures (missing file, non-zero exit, timeout) are returned **to the
  model** as error tool results so it can recover, and mirrored to the UI as failed tool
  cards.
- Path-sandbox violations and approval denials are tool errors, not app errors.
- Request timeout: 120 s per provider HTTP request (configurable per provider later).

## Testing

All tests target `core` (no Tauri runtime needed):

- **Providers**: request-serialization unit tests against golden JSON fixtures; SSE
  parsers fed recorded byte streams (success, tool calls, error frames, malformed lines).
- **Agent loop**: `MockProvider` (scripted `ChatEvent` sequences) — verifies multi-turn
  tool cycles, denial path, cancellation, max-iteration guard, 50 KB truncation.
- **Tools**: tempdir-based — sandbox escape rejection, write modes, grep/glob correctness,
  shell timeout/kill.
- **Approvals**: Manual await/allow/deny, Auto passthrough, mode switch mid-run.
- **Store**: fresh-DB migrations, round-trips, foreign-key behavior.
- **Manual smoke**: run the app against a `MockProvider`-backed dev flag or a local Ollama
  instance; verify streaming, approvals, cancel in the UI (requires a human or a scripted
  webview check; at minimum `cargo tauri build` must succeed).

`cargo test` and `cargo clippy` must pass clean before v1 is called done.

## Out of scope (v1)

- Scheduled tasks / cron agent runs (deferred by user decision)
- Embedded editor, terminal pane, browser preview (mission-control only)
- Syntax highlighting, rich markdown extensions
- Conversation search, export, branching/forking
- Multi-agent parallel runs UI (backend allows it; UI renders one conversation at a time)
- MCP / external tool plugins

## Risks

- **Provider API drift** (esp. Gemini/Anthropic streaming shapes) — mitigated by fixture
  tests and isolating SSE parsing per provider module.
- **Tauri v2 on this machine** needs WebView2 (standard on Windows 10/11) and
  `cargo tauri` CLI — install during scaffolding; if Tauri setup fails irrecoverably,
  fall back to the Axum-server variant serving `ui/` (core unchanged).
- **No live API keys during development** — MockProvider + Ollama-local paths make the app
  demonstrable without paid keys.
