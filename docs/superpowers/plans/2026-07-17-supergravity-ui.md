# Supergravity UI + Bridge — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the Tauri v2 shell, command bridge, and vanilla HTML/JS/CSS UI on top of the finished `supergravity` core, delivering the two-pane agent mission-control app from the design spec.

**Architecture:** The existing pure-Rust core (`src-tauri/src/core/`) stays untouched. A thin `src-tauri/src/bridge/` module adapts it to Tauri: `AppState` (Store + KeyStore + running-agent registry), 19 commands, and an event pump forwarding `AgentEvent`s to the webview as JSON. The UI in `ui/` is framework-free ES modules talking to the bridge via `window.__TAURI__`.

**Tech Stack:** Tauri v2 (withGlobalTauri, no plugins), Rust core (done), vanilla JS ES modules. cargo tauri CLI installed globally. No npm, no bundler.

**Spec:** `docs/superpowers/specs/2026-07-17-supergravity-design.md`
**Core plan (complete):** `docs/superpowers/plans/2026-07-17-supergravity-core.md`

**Working directory note:** Cargo commands run in `src-tauri/`; `cargo tauri` commands run in `src-tauri/` too; git commands at repo root `B:/Jetbrains/projects/kimislop`.

**Key contracts (already built, do not change):**
- `AgentEvent` serializes as `{"kind": "<snake_case>", "data": ...}` (e.g. `{"kind":"text_delta","data":"hi"}`; `{"kind":"approval_requested","data":{"request_id":...}}`)
- Bridge→UI payload: `{"conversation_id": "...", "event": <AgentEvent>}` on the Tauri event `"agent-event"`
- `AgentOutcome { produced, error }` — produced messages persist even on error
- `Store`, `KeyStore` are sync — call via `tauri::async_runtime::spawn_blocking` (helper `block()` below)
- `providers::presets()` — first-run seeding
- `MockProvider` — usable for UI dev without keys (not wired by default)

---

### Task U1: Tauri scaffold

**Files:**
- Modify: `src-tauri/Cargo.toml` (bin target + tauri deps)
- Create: `src-tauri/build.rs`, `src-tauri/tauri.conf.json`, `src-tauri/src/main.rs`, `src-tauri/src/bridge/mod.rs`, `src-tauri/icons/` (generated)
- Create: `ui/index.html`, `ui/style.css`, `ui/app.js` (placeholder shell)
- Modify: `src-tauri/src/lib.rs`

- [x] **Step 1: Install the Tauri CLI (global, one-time)**

```bash
cargo install tauri-cli --version "^2.0.0" --locked
```

Verify: `cargo tauri --version` prints 2.x. (This compiles from source; allow 5-10 min. Use a long timeout.)

- [x] **Step 2: Add Tauri deps and the bin target**

```bash
cd /b/Jetbrains/projects/kimislop/src-tauri
cargo add tauri
cargo add tauri-build --build
```

Append to `src-tauri/Cargo.toml`:

```toml
[[bin]]
name = "supergravity"
path = "src/main.rs"
```

- [x] **Step 3: `src-tauri/build.rs`**

```rust
fn main() {
    tauri_build::build()
}
```

- [x] **Step 4: `src-tauri/tauri.conf.json`**

```json
{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "supergravity",
  "version": "0.1.0",
  "identifier": "com.supergravity.app",
  "build": {
    "frontendDist": "../ui"
  },
  "app": {
    "withGlobalTauri": true,
    "windows": [
      {
        "title": "supergravity",
        "width": 1200,
        "height": 800,
        "minWidth": 800,
        "minHeight": 500
      }
    ],
    "security": {
      "csp": null
    }
  },
  "bundle": {
    "active": true,
    "targets": "all",
    "icon": [
      "icons/32x32.png",
      "icons/128x128.png",
      "icons/128x128@2x.png",
      "icons/icon.icns",
      "icons/icon.ico"
    ]
  }
}
```

- [x] **Step 5: Generate icons**

`assets/icon.png` exists at the repo root (a 256x256 PNG, committed). Run:

```bash
cd /b/Jetbrains/projects/kimislop/src-tauri
cargo tauri icon ../assets/icon.png
```

Expected: `src-tauri/icons/` populated (32x32.png, 128x128.png, icon.ico, …).

- [x] **Step 6: `src-tauri/src/main.rs`**

```rust
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    supergravity::bridge::run();
}
```

- [x] **Step 7: `src-tauri/src/lib.rs` — add bridge module**

Full content:

```rust
pub mod bridge;
pub mod core;
```

- [x] **Step 8: `src-tauri/src/bridge/mod.rs` (minimal run())**

```rust
pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running supergravity");
}
```

- [x] **Step 9: UI placeholder — `ui/index.html`**

```html
<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>supergravity</title>
    <link rel="stylesheet" href="style.css" />
  </head>
  <body>
    <div id="app">
      <aside id="sidebar">
        <button id="new-conversation">+ New Conversation</button>
        <div id="workspace-list"></div>
        <div class="sidebar-footer">
          <button id="open-settings">⚙ Settings</button>
        </div>
      </aside>
      <main id="main">
        <header id="chat-header">
          <span id="chat-title">Select or create a conversation</span>
          <button id="stop-agent" class="hidden">■ Stop</button>
        </header>
        <div id="messages"></div>
        <div id="composer" class="hidden">
          <textarea id="input" rows="3" placeholder="Ask anything…"></textarea>
          <div id="composer-bar">
            <span id="model-slot"></span>
            <button id="mode-toggle" title="Approval mode: Manual = approve every write/exec">Manual</button>
            <button id="send">Send</button>
          </div>
        </div>
      </main>
      <div id="settings" class="hidden">
        <div id="settings-panel">
          <header>
            <h2>Settings</h2>
            <button id="close-settings">✕</button>
          </header>
          <section>
            <h3>Providers</h3>
            <p class="dim">OpenAI, Anthropic, Gemini, and Ollama presets were added automatically on first run — edit them below, or add any OpenAI-compatible endpoint as custom.</p>
            <div id="provider-list"></div>
            <h4>Add custom provider (OpenAI-compatible)</h4>
            <form id="custom-provider-form">
              <input id="cp-label" placeholder="Label (e.g. Groq)" required />
              <input id="cp-base-url" placeholder="Base URL (e.g. https://api.groq.com/openai/v1)" required />
              <input id="cp-models" placeholder="Models, comma-separated (e.g. llama-3.3-70b-versatile)" required />
              <input id="cp-key" placeholder="API key (stored in OS keychain)" type="password" />
              <button type="submit">Add custom provider</button>
            </form>
          </section>
          <section>
            <h3>Add workspace</h3>
            <form id="workspace-form">
              <input id="ws-name" placeholder="Name (e.g. my-project)" required />
              <input id="ws-path" placeholder="Absolute path (e.g. C:\dev\my-project)" required />
              <button type="submit">Add workspace</button>
            </form>
          </section>
        </div>
      </div>
    </div>
    <script type="module" src="app.js"></script>
  </body>
</html>
```

- [x] **Step 10: `ui/style.css` (dark theme skeleton — extended in U4)**

```css
:root {
  --bg: #17171a;
  --bg-alt: #1e1e22;
  --bg-input: #232329;
  --border: #30303a;
  --text: #e4e4e8;
  --text-dim: #9a9aa5;
  --accent: #8b5cf6;
  --danger: #e5534b;
  --ok: #57ab5a;
  --warn: #d29922;
}

* { box-sizing: border-box; }

html, body, #app {
  height: 100%;
  margin: 0;
  background: var(--bg);
  color: var(--text);
  font: 14px/1.5 -apple-system, "Segoe UI", Roboto, sans-serif;
}

.hidden { display: none !important; }

#app { display: flex; }

#sidebar {
  width: 260px;
  min-width: 260px;
  background: var(--bg-alt);
  border-right: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  padding: 10px;
  gap: 8px;
  overflow-y: auto;
}

#main { flex: 1; display: flex; flex-direction: column; min-width: 0; }

#chat-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 10px 16px;
  border-bottom: 1px solid var(--border);
  color: var(--text-dim);
}

#messages { flex: 1; overflow-y: auto; padding: 16px; }

#composer { border-top: 1px solid var(--border); padding: 10px 16px; }

#input {
  width: 100%;
  background: var(--bg-input);
  color: var(--text);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 10px;
  resize: vertical;
  font: inherit;
}

#composer-bar { display: flex; gap: 8px; margin-top: 8px; align-items: center; }

button, select {
  background: var(--bg-input);
  color: var(--text);
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 6px 12px;
  cursor: pointer;
  font: inherit;
}

button:hover, select:hover { border-color: var(--accent); }

#new-conversation { width: 100%; }
.sidebar-footer { margin-top: auto; }

#settings {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.6);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 10;
}

#settings-panel {
  background: var(--bg-alt);
  border: 1px solid var(--border);
  border-radius: 10px;
  width: 640px;
  max-height: 80vh;
  overflow-y: auto;
  padding: 20px;
}

#settings-panel header { display: flex; justify-content: space-between; align-items: center; }
#settings-panel input { background: var(--bg-input); color: var(--text); border: 1px solid var(--border); border-radius: 6px; padding: 8px; margin: 4px 0; width: 100%; font: inherit; }
```

- [x] **Step 11: `ui/app.js` placeholder**

```js
// supergravity UI — placeholder; real logic lands in U4-U6.
document.getElementById("chat-title").textContent = "supergravity — bridge not wired yet";
```

- [x] **Step 12: Verify build + launch smoke**

```bash
cd /b/Jetbrains/projects/kimislop/src-tauri
cargo build
```

Expected: builds clean (first tauri build downloads many crates; allow several minutes).

```bash
cargo tauri dev
```

Expected: a "supergravity" window opens showing the sidebar + placeholder text. Close it after verifying (run in background and kill, or verify interactively — for an autonomous agent: launch with `run_in_background`, check the process is alive and no panic appears in output, then stop it).

Also re-run core tests to prove nothing broke: `cargo test` → all pass; `cargo clippy --all-targets -- -D warnings` → clean.

- [x] **Step 13: Commit**

```bash
cd /b/Jetbrains/projects/kimislop
git add src-tauri ui
git commit -m "feat: tauri v2 scaffold with ui placeholder"
```

---

### Task U2: Bridge state and non-agent commands

**Files:**
- Create: `src-tauri/src/bridge/state.rs`
- Create: `src-tauri/src/bridge/commands.rs`
- Modify: `src-tauri/src/bridge/mod.rs` (wire state + invoke_handler + preset seeding)

- [x] **Step 1: Write the failing tests — `src-tauri/src/bridge/state.rs` containing ONLY**

```rust
use crate::core::approvals::ApprovalBroker;
use crate::core::config::{AppConfig, KeyStore, OsKeyStore};
#[cfg(test)]
use crate::core::config::MemKeyStore;
use crate::core::store::Store;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

/// A currently-running agent in one conversation.
pub struct RunningAgent {
    pub cancel: CancellationToken,
    pub broker: Arc<ApprovalBroker>,
}

/// Shared app state (managed by Tauri; cloned Arcs into agent tasks).
pub struct AppState {
    pub store: Arc<Store>,
    pub keys: Arc<dyn KeyStore>,
    pub agents: Arc<Mutex<HashMap<String, RunningAgent>>>,
    pub config_path: PathBuf,
    pub ui_config: Arc<Mutex<AppConfig>>,
}

impl AppState {
    /// Production state: OS keychain + real DB file.
    pub fn production(store: Store, config_path: PathBuf, ui_config: AppConfig) -> Self {
        Self::with_keys(store, Arc::new(OsKeyStore), config_path, ui_config)
    }

    pub fn with_keys(store: Store, keys: Arc<dyn KeyStore>, config_path: PathBuf, ui_config: AppConfig) -> Self {
        AppState {
            store: Arc::new(store),
            keys,
            agents: Arc::new(Mutex::new(HashMap::new())),
            config_path,
            ui_config: Arc::new(Mutex::new(ui_config)),
        }
    }

    #[cfg(test)]
    pub fn test() -> Self {
        Self::with_keys(
            Store::open_in_memory().unwrap(),
            Arc::new(MemKeyStore::new()),
            PathBuf::from("test-config.toml"),
            AppConfig::default(),
        )
    }
}
```

- [x] **Step 2: `block()` helper + error mapping — `src-tauri/src/bridge/commands.rs` skeleton**

```rust
use crate::core::error::Result;

/// Run a sync core call off the async runtime (Store/KeyStore contract).
pub async fn block<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| crate::core::error::Error::Tool(format!("task join: {e}")))?
}

/// Commands return String errors to the frontend.
pub fn estr(e: crate::core::error::Error) -> String {
    e.to_string()
}
```

- [x] **Step 3: Write the failing tests — append to `src-tauri/src/bridge/commands.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::state::AppState;
    use crate::core::types::{ApprovalMode, Message, ProviderKind, Role};

    #[tokio::test]
    async fn workspace_and_conversation_crud() {
        let state = AppState::test();
        let dir = tempfile::tempdir().unwrap();
        let ws = add_workspace_impl(&state, "proj".into(), dir.path().to_string_lossy().to_string()).await.unwrap();
        assert_eq!(list_workspaces_impl(&state).await.unwrap().len(), 1);
        let cid = create_conversation_impl(&state, ws.clone(), "New Conversation".into(), "ollama".into(), "qwen3".into()).await.unwrap();
        rename_conversation_impl(&state, cid.clone(), "Fix bug".into()).await.unwrap();
        set_approval_mode_impl(&state, cid.clone(), "auto".into()).await.unwrap();
        let convs = list_conversations_impl(&state, ws.clone()).await.unwrap();
        assert_eq!(convs.len(), 1);
        assert_eq!(convs[0].title, "Fix bug");
        assert_eq!(convs[0].approval_mode, ApprovalMode::Auto);
        delete_conversation_impl(&state, cid).await.unwrap();
        assert!(list_conversations_impl(&state, ws).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn add_workspace_rejects_nonexistent_or_relative() {
        let state = AppState::test();
        assert!(add_workspace_impl(&state, "x".into(), "relative/path".into()).await.is_err());
        assert!(add_workspace_impl(&state, "x".into(), "C:\\definitely\\not\\here\\12345".into()).await.is_err());
    }

    #[tokio::test]
    async fn provider_lifecycle_with_keys() {
        let state = AppState::test();
        seed_presets_if_empty_impl(&state).await.unwrap();
        let providers = list_providers_impl(&state).await.unwrap();
        assert_eq!(providers.len(), 4);
        // second seed is a no-op
        seed_presets_if_empty_impl(&state).await.unwrap();
        assert_eq!(list_providers_impl(&state).await.unwrap().len(), 4);
        assert!(providers.iter().all(|p| !p.has_key));
        set_api_key_impl(&state, "openai".into(), "sk-test".into()).await.unwrap();
        let providers = list_providers_impl(&state).await.unwrap();
        let openai = providers.iter().find(|p| p.id == "openai").unwrap();
        assert!(openai.has_key);
        assert_eq!(state.keys.get("openai").unwrap().as_deref(), Some("sk-test"));
        delete_api_key_impl(&state, "openai".into()).await.unwrap();
        let openai = list_providers_impl(&state).await.unwrap().into_iter().find(|p| p.id == "openai").unwrap();
        assert!(!openai.has_key);
        assert_eq!(state.keys.get("openai").unwrap(), None);
        // custom provider
        let custom = crate::core::types::ProviderConfig {
            id: "groq".into(),
            label: "Groq".into(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: Some("https://api.groq.com/openai/v1".into()),
            has_key: false,
            models: vec!["llama-3.3-70b".into()],
            extra_headers: vec![],
        };
        upsert_provider_impl(&state, custom).await.unwrap();
        assert_eq!(list_providers_impl(&state).await.unwrap().len(), 5);
        delete_provider_impl(&state, "groq".into()).await.unwrap();
        assert_eq!(list_providers_impl(&state).await.unwrap().len(), 4);
    }

    #[tokio::test]
    async fn ui_state_roundtrip() {
        let state = AppState::test();
        let initial = get_initial_state_impl(&state).await.unwrap();
        assert!(initial.config.last_conversation_id.is_none());
        set_ui_state_impl(&state, Some("w1".into()), Some("c1".into())).await.unwrap();
        let after = get_initial_state_impl(&state).await.unwrap();
        assert_eq!(after.config.last_workspace_id.as_deref(), Some("w1"));
        assert_eq!(after.config.last_conversation_id.as_deref(), Some("c1"));
    }

    #[tokio::test]
    async fn messages_roundtrip() {
        let state = AppState::test();
        let dir = tempfile::tempdir().unwrap();
        let ws = add_workspace_impl(&state, "proj".into(), dir.path().to_string_lossy().to_string()).await.unwrap();
        let cid = create_conversation_impl(&state, ws, "c".into(), "ollama".into(), "m".into()).await.unwrap();
        let msg = Message::text(Role::User, "hello");
        state.store.append_message(&cid, &msg).unwrap();
        let msgs = get_messages_impl(&state, cid).await.unwrap();
        assert_eq!(msgs, vec![msg]);
    }
}
```

- [x] **Step 4: Run tests to verify they fail**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test bridge`
Expected: compile errors — `add_workspace_impl` etc. not found.

- [x] **Step 5: Implement the commands — prepend to `src-tauri/src/bridge/commands.rs`**

```rust
use crate::bridge::state::AppState;
use crate::core::config::AppConfig;
use crate::core::error::{Error, Result};
use crate::core::store::{ConversationRow, WorkspaceRow};
use crate::core::types::{Message, ProviderConfig};
use serde::Serialize;
use std::path::Path;

pub async fn list_workspaces_impl(state: &AppState) -> Result<Vec<WorkspaceRow>> {
    let s = state.store.clone();
    block(move || s.list_workspaces()).await
}

pub async fn add_workspace_impl(state: &AppState, name: String, path: String) -> Result<String> {
    let p = Path::new(&path);
    if !p.is_absolute() {
        return Err(Error::Tool("workspace path must be absolute".into()));
    }
    if !p.is_dir() {
        return Err(Error::Tool(format!("not a directory: {path}")));
    }
    let s = state.store.clone();
    block(move || s.add_workspace(&name, &path)).await
}

pub async fn remove_workspace_impl(state: &AppState, id: String) -> Result<()> {
    let s = state.store.clone();
    block(move || s.remove_workspace(&id)).await
}

pub async fn list_conversations_impl(state: &AppState, workspace_id: String) -> Result<Vec<ConversationRow>> {
    let s = state.store.clone();
    block(move || s.list_conversations(&workspace_id)).await
}

pub async fn create_conversation_impl(
    state: &AppState,
    workspace_id: String,
    title: String,
    provider_id: String,
    model: String,
) -> Result<String> {
    let s = state.store.clone();
    block(move || {
        s.create_conversation(&workspace_id, &title, &provider_id, &model, crate::core::types::ApprovalMode::Manual)
    })
    .await
}

pub async fn rename_conversation_impl(state: &AppState, id: String, title: String) -> Result<()> {
    let s = state.store.clone();
    block(move || s.rename_conversation(&id, &title)).await
}

pub async fn delete_conversation_impl(state: &AppState, id: String) -> Result<()> {
    if let Some(agent) = state.agents.lock().unwrap().get(&id) {
        agent.cancel.cancel();
    }
    let s = state.store.clone();
    block(move || s.delete_conversation(&id)).await
}

pub async fn get_messages_impl(state: &AppState, conversation_id: String) -> Result<Vec<Message>> {
    let s = state.store.clone();
    block(move || s.get_messages(&conversation_id)).await
}

pub async fn set_approval_mode_impl(state: &AppState, conversation_id: String, mode: String) -> Result<()> {
    let mode = match mode.as_str() {
        "auto" => crate::core::types::ApprovalMode::Auto,
        "manual" => crate::core::types::ApprovalMode::Manual,
        other => return Err(Error::Tool(format!("unknown approval mode: {other}"))),
    };
    if let Some(agent) = state.agents.lock().unwrap().get(&conversation_id) {
        agent.broker.set_mode(mode);
    }
    let s = state.store.clone();
    block(move || s.set_approval_mode(&conversation_id, mode)).await
}

pub async fn update_conversation_model_impl(
    state: &AppState,
    conversation_id: String,
    provider_id: String,
    model: String,
) -> Result<()> {
    let s = state.store.clone();
    block(move || s.update_conversation_model(&conversation_id, &provider_id, &model)).await
}

pub async fn list_providers_impl(state: &AppState) -> Result<Vec<ProviderConfig>> {
    let s = state.store.clone();
    block(move || s.list_providers()).await
}

pub async fn upsert_provider_impl(state: &AppState, cfg: ProviderConfig) -> Result<()> {
    let s = state.store.clone();
    block(move || s.upsert_provider(&cfg)).await
}

pub async fn delete_provider_impl(state: &AppState, id: String) -> Result<()> {
    let k = state.keys.clone();
    let kid = id.clone();
    let _ = block(move || k.delete(&kid)).await;
    let s = state.store.clone();
    block(move || s.delete_provider(&id)).await
}

pub async fn set_api_key_impl(state: &AppState, provider_id: String, key: String) -> Result<()> {
    // Validate the provider exists BEFORE writing the keychain (no orphan entries).
    let mut cfg = {
        let s = state.store.clone();
        let pid = provider_id.clone();
        block(move || s.get_provider(&pid)).await?
    };
    {
        let k = state.keys.clone();
        let pid = provider_id.clone();
        block(move || k.set(&pid, &key)).await?;
    }
    cfg.has_key = true;
    let s = state.store.clone();
    block(move || s.upsert_provider(&cfg)).await
}

pub async fn delete_api_key_impl(state: &AppState, provider_id: String) -> Result<()> {
    let mut cfg = {
        let s = state.store.clone();
        let pid = provider_id.clone();
        block(move || s.get_provider(&pid)).await?
    };
    {
        let k = state.keys.clone();
        let pid = provider_id.clone();
        block(move || k.delete(&pid)).await?;
    }
    cfg.has_key = false;
    let s = state.store.clone();
    block(move || s.upsert_provider(&cfg)).await
}

/// Insert the four provider presets on first run (providers table empty).
pub async fn seed_presets_if_empty_impl(state: &AppState) -> Result<()> {
    let existing = {
        let s = state.store.clone();
        block(move || s.list_providers()).await?
    };
    if !existing.is_empty() {
        return Ok(());
    }
    for preset in crate::core::providers::presets() {
        let s = state.store.clone();
        block(move || s.upsert_provider(&preset)).await?;
    }
    Ok(())
}

/// Everything the UI needs at boot.
#[derive(Serialize)]
pub struct InitialState {
    pub config: AppConfig,
    pub workspaces: Vec<WorkspaceRow>,
    pub providers: Vec<ProviderConfig>,
}

pub async fn get_initial_state_impl(state: &AppState) -> Result<InitialState> {
    let workspaces = list_workspaces_impl(state).await?;
    let providers = list_providers_impl(state).await?;
    let config = state.ui_config.lock().unwrap().clone();
    Ok(InitialState { config, workspaces, providers })
}

pub async fn set_ui_state_impl(
    state: &AppState,
    last_workspace_id: Option<String>,
    last_conversation_id: Option<String>,
) -> Result<()> {
    let cfg = {
        let mut guard = state.ui_config.lock().unwrap();
        guard.last_workspace_id = last_workspace_id;
        guard.last_conversation_id = last_conversation_id;
        guard.clone()
    };
    let path = state.config_path.clone();
    tauri::async_runtime::spawn_blocking(move || cfg.save(&path))
        .await
        .map_err(|e| Error::Tool(format!("task join: {e}")))?
}
```

Then the Tauri command wrappers — append to the same file (outside tests). Write each wrapper explicitly (Tauri commands have varying return types — no macro).

**Compile note (implementation-aligned):** `commands.rs` imports `core::error::Result` (a 1-parameter alias) for the `*_impl` fns, so the wrappers' return types must be written fully-qualified as `std::result::Result<T, String>` — unqualified `Result<T, String>` does not compile here.

```rust
#[tauri::command]
pub async fn list_workspaces(state: tauri::State<'_, AppState>) -> std::result::Result<Vec<WorkspaceRow>, String> {
    list_workspaces_impl(&state).await.map_err(estr)
}

#[tauri::command]
pub async fn add_workspace(state: tauri::State<'_, AppState>, name: String, path: String) -> Result<String, String> {
    add_workspace_impl(&state, name, path).await.map_err(estr)
}

#[tauri::command]
pub async fn remove_workspace(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    remove_workspace_impl(&state, id).await.map_err(estr)
}

#[tauri::command]
pub async fn list_conversations(state: tauri::State<'_, AppState>, workspace_id: String) -> Result<Vec<ConversationRow>, String> {
    list_conversations_impl(&state, workspace_id).await.map_err(estr)
}

#[tauri::command]
pub async fn create_conversation(
    state: tauri::State<'_, AppState>,
    workspace_id: String,
    title: String,
    provider_id: String,
    model: String,
) -> Result<String, String> {
    create_conversation_impl(&state, workspace_id, title, provider_id, model).await.map_err(estr)
}

#[tauri::command]
pub async fn rename_conversation(state: tauri::State<'_, AppState>, id: String, title: String) -> Result<(), String> {
    rename_conversation_impl(&state, id, title).await.map_err(estr)
}

#[tauri::command]
pub async fn delete_conversation(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    delete_conversation_impl(&state, id).await.map_err(estr)
}

#[tauri::command]
pub async fn get_messages(state: tauri::State<'_, AppState>, conversation_id: String) -> Result<Vec<Message>, String> {
    get_messages_impl(&state, conversation_id).await.map_err(estr)
}

#[tauri::command]
pub async fn set_approval_mode(state: tauri::State<'_, AppState>, conversation_id: String, mode: String) -> Result<(), String> {
    set_approval_mode_impl(&state, conversation_id, mode).await.map_err(estr)
}

#[tauri::command]
pub async fn update_conversation_model(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    provider_id: String,
    model: String,
) -> Result<(), String> {
    update_conversation_model_impl(&state, conversation_id, provider_id, model).await.map_err(estr)
}

#[tauri::command]
pub async fn list_providers(state: tauri::State<'_, AppState>) -> Result<Vec<ProviderConfig>, String> {
    list_providers_impl(&state).await.map_err(estr)
}

#[tauri::command]
pub async fn upsert_provider(state: tauri::State<'_, AppState>, cfg: ProviderConfig) -> Result<(), String> {
    upsert_provider_impl(&state, cfg).await.map_err(estr)
}

#[tauri::command]
pub async fn delete_provider(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    delete_provider_impl(&state, id).await.map_err(estr)
}

#[tauri::command]
pub async fn set_api_key(state: tauri::State<'_, AppState>, provider_id: String, key: String) -> Result<(), String> {
    set_api_key_impl(&state, provider_id, key).await.map_err(estr)
}

#[tauri::command]
pub async fn delete_api_key(state: tauri::State<'_, AppState>, provider_id: String) -> Result<(), String> {
    delete_api_key_impl(&state, provider_id).await.map_err(estr)
}

#[tauri::command]
pub async fn get_initial_state(state: tauri::State<'_, AppState>) -> Result<InitialState, String> {
    get_initial_state_impl(&state).await.map_err(estr)
}

#[tauri::command]
pub async fn set_ui_state(
    state: tauri::State<'_, AppState>,
    last_workspace_id: Option<String>,
    last_conversation_id: Option<String>,
) -> Result<(), String> {
    set_ui_state_impl(&state, last_workspace_id, last_conversation_id).await.map_err(estr)
}
```

(All 19 wrappers follow the same one-line pattern: call the `*_impl` and `.map_err(estr)`.)

- [x] **Step 6: Wire everything in `src-tauri/src/bridge/mod.rs`**

Full content:

```rust
pub mod agent_runner;
pub mod commands;
pub mod state;

use crate::core::config::{data_dir, AppConfig};
use crate::core::store::Store;
use tauri::Manager;

pub fn run() {
    let dir = data_dir().expect("cannot determine app data dir");
    std::fs::create_dir_all(&dir).expect("cannot create app data dir");
    let store = Store::open(&dir.join("supergravity.db")).expect("cannot open store");
    let config_path = dir.join("config.toml");
    let ui_config = AppConfig::load(&config_path).unwrap_or_default();
    let state = state::AppState::production(store, config_path, ui_config);

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::list_workspaces,
            commands::add_workspace,
            commands::remove_workspace,
            commands::list_conversations,
            commands::create_conversation,
            commands::rename_conversation,
            commands::delete_conversation,
            commands::get_messages,
            commands::set_approval_mode,
            commands::update_conversation_model,
            commands::list_providers,
            commands::upsert_provider,
            commands::delete_provider,
            commands::set_api_key,
            commands::delete_api_key,
            commands::get_initial_state,
            commands::set_ui_state,
            agent_runner::send_message,
            agent_runner::cancel_agent,
            agent_runner::resolve_approval,
        ])
        .setup(|app| {
            let state = app.state::<state::AppState>();
            let handle = tauri::async_runtime::handle();
            let inner = state.inner();
            // Preset seeding needs an async context; block briefly at startup.
            handle.block_on(async {
                commands::seed_presets_if_empty_impl(inner).await.expect("seed presets");
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running supergravity");
}
```

(`agent_runner` is Task U3 — create `src-tauri/src/bridge/agent_runner.rs` with placeholder `send_message`/`cancel_agent`/`resolve_approval` commands returning `Err("not implemented".into())` for now, so this task compiles.)

- [x] **Step 7: Run tests to verify they pass**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test bridge`
Expected: `test result: ok. 5 passed` (bridge tests); full suite all green; `cargo clippy --all-targets -- -D warnings` clean.

- [x] **Step 8: Commit**

```bash
cd /b/Jetbrains/projects/kimislop
git add src-tauri
git commit -m "feat(bridge): app state and non-agent commands"
```

---

### Task U3: Agent runner (send_message, cancel, resolve_approval, event pump)

**Files:**
- Modify: `src-tauri/src/bridge/agent_runner.rs` (replace placeholder)

Design notes (from quality review — these fix real bugs in the original sketch):

1. **The agents map holds a broker clone, and the broker owns an events sender.** The runner task must drop the map entry BEFORE awaiting the pump, or the channel never closes and the task deadlocks (proven 100% reproducible). A drop guard (`AgentGuard`) also covers the panic path.
2. **Guard + insert must be atomic** (single lock scope, before any await) — otherwise two rapid sends both pass the guard and spawn two agents.
3. **Provider is built BEFORE the user message is appended**, so a config/key failure doesn't leave a dangling user message in history.
4. **Store calls in the persist loop go through `block()`** (spawn_blocking contract), with `eprintln!` on failure.

- [x] **Step 1: Write the failing tests — `src-tauri/src/bridge/agent_runner.rs` containing ONLY**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::state::AppState;
    use crate::core::providers::mock::MockProvider;
    use crate::core::types::{AgentEvent, ChatEvent, Message, Role};
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn pump_forwards_events_as_envelopes() {
        let (tx, rx) = mpsc::channel(8);
        let (out_tx, mut out_rx) = mpsc::channel(8);
        let pump = tokio::spawn(pump_events("conv-1".into(), rx, move |v| {
            let _ = out_tx.try_send(v);
        }));
        tx.send(AgentEvent::TextDelta("hi".into())).await.unwrap();
        tx.send(AgentEvent::MessageDone).await.unwrap();
        drop(tx);
        pump.await.unwrap();
        let first = out_rx.recv().await.unwrap();
        assert_eq!(
            first,
            serde_json::json!({"conversation_id": "conv-1", "event": {"kind": "text_delta", "data": "hi"}})
        );
        let second = out_rx.recv().await.unwrap();
        assert_eq!(
            second,
            serde_json::json!({"conversation_id": "conv-1", "event": {"kind": "message_done"}})
        );
        assert!(out_rx.try_recv().is_err(), "channel closes after senders drop");
    }

    /// Full lifecycle: scripted run persists produced messages, emits events,
    /// and ALWAYS frees the agents-map entry (the deadlock regression test).
    #[tokio::test]
    async fn agent_task_lifecycle_completes_and_cleans_up() {
        let state = AppState::test();
        let dir = tempfile::tempdir().unwrap();
        let ws = state.store.add_workspace("proj", &dir.path().to_string_lossy()).unwrap();
        let cid = state
            .store
            .create_conversation(&ws, "c", "mock", "m", crate::core::types::ApprovalMode::Auto)
            .unwrap();
        state.store.append_message(&cid, &Message::text(Role::User, "go")).unwrap();

        let provider = std::sync::Arc::new(MockProvider::new(vec![vec![
            Ok(ChatEvent::TextDelta("hello".into())),
            Ok(ChatEvent::Done),
        ]]));
        let conv = state.store.get_conversation(&cid).unwrap();
        let parts = AgentTaskParts::new(&state, conv, provider).await.unwrap();
        let (out_tx, mut out_rx) = mpsc::channel::<serde_json::Value>(16);
        let events_seen = tokio::spawn(async move {
            let mut kinds = vec![];
            while let Some(v) = out_rx.recv().await {
                kinds.push(v["event"]["kind"].as_str().unwrap().to_string());
            }
            kinds
        });

        // Must COMPLETE (the original pump-order bug deadlocked here forever).
        tokio::time::timeout(std::time::Duration::from_secs(10), run_agent_task(parts, move |v| {
            let _ = out_tx.try_send(v);
        }))
        .await
        .expect("agent task must not deadlock");

        // Map entry freed.
        assert!(state.agents.lock().unwrap().is_empty(), "agents map must be empty after run");
        // Produced assistant message persisted.
        let msgs = state.store.get_messages(&cid).unwrap();
        assert_eq!(msgs.len(), 2, "user + assistant");
        assert_eq!(msgs[1].role, Role::Assistant);
        // Events flowed through the emit seam.
        let kinds = events_seen.await.unwrap();
        assert!(kinds.contains(&"text_delta".to_string()), "{kinds:?}");
        assert!(kinds.contains(&"message_done".to_string()), "{kinds:?}");
    }

    #[tokio::test]
    async fn reservation_guard_rejects_duplicate() {
        let state = AppState::test();
        let dir = tempfile::tempdir().unwrap();
        let ws = state.store.add_workspace("proj", &dir.path().to_string_lossy()).unwrap();
        let cid = state
            .store
            .create_conversation(&ws, "c", "mock", "m", crate::core::types::ApprovalMode::Auto)
            .unwrap();
        let conv = state.store.get_conversation(&cid).unwrap();
        let provider = std::sync::Arc::new(MockProvider::new(vec![]));
        let _parts = AgentTaskParts::new(&state, conv.clone(), provider).await.unwrap();
        let provider2 = std::sync::Arc::new(MockProvider::new(vec![]));
        let err = AgentTaskParts::new(&state, conv, provider2).await.err().unwrap();
        assert!(err.to_string().contains("already running"), "{err}");
    }
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test agent_runner`
Expected: compile errors — `pump_events`, `AgentTaskParts`, `run_agent_task`, `send_message_impl` not found.

- [x] **Step 3: Implement the agent runner (prepend above the test module)**

```rust
use crate::bridge::commands::{block, estr};
use crate::bridge::state::{AppState, RunningAgent};
use crate::core::agent::{self, AgentRequest, DEFAULT_MAX_ITERATIONS};
use crate::core::approvals::ApprovalBroker;
use crate::core::error::{Error, Result};
use crate::core::providers::{build_provider, Provider};
use crate::core::store::{ConversationRow, Store};
use crate::core::tools::default_tools;
use crate::core::types::{AgentEvent, ApprovalMode, Message, Role};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Forward agent events to the webview as `{"conversation_id", "event"}` JSON.
pub async fn pump_events(
    conversation_id: String,
    mut rx: mpsc::Receiver<AgentEvent>,
    emit: impl Fn(serde_json::Value) + Send,
) {
    while let Some(ev) = rx.recv().await {
        emit(serde_json::json!({ "conversation_id": conversation_id, "event": ev }));
    }
}

/// Removes the agents-map entry on drop — including panic paths. Dropping the
/// entry also drops the last broker `Sender` clone, which lets the pump end.
struct AgentGuard {
    agents: Arc<Mutex<HashMap<String, RunningAgent>>>,
    cid: String,
}

impl Drop for AgentGuard {
    fn drop(&mut self) {
        if let Ok(mut g) = self.agents.lock() {
            g.remove(&self.cid);
        }
    }
}

/// Everything the spawned agent task needs (assembled after the atomic reservation).
pub struct AgentTaskParts {
    pub agents: Arc<Mutex<HashMap<String, RunningAgent>>>,
    pub store: Arc<Store>,
    pub workspace_root: PathBuf,
    pub model: String,
    pub history: Vec<Message>,
    pub provider: Arc<dyn Provider>,
    pub broker: Arc<ApprovalBroker>,
    pub cancel: CancellationToken,
    pub events_tx: mpsc::Sender<AgentEvent>,
    pub events_rx: mpsc::Receiver<AgentEvent>,
    pub conversation_id: String,
}

impl AgentTaskParts {
    /// Reserve the agents-map slot (atomic guard+insert), then load everything
    /// the run needs. On any failure the reservation is released.
    pub async fn new(state: &AppState, conv: ConversationRow, provider: Arc<dyn Provider>) -> Result<Self> {
        let cid = conv.id.clone();
        let (events_tx, events_rx) = mpsc::channel::<AgentEvent>(256);
        let cancel = CancellationToken::new();
        let broker = Arc::new(ApprovalBroker::new(ApprovalMode::Manual, events_tx.clone()));
        {
            let mut agents = state.agents.lock().unwrap();
            if agents.contains_key(&cid) {
                return Err(Error::Tool("an agent is already running in this conversation".into()));
            }
            agents.insert(cid.clone(), RunningAgent { cancel: cancel.clone(), broker: broker.clone() });
        }
        let result = Self::load(state, &conv, provider, broker, cancel, events_tx, events_rx).await;
        if result.is_err() {
            state.agents.lock().unwrap().remove(&cid);
        }
        result
    }

    async fn load(
        state: &AppState,
        conv: &ConversationRow,
        provider: Arc<dyn Provider>,
        broker: Arc<ApprovalBroker>,
        cancel: CancellationToken,
        events_tx: mpsc::Sender<AgentEvent>,
        events_rx: mpsc::Receiver<AgentEvent>,
    ) -> Result<Self> {
        broker.set_mode(conv.approval_mode);
        let ws = {
            let s = state.store.clone();
            let w = conv.workspace_id.clone();
            block(move || s.get_workspace(&w)).await?
        };
        let history = {
            let s = state.store.clone();
            let c = conv.id.clone();
            block(move || s.get_messages(&c)).await?
        };
        Ok(AgentTaskParts {
            agents: state.agents.clone(),
            store: state.store.clone(),
            workspace_root: PathBuf::from(&ws.path),
            model: conv.model.clone(),
            history,
            provider,
            broker,
            cancel,
            events_tx,
            events_rx,
            conversation_id: conv.id.clone(),
        })
    }
}

/// The spawned task body: pump events, run the loop, persist produced messages
/// (even on failure — partial runs stay resumable), free the agents-map entry.
pub async fn run_agent_task(parts: AgentTaskParts, emit: impl Fn(serde_json::Value) + Send + 'static) {
    let guard = AgentGuard { agents: parts.agents.clone(), cid: parts.conversation_id.clone() };
    let pump = tokio::spawn(pump_events(parts.conversation_id.clone(), parts.events_rx, emit));
    let outcome = agent::run(AgentRequest {
        workspace_root: parts.workspace_root,
        provider: parts.provider,
        model: parts.model,
        history: parts.history,
        tools: default_tools(),
        approvals: parts.broker,
        events: parts.events_tx,
        cancel: parts.cancel,
        max_iterations: DEFAULT_MAX_ITERATIONS,
    })
    .await;
    {
        let store = parts.store.clone();
        let cid = parts.conversation_id.clone();
        let produced = outcome.produced;
        let _ = block(move || {
            for msg in &produced {
                if let Err(e) = store.append_message(&cid, msg) {
                    eprintln!("supergravity: failed to persist produced message: {e}");
                }
            }
            Ok(())
        })
        .await;
    }
    // Drop the map entry (and with it the last broker Sender clone) BEFORE
    // awaiting the pump — otherwise the channel never closes and this task
    // deadlocks. (A new run may start while the old pump drains its last
    // buffered events; that window is acceptable.)
    drop(guard);
    let _ = pump.await;
}

pub async fn send_message_impl(
    app: AppHandle,
    state: &AppState,
    conversation_id: String,
    text: String,
) -> Result<()> {
    // Resolve provider/key FIRST — a config failure must not leave a dangling
    // user message in history.
    let conv = {
        let s = state.store.clone();
        let c = conversation_id.clone();
        block(move || s.get_conversation(&c)).await?
    };
    let pcfg = {
        let s = state.store.clone();
        let pid = conv.provider_id.clone();
        block(move || s.get_provider(&pid)).await?
    };
    let api_key = {
        let k = state.keys.clone();
        let pid = conv.provider_id.clone();
        block(move || k.get(&pid)).await?
    };
    let provider: Arc<dyn Provider> = Arc::from(build_provider(&pcfg, api_key)?);

    let user_msg = Message::text(Role::User, &text);
    {
        let s = state.store.clone();
        let c = conversation_id.clone();
        block(move || s.append_message(&c, &user_msg)).await?;
    }
    // Auto-title placeholder conversations from the first message.
    if conv.title == "New Conversation" || conv.title.trim().is_empty() {
        let title: String = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(40).collect();
        let s = state.store.clone();
        let c = conversation_id.clone();
        block(move || s.rename_conversation(&c, &title)).await?;
    }

    let parts = AgentTaskParts::new(state, conv, provider).await?;
    tauri::async_runtime::spawn(async move {
        let emit = move |v: serde_json::Value| {
            let _ = app.emit("agent-event", v);
        };
        run_agent_task(parts, emit).await;
    });
    Ok(())
}

pub async fn cancel_agent_impl(state: &AppState, conversation_id: String) -> Result<()> {
    match state.agents.lock().unwrap().get(&conversation_id) {
        Some(agent) => {
            agent.cancel.cancel();
            Ok(())
        }
        None => Err(Error::Tool("no agent running in this conversation".into())),
    }
}

pub async fn resolve_approval_impl(state: &AppState, conversation_id: String, request_id: String, allow: bool) -> Result<()> {
    match state.agents.lock().unwrap().get(&conversation_id) {
        Some(agent) => agent.broker.resolve(&request_id, allow),
        None => Err(Error::Tool("no agent running in this conversation".into())),
    }
}

// Wrappers use fully-qualified Result: `core::error::Result` (1-param alias) is
// imported in this file, so unqualified `Result<(), String>` would not compile.
#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    text: String,
) -> std::result::Result<(), String> {
    send_message_impl(app, &state, conversation_id, text).await.map_err(estr)
}

#[tauri::command]
pub async fn cancel_agent(state: tauri::State<'_, AppState>, conversation_id: String) -> std::result::Result<(), String> {
    cancel_agent_impl(&state, conversation_id).await.map_err(estr)
}

#[tauri::command]
pub async fn resolve_approval(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    request_id: String,
    allow: bool,
) -> std::result::Result<(), String> {
    resolve_approval_impl(&state, conversation_id, request_id, allow).await.map_err(estr)
}
```

- [x] **Step 4: Run tests to verify they pass**

Run: `cd /b/Jetbrains/projects/kimislop/src-tauri && cargo test bridge`
Expected: all bridge tests pass (5 from U2 + 3 new). Full suite green; clippy clean.

- [x] **Step 5: Commit**

```bash
cd /b/Jetbrains/projects/kimislop
git add src-tauri
git commit -m "feat(bridge): agent runner with event pump and cancellation"
```

### Task U4: UI shell — sidebar, workspaces, conversations

**Files:**
- Create: `ui/api.js`
- Modify: `ui/app.js` (replace placeholder)
- Modify: `ui/style.css` (extend for sidebar lists)

- [x] **Step 1: `ui/api.js`**

```js
// Thin wrappers over the Tauri bridge.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

export const api = {
  getInitialState: () => invoke("get_initial_state"),
  setUiState: (lastWorkspaceId, lastConversationId) =>
    invoke("set_ui_state", { lastWorkspaceId, lastConversationId }),
  listWorkspaces: () => invoke("list_workspaces"),
  addWorkspace: (name, path) => invoke("add_workspace", { name, path }),
  removeWorkspace: (id) => invoke("remove_workspace", { id }),
  listConversations: (workspaceId) => invoke("list_conversations", { workspaceId }),
  createConversation: (workspaceId, title, providerId, model) =>
    invoke("create_conversation", { workspaceId, title, providerId, model }),
  renameConversation: (id, title) => invoke("rename_conversation", { id, title }),
  deleteConversation: (id) => invoke("delete_conversation", { id }),
  getMessages: (conversationId) => invoke("get_messages", { conversationId }),
  sendMessage: (conversationId, text) => invoke("send_message", { conversationId, text }),
  cancelAgent: (conversationId) => invoke("cancel_agent", { conversationId }),
  resolveApproval: (conversationId, requestId, allow) =>
    invoke("resolve_approval", { conversationId, requestId, allow }),
  setApprovalMode: (conversationId, mode) => invoke("set_approval_mode", { conversationId, mode }),
  updateConversationModel: (conversationId, providerId, model) =>
    invoke("update_conversation_model", { conversationId, providerId, model }),
  listProviders: () => invoke("list_providers"),
  upsertProvider: (cfg) => invoke("upsert_provider", { cfg }),
  deleteProvider: (id) => invoke("delete_provider", { id }),
  setApiKey: (providerId, key) => invoke("set_api_key", { providerId, key }),
  deleteApiKey: (providerId) => invoke("delete_api_key", { providerId }),
  onAgentEvent: (handler) => listen("agent-event", (e) => handler(e.payload)),
};
```

- [x] **Step 2: `ui/app.js` — app state, sidebar, conversation selection**

```js
import { api } from "./api.js";
import { renderMessages } from "./render.js";
import { initSettings } from "./settings.js";

export const state = {
  workspaces: [],
  providers: [],
  conversations: new Map(), // workspaceId -> ConversationRow[]
  active: null, // active ConversationRow
  running: new Set(), // conversation_ids with a live agent
  streaming: false,
};

const $ = (id) => document.getElementById(id);

// Wrap async UI handlers: surface failures instead of silent unhandled rejections.
const guard = (fn) => (e) => fn(e).catch((err) => {
  console.error(err);
  alert(String(err));
});

async function boot() {
  const initial = await api.getInitialState();
  state.workspaces = initial.workspaces;
  state.providers = initial.providers;
  for (const ws of state.workspaces) {
    state.conversations.set(ws.id, await api.listConversations(ws.id));
  }
  renderSidebar();
  initSettings(state, refreshProviders);
  // Restore last conversation if it still exists.
  if (initial.config.last_conversation_id) {
    for (const convs of state.conversations.values()) {
      const found = convs.find((c) => c.id === initial.config.last_conversation_id);
      if (found) {
        await selectConversation(found);
        break;
      }
    }
  }
}

export async function refreshProviders() {
  state.providers = await api.listProviders();
  renderModelPicker();
}

export function renderSidebar() {
  const list = $("workspace-list");
  list.innerHTML = "";
  for (const ws of state.workspaces) {
    const wsEl = document.createElement("div");
    wsEl.className = "workspace";
    const header = document.createElement("div");
    header.className = "workspace-header";
    header.textContent = `📁 ${ws.name}`;
    header.title = ws.path;
    wsEl.appendChild(header);
    const convs = state.conversations.get(ws.id) || [];
    for (const conv of convs) {
      const el = document.createElement("div");
      el.className = "conversation" + (state.active?.id === conv.id ? " active" : "");
      el.textContent = conv.title;
      if (state.running.has(conv.id)) {
        const dot = document.createElement("span");
        dot.className = "running-dot";
        el.appendChild(dot);
      }
      el.onclick = guard(() => selectConversation(conv));
      wsEl.appendChild(el);
    }
    list.appendChild(wsEl);
  }
}

export async function selectConversation(conv) {
  state.active = conv;
  renderSidebar(); // instant feedback, before the fetch
  $("chat-title").textContent = conv.title;
  $("composer").classList.remove("hidden");
  renderModelPicker();
  renderModeToggle(conv.approval_mode);
  const msgs = await api.getMessages(conv.id);
  // A newer click may have switched away while the fetch was in flight.
  if (state.active?.id !== conv.id) return;
  renderMessages(msgs);
  api.setUiState(conv.workspace_id, conv.id).catch(() => {});
}

export function renderModeToggle(mode) {
  $("mode-toggle").textContent = mode === "auto" ? "Auto" : "Manual";
}

export function renderModelPicker() {
  const slot = $("model-slot");
  slot.innerHTML = "";
  if (!state.active) return;
  const conv = state.active;
  const select = document.createElement("select");
  select.id = "model-picker";
  for (const p of state.providers) {
    for (const m of p.models) {
      const opt = document.createElement("option");
      opt.value = `${p.id}/${m}`;
      opt.textContent = `${p.label} · ${m}`;
      if (p.id === conv.provider_id && m === conv.model) opt.selected = true;
      select.appendChild(opt);
    }
  }
  if (select.options.length === 0) {
    const hint = document.createElement("span");
    hint.className = "dim";
    hint.textContent = "No models — add one in Settings";
    slot.appendChild(hint);
    return;
  }
  select.onchange = guard(async () => {
    const [providerId, ...rest] = select.value.split("/");
    const model = rest.join("/");
    await api.updateConversationModel(conv.id, providerId, model);
    conv.provider_id = providerId;
    conv.model = model;
  });
  slot.appendChild(select);
}

$("new-conversation").onclick = guard(async () => {
  if (state.workspaces.length === 0) {
    alert("Add a workspace first (Settings → Add workspace).");
    return;
  }
  const ws = state.workspaces.find((w) => w.id === state.active?.workspace_id) || state.workspaces[0];
  const provider = state.providers.find((p) => p.models.length > 0) || state.providers[0];
  if (!provider) {
    alert("Add a provider first (Settings).");
    return;
  }
  const model = provider.models[0] || "";
  if (!model) {
    alert(`Provider ${provider.label} has no models — add one in Settings.`);
    return;
  }
  const id = await api.createConversation(ws.id, "New Conversation", provider.id, model);
  state.conversations.set(ws.id, await api.listConversations(ws.id));
  renderSidebar();
  const conv = state.conversations.get(ws.id).find((c) => c.id === id);
  if (conv) await selectConversation(conv);
});

$("mode-toggle").onclick = guard(async () => {
  if (!state.active) return;
  const next = state.active.approval_mode === "auto" ? "manual" : "auto";
  await api.setApprovalMode(state.active.id, next);
  state.active.approval_mode = next;
  renderModeToggle(next);
});

boot().catch((e) => {
  document.getElementById("chat-title").textContent = `Boot failed: ${e}`;
});
```

(Note: `render.js` and `settings.js` are U5/U6 — for THIS task, create minimal stubs so imports resolve: `ui/render.js` exporting `renderMessages(msgs)` that just lists raw text, `ui/settings.js` exporting `initSettings(state, refresh)` wiring open/close buttons. U5/U6 replace them.)

Stub `ui/render.js`:

```js
export function renderMessages(msgs) {
  const el = document.getElementById("messages");
  el.innerHTML = msgs
    .map((m) => `<div>${m.role}: ${m.parts.map((p) => p.text || p.content || p.name || "").join(" ")}</div>`)
    .join("");
}
```

Stub `ui/settings.js`:

```js
export function initSettings(_state, _refresh) {
  document.getElementById("open-settings").onclick = () =>
    document.getElementById("settings").classList.remove("hidden");
  document.getElementById("close-settings").onclick = () =>
    document.getElementById("settings").classList.add("hidden");
}
```

- [x] **Step 3: Extend `ui/style.css` — sidebar lists**

```css
.workspace { margin-bottom: 6px; }
.workspace-header {
  font-weight: 600;
  padding: 6px 6px 2px;
  color: var(--text-dim);
  font-size: 12px;
  text-transform: uppercase;
  letter-spacing: 0.04em;
}
.conversation {
  padding: 5px 8px;
  border-radius: 6px;
  cursor: pointer;
  display: flex;
  justify-content: space-between;
  align-items: center;
  color: var(--text);
  font-size: 13px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.conversation:hover { background: var(--bg-input); }
.conversation.active { background: var(--bg-input); outline: 1px solid var(--accent); }
.running-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--ok);
  flex-shrink: 0;
}
.dim { color: var(--text-dim); font-size: 12px; }
```

- [x] **Step 4: Verify**

`cargo tauri dev` — sidebar renders; settings overlay opens/closes; "+ New Conversation" after adding a workspace via SQL-less path (Settings stub can't add workspaces yet — verify conversation creation by adding a workspace row manually via a temporary test OR defer interactive check to U6; minimum bar: `cargo build` clean, `cargo test` green, window opens without console errors).

- [x] **Step 5: Commit**

```bash
cd /b/Jetbrains/projects/kimislop
git add ui src-tauri
git commit -m "feat(ui): sidebar, workspace/conversation navigation, model picker"
```

---

### Task U5: Chat — streaming, tool cards, approvals, markdown-lite

**Gates from the U4 review (hard requirements, not optional):**
1. This task REPLACES the `ui/render.js` stub (which injects unescaped content via innerHTML) before any send path goes live — the markdown renderer escapes everything.
2. After the first `sendMessage` in a conversation, refresh the conversation list + `#chat-title` — the bridge auto-renames "New Conversation" and the sidebar would otherwise stay stale until restart.

**Files:**
- Create: `ui/render.js` (replace stub), `ui/markdown.js`, `ui/events.js`
- Modify: `ui/app.js` (wire send/stop + event handler), `ui/style.css` (message bubbles, cards)

- [x] **Step 1: `ui/markdown.js` — minimal safe markdown**

```js
// Minimal markdown: escapes HTML, then handles ``` fences, `code`, **bold**, *italic*, lists, paragraphs.
export function renderMarkdown(src) {
  const esc = (s) =>
    s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
  const inline = (s) => {
    // Protect code spans from later passes (bold/italic must not apply inside code).
    const codes = [];
    let out = esc(s).replace(/`([^`]+)`/g, (_, c) => {
      codes.push(c);
      return `\u0000${codes.length - 1}\u0000`;
    });
    out = out
      .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
      .replace(/\*([^*]+)\*/g, "<em>$1</em>")
      .replace(/\[([^\]]+)\]\((https?:\/\/[^)]+)\)/g, '<a href="$2" target="_blank" rel="noopener">$1</a>');
    return out.replace(/\u0000(\d+)\u0000/g, (_, i) => `<code>${codes[Number(i)]}</code>`);
  };
  // Split on ``` fences: odd indices are code blocks (first line = language hint, skipped).
  let html = "";
  const parts = src.split("```");
  for (let i = 0; i < parts.length; i++) {
    if (i % 2 === 1) {
      const nl = parts[i].indexOf("\n");
      const code = nl === -1 ? parts[i] : parts[i].slice(nl + 1);
      html += `<pre><code>${esc(code)}</code></pre>`;
    } else {
      const paragraphs = parts[i].split(/\n{2,}/);
      for (const p of paragraphs) {
        if (!p.trim()) continue;
        // Line-by-line: consecutive list lines form a <ul>; other lines form
        // paragraphs — mixed list+text blocks keep both (no dropped lines).
        let listItems = [];
        let para = [];
        const flushPara = () => {
          if (para.length) {
            // Single newlines become <br> — LLM output uses them structurally.
            html += `<p>${para.map((l) => inline(l)).join("<br>")}</p>`;
            para = [];
          }
        };
        const flushList = () => {
          if (listItems.length) {
            html += `<ul>${listItems.join("")}</ul>`;
            listItems = [];
          }
        };
        for (const line of p.split("\n")) {
          const m = line.match(/^\s*[-*] (.*)$/);
          if (m) {
            flushPara();
            listItems.push(`<li>${inline(m[1])}</li>`);
          } else {
            flushList();
            para.push(line);
          }
        }
        flushList();
        flushPara();
      }
    }
  }
  return html;
}
```

(Note: the plan's earlier sketch loop is gone — the final file contains only the `parts`-based implementation above.)

- [x] **Step 2: `ui/render.js` — message history + live elements (replaces stub)**

```js
import { renderMarkdown } from "./markdown.js";

const $ = (id) => document.getElementById(id);

function scrollToBottom() {
  const el = $("messages");
  el.scrollTop = el.scrollHeight;
}

export function addBubble(role) {
  const el = document.createElement("div");
  el.className = `bubble ${role}`;
  $("messages").appendChild(el);
  scrollToBottom();
  return el;
}

export function renderTextPart(container, text) {
  const div = document.createElement("div");
  div.className = "md";
  div.innerHTML = renderMarkdown(text);
  container.appendChild(div);
}

export function renderToolCallCard(call) {
  const card = document.createElement("div");
  card.className = "tool-card";
  const head = document.createElement("div");
  head.className = "tool-head";
  head.textContent = `🔧 ${call.name}`;
  const args = document.createElement("pre");
  args.className = "tool-args";
  args.textContent = prettyArgs(call.args_json);
  const status = document.createElement("div");
  status.className = "tool-status";
  card.append(head, args, status);
  return card;
}

export function prettyArgs(argsJson) {
  try {
    const v = JSON.parse(argsJson);
    const s = JSON.stringify(v, null, 1);
    return s.length > 300 ? s.slice(0, 300) + "…" : s;
  } catch {
    return argsJson.slice(0, 300);
  }
}

export function renderResultOnCard(card, result) {
  const status = card.querySelector(".tool-status");
  status.textContent = result.is_error ? `✗ ${result.content.slice(0, 200)}` : `✓ ${result.content.slice(0, 200)}`;
  status.className = "tool-status " + (result.is_error ? "err" : "ok");
  const pre = document.createElement("pre");
  pre.className = "tool-result";
  pre.textContent = result.content.length > 1000 ? result.content.slice(0, 1000) + "\n…" : result.content;
  card.appendChild(pre);
}

export function renderMessages(msgs) {
  const el = $("messages");
  el.innerHTML = "";
  for (const m of msgs) {
    if (m.role === "system") continue;
    if (m.role === "tool") {
      // attach results to the preceding assistant tool cards by order
      for (const p of m.parts) {
        if (p.type === "tool_result") {
          const card = document.querySelector(`[data-call-id="${p.tool_call_id}"]`);
          if (card) renderResultOnCard(card, p);
        }
      }
      continue;
    }
    const bubble = addBubble(m.role);
    for (const p of m.parts) {
      if (p.type === "text") renderTextPart(bubble, p.text);
      if (p.type === "tool_call") {
        const card = renderToolCallCard(p);
        card.dataset.callId = p.id;
        bubble.appendChild(card);
      }
    }
  }
  scrollToBottom();
}
```

- [x] **Step 3: `ui/events.js` — live agent event handling**

```js
import { state } from "./app.js";
import { api } from "./api.js";
import { addBubble, renderTextPart, renderToolCallCard, prettyArgs } from "./render.js";

const $ = (id) => document.getElementById(id);

// Per-conversation live state, so switching conversations mid-run loses
// nothing: streamed text accumulates here (not only in the DOM), and pending
// approvals are remembered until resolved or finished.
const streamBuffers = new Map(); // conversation_id -> raw streamed text
export const pendingApprovals = new Map(); // conversation_id -> {request_id, tool_call_id, name, args_json}

let currentTextBubble = null;

export function handleAgentEvent(payload) {
  const { conversation_id, event } = payload;

  // Non-active conversations: track state only (no DOM).
  if (state.active?.id !== conversation_id) {
    if (event.kind === "text_delta") {
      streamBuffers.set(conversation_id, (streamBuffers.get(conversation_id) || "") + event.data);
    } else if (event.kind === "approval_requested") {
      pendingApprovals.set(conversation_id, { ...event.data });
    } else if (event.kind === "tool_call_finished") {
      pendingApprovals.delete(conversation_id);
    } else if (["message_done", "error", "cancelled"].includes(event.kind)) {
      streamBuffers.delete(conversation_id);
      pendingApprovals.delete(conversation_id);
    }
    return;
  }

  switch (event.kind) {
    case "text_delta": {
      const raw = (streamBuffers.get(conversation_id) || "") + event.data;
      streamBuffers.set(conversation_id, raw);
      // The bubble may have been detached by a history re-render — recreate it.
      if (!currentTextBubble || !currentTextBubble.isConnected) {
        currentTextBubble = addBubble("assistant");
      }
      currentTextBubble.innerHTML = "";
      renderTextPart(currentTextBubble, raw);
      const el = document.getElementById("messages");
      el.scrollTop = el.scrollHeight;
      break;
    }
    case "tool_call_proposed": {
      currentTextBubble = null;
      const card = renderToolCallCard({ name: event.data.name, args_json: event.data.args_json });
      card.dataset.callId = event.data.tool_call_id;
      card.querySelector(".tool-status").textContent = "running…";
      addBubble("assistant").appendChild(card);
      break;
    }
    case "approval_requested": {
      currentTextBubble = null;
      pendingApprovals.set(conversation_id, { ...event.data });
      addBubble("assistant").appendChild(buildApprovalCard(conversation_id, event.data));
      break;
    }
    case "tool_call_finished": {
      pendingApprovals.delete(conversation_id);
      const card = document.querySelector(`[data-call-id="${event.data.tool_call_id}"]`);
      if (card) {
        const status = card.querySelector(".tool-status");
        if (status) {
          status.textContent = (event.data.ok ? "✓ " : "✗ ") + event.data.summary.slice(0, 200);
          status.className = "tool-status " + (event.data.ok ? "ok" : "err");
        }
        card.querySelector(".approval-buttons")?.remove();
      }
      break;
    }
    case "message_done":
      currentTextBubble = null;
      streamBuffers.delete(conversation_id);
      pendingApprovals.delete(conversation_id);
      $("stop-agent").classList.add("hidden");
      break;
    case "error": {
      currentTextBubble = null;
      streamBuffers.delete(conversation_id);
      pendingApprovals.delete(conversation_id);
      const bubble = addBubble("error");
      bubble.textContent = `Error: ${event.data}`;
      $("stop-agent").classList.add("hidden");
      break;
    }
    case "cancelled": {
      currentTextBubble = null;
      streamBuffers.delete(conversation_id);
      pendingApprovals.delete(conversation_id);
      const bubble = addBubble("error");
      bubble.textContent = "Cancelled.";
      $("stop-agent").classList.add("hidden");
      break;
    }
  }
}

export function buildApprovalCard(conversationId, data) {
  const card = document.createElement("div");
  card.className = "approval-card";
  card.dataset.callId = data.tool_call_id;
  const head = document.createElement("div");
  head.className = "tool-head";
  head.textContent = `⚠ ${data.name} needs approval`;
  const args = document.createElement("pre");
  args.className = "tool-args";
  args.textContent = prettyArgs(data.args_json);
  const buttons = document.createElement("div");
  buttons.className = "approval-buttons";
  const approve = document.createElement("button");
  approve.className = "approve";
  approve.textContent = "Approve";
  approve.onclick = () => {
    api.resolveApproval(conversationId, data.request_id, true).catch(() => {});
    buttons.remove();
  };
  const deny = document.createElement("button");
  deny.className = "deny";
  deny.textContent = "Deny";
  deny.onclick = () => {
    api.resolveApproval(conversationId, data.request_id, false).catch(() => {});
    buttons.remove();
  };
  buttons.append(approve, deny);
  card.append(head, args, buttons);
  return card;
}

/// Called by selectConversation after rendering history: re-render any live
/// approval card for this conversation (stream text resumes on next delta).
export function resumeLiveState(conversationId) {
  currentTextBubble = null;
  const pending = pendingApprovals.get(conversationId);
  if (pending) {
    addBubble("assistant").appendChild(buildApprovalCard(conversationId, pending));
  }
}

export function resetEventState() {
  currentTextBubble = null;
}
```

- [x] **Step 4: Wire send/stop/events in `ui/app.js` — append (and import `handleAgentEvent`, `resetEventState`)**

Also make two edits to `selectConversation` (from U4): sync the stop button to the conversation's live state, and resume live approval/stream state after rendering history:

```js
export async function selectConversation(conv) {
  state.active = conv;
  renderSidebar(); // instant feedback, before the fetch
  $("chat-title").textContent = conv.title;
  $("composer").classList.remove("hidden");
  renderModelPicker();
  renderModeToggle(conv.approval_mode);
  $("stop-agent").classList.toggle("hidden", !state.running.has(conv.id));
  const msgs = await api.getMessages(conv.id);
  // A newer click may have switched away while the fetch was in flight.
  if (state.active?.id !== conv.id) return;
  renderMessages(msgs);
  resumeLiveState(conv.id);
  api.setUiState(conv.workspace_id, conv.id).catch(() => {});
}
```

```js
import { handleAgentEvent, resetEventState, resumeLiveState } from "./events.js";

api.onAgentEvent((payload) => {
  // Running-set bookkeeping (sidebar dots) for ALL conversations, then
  // delegate rendering to events.js. Only re-render the sidebar on change.
  const k = payload.event.kind;
  const cid = payload.conversation_id;
  let changed = false;
  if (["text_delta", "tool_call_proposed", "approval_requested"].includes(k) && !state.running.has(cid)) {
    state.running.add(cid);
    changed = true;
  }
  if (["message_done", "error", "cancelled"].includes(k) && state.running.has(cid)) {
    state.running.delete(cid);
    changed = true;
  }
  if (changed) renderSidebar();
  handleAgentEvent(payload);
});

$("send").onclick = send;
$("input").addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey) {
    e.preventDefault();
    send();
  }
});

async function send() {
  const text = $("input").value.trim();
  if (!text || !state.active) return;
  const cid = state.active.id;
  if (state.running.has(cid)) return;
  $("input").value = "";
  resetEventState();
  const bubble = document.createElement("div");
  bubble.className = "bubble user";
  bubble.textContent = text;
  document.getElementById("messages").appendChild(bubble);
  // Mark running NOW (the first event may lag) — closes the double-send window.
  state.running.add(cid);
  renderSidebar();
  $("stop-agent").classList.remove("hidden");
  try {
    await api.sendMessage(cid, text);
    // The bridge may have auto-renamed the conversation on first send — refresh.
    const convs = await api.listConversations(state.active.workspace_id);
    state.conversations.set(state.active.workspace_id, convs);
    const fresh = convs.find((c) => c.id === cid);
    if (fresh) {
      state.active = fresh;
      $("chat-title").textContent = fresh.title;
    }
    renderSidebar();
  } catch (e) {
    state.running.delete(cid);
    renderSidebar();
    const err = document.createElement("div");
    err.className = "bubble error";
    err.textContent = `Error: ${e}`;
    document.getElementById("messages").appendChild(err);
    $("stop-agent").classList.add("hidden");
  }
}

$("stop-agent").onclick = () => {
  if (state.active) api.cancelAgent(state.active.id).catch(() => {});
};
```

- [x] **Step 5: Extend `ui/style.css` — bubbles and cards**

```css
.bubble { max-width: 80%; margin: 6px 0; padding: 10px 14px; border-radius: 10px; }
.bubble.user { background: #2b3a5a; margin-left: auto; }
.bubble.assistant { background: var(--bg-alt); border: 1px solid var(--border); }
.bubble.error { background: #3a2326; border: 1px solid var(--danger); color: #f0b9b4; }
.bubble .md p { margin: 6px 0; }
.bubble pre { background: #121215; padding: 10px; border-radius: 6px; overflow-x: auto; font-size: 12px; }
.bubble code { font-family: "Cascadia Code", Consolas, monospace; font-size: 12px; }
.tool-card, .approval-card {
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 8px 10px;
  margin: 6px 0;
  background: var(--bg-input);
}
.approval-card { border-color: var(--warn); }
.tool-head { font-weight: 600; margin-bottom: 4px; }
.tool-args, .tool-result { font-size: 11px; color: var(--text-dim); margin: 4px 0; white-space: pre-wrap; }
.tool-status { font-size: 12px; margin-top: 4px; }
.tool-status.ok { color: var(--ok); }
.tool-status.err { color: var(--danger); }
.approval-buttons { display: flex; gap: 8px; margin-top: 6px; }
.approval-buttons .approve { border-color: var(--ok); }
.approval-buttons .deny { border-color: var(--danger); }
#stop-agent { color: var(--danger); }
```

- [x] **Step 6: Verify**

`cargo tauri dev` — with Ollama (if present) or a configured provider: send a message, see streaming text; approvals appear for write/shell in Manual mode; stop button cancels. Without keys: verify rendering by checking message history renders after a mocked run (or accept code review + U7 smoke checklist). Minimum bar: build clean, no console errors, manual interaction works where a provider exists.

- [x] **Step 7: Commit**

```bash
cd /b/Jetbrains/projects/kimislop
git add ui
git commit -m "feat(ui): streaming chat, tool cards, inline approvals"
```

---

### Task U6: Settings view — providers, keys, workspaces

**Files:**
- Create: `ui/settings.js` (replace stub)
- Modify: `ui/style.css` (settings list rows)

- [ ] **Step 1: `ui/settings.js`**

```js
import { api } from "./api.js";
import { state, renderSidebar } from "./app.js";

const $ = (id) => document.getElementById(id);

export function initSettings(_state, refreshProviders) {
  $("open-settings").onclick = () => {
    renderSettings();
    $("settings").classList.remove("hidden");
  };
  $("close-settings").onclick = () => $("settings").classList.add("hidden");

  $("custom-provider-form").onsubmit = async (e) => {
    e.preventDefault();
    const label = $("cp-label").value.trim();
    const baseUrl = $("cp-base-url").value.trim();
    const models = $("cp-models").value.split(",").map((m) => m.trim()).filter(Boolean);
    const key = $("cp-key").value.trim();
    const id = label.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || `custom-${Date.now()}`;
    await api.upsertProvider({
      id,
      label,
      kind: "open_ai_compatible",
      base_url: baseUrl,
      has_key: false,
      models,
      extra_headers: [],
    });
    if (key) await api.setApiKey(id, key);
    e.target.reset();
    await refreshProviders();
    renderSettings();
  };

  $("workspace-form").onsubmit = async (e) => {
    e.preventDefault();
    try {
      await api.addWorkspace($("ws-name").value.trim(), $("ws-path").value.trim());
      state.workspaces = await api.listWorkspaces();
      for (const ws of state.workspaces) {
        if (!state.conversations.has(ws.id)) {
          state.conversations.set(ws.id, await api.listConversations(ws.id));
        }
      }
      renderSidebar();
      e.target.reset();
    } catch (err) {
      alert(`Could not add workspace: ${err}`);
    }
  };
}

function renderSettings() {
  const list = $("provider-list");
  list.innerHTML = "";
  for (const p of state.providers) {
    const row = document.createElement("div");
    row.className = "provider-row";

    const head = document.createElement("div");
    head.className = "provider-head";
    const strong = document.createElement("strong");
    strong.textContent = p.label;
    const kind = document.createElement("span");
    kind.className = "dim";
    kind.textContent = p.kind;
    const badge = document.createElement("span");
    badge.className = "badge " + (p.has_key ? "ok" : "warn");
    badge.textContent = p.has_key ? "key set" : "no key";
    head.append(strong, " ", kind, " ", badge);

    const baseLabel = document.createElement("label");
    baseLabel.textContent = "Base URL ";
    const baseInput = document.createElement("input");
    baseInput.className = "p-base";
    baseInput.value = p.base_url ?? "";
    baseInput.placeholder = "(default)";
    baseLabel.appendChild(baseInput);

    const modelsLabel = document.createElement("label");
    modelsLabel.textContent = "Models ";
    const modelsInput = document.createElement("input");
    modelsInput.className = "p-models";
    modelsInput.value = p.models.join(", ");
    modelsLabel.appendChild(modelsInput);

    const actions = document.createElement("div");
    actions.className = "provider-actions";
    const mkBtn = (cls, text) => {
      const b = document.createElement("button");
      b.className = cls;
      b.textContent = text;
      return b;
    };
    actions.append(mkBtn("p-save", "Save"), mkBtn("p-set-key", "Set API key"));
    if (p.has_key) actions.append(mkBtn("p-del-key", "Delete key"));
    actions.append(mkBtn("p-delete", "Delete provider"));

    row.append(head, baseLabel, modelsLabel, actions);
    row.querySelector(".p-save").onclick = async () => {
      p.base_url = row.querySelector(".p-base").value.trim() || null;
      p.models = row.querySelector(".p-models").value.split(",").map((m) => m.trim()).filter(Boolean);
      await api.upsertProvider(p);
    };
    row.querySelector(".p-set-key").onclick = async () => {
      const key = prompt(`API key for ${p.label} (stored in OS keychain):`);
      if (key) {
        await api.setApiKey(p.id, key.trim());
        p.has_key = true;
        renderSettings();
      }
    };
    const delKey = row.querySelector(".p-del-key");
    if (delKey) {
      delKey.onclick = async () => {
        await api.deleteApiKey(p.id);
        p.has_key = false;
        renderSettings();
      };
    }
    row.querySelector(".p-delete").onclick = async () => {
      if (confirm(`Delete provider ${p.label}?`)) {
        await api.deleteProvider(p.id);
        state.providers = await api.listProviders();
        renderSettings();
      }
    };
    list.appendChild(row);
  }
}
```

- [ ] **Step 2: Extend `ui/style.css`**

```css
.provider-row {
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 10px;
  margin: 8px 0;
}
.provider-head { display: flex; gap: 8px; align-items: center; margin-bottom: 6px; }
.badge { font-size: 11px; padding: 2px 6px; border-radius: 4px; }
.badge.ok { background: #1d3a24; color: var(--ok); }
.badge.warn { background: #3a2f1d; color: var(--warn); }
.provider-actions { display: flex; gap: 8px; margin-top: 6px; }
```

- [ ] **Step 3: Verify**

`cargo tauri dev` — Settings: providers render with key badges; set/delete key works (check Windows Credential Manager has a `supergravity/ollama`-style entry after setting); custom provider add works; workspace add persists across restart.

- [ ] **Step 4: Commit**

```bash
cd /b/Jetbrains/projects/kimislop
git add ui
git commit -m "feat(ui): settings — providers, api keys, workspaces"
```

---

### Task U7: Final verification

- [ ] **Step 1: Full Rust suite + clippy + fmt**

```bash
cd /b/Jetbrains/projects/kimislop/src-tauri
cargo test          # all green
cargo clippy --all-targets -- -D warnings
cargo fmt
```

- [ ] **Step 2: Production build**

```bash
cargo tauri build --no-bundle
```

Expected: completes; binary at `src-tauri/target/release/supergravity.exe`. (`--no-bundle` skips the MSI/NSIS installer packaging, which needs extra tooling we don't need for v1.)

- [ ] **Step 2b: Revisit CSP**

Set an explicit CSP in `tauri.conf.json` now that the full UI exists (all assets are local files): replace `"csp": null` with `"csp": "default-src 'self'; style-src 'self' 'unsafe-inline'"` and smoke-test the app once (styling must still work — our CSS is a single local file, but confirm no inline style attributes broke). If anything renders wrong, adjust the policy rather than reverting to `null`.

- [ ] **Step 3: Run the release binary smoke checklist**

- App opens; sidebar shows seeded 4 providers in Settings (OpenAI, Anthropic, Gemini, Ollama)
- Add a workspace (a real project dir); create a conversation
- With Ollama running locally (or any configured key): send "list the files in this workspace" — agent calls `list_dir`, card shows ✓
- Manual mode: on a write/shell call an approval card appears; Deny → agent adapts; Approve → executes
- Stop button cancels mid-stream
- Restart: conversation + messages persist

- [ ] **Step 4: Commit + update docs**

```bash
cd /b/Jetbrains/projects/kimislop
git add -A
git commit -m "chore: final verification — release build green"
```

---

## Done criteria for this plan

- `cargo test` + `cargo clippy --all-targets -- -D warnings` green
- `cargo tauri build --no-bundle` produces `supergravity.exe`
- The app runs the full loop against at least one real provider (Ollama local or a keyed API): streaming text, tool cards, inline approvals, stop, persistence across restart
