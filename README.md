# supergravity

An agentic coding desktop app in the spirit of Google Antigravity, without the
vendor lock-in. It runs against OpenAI, Anthropic, Gemini, local Ollama models,
or any OpenAI-compatible endpoint.

The agent reads, writes, and edits files, searches code, and runs shell
commands inside a project folder you give it. Every class of action has its own
permission policy, so you decide what runs free and what asks first.

## Running it

Grab the installer from Releases, or build from source:

```
cd src-tauri
cargo tauri build
```

You'll need Rust and Tauri's platform prerequisites. Windows is the only tested
target right now.

## Using it

1. Open Settings, pick a provider, paste your API key (stored in the OS keychain).
2. Enable the models you want in the picker.
3. Add a project folder.
4. Start typing. The conversation is created on your first message.

The composer takes `@` to attach files and `/` for quick actions
(`/plan`, `/auto`, `/manual`, `/model`, `/new`).

## What it does differently

**Workshop.** Every conversation gets a scratch directory outside your project.
The agent uses it for python scripts and experiments with full permissions, so
your project only sees intentional changes.

**Checkpoints.** File changes are snapshotted per turn. Right-click any of your
messages to rewind the conversation and the files to that point, or revert a
single file from the Review panel.

**Approvals that make sense.** Writes and shell commands ask first in Manual
mode. External paths (outside the project and Workshop) always ask, even in
Auto. All of it is tunable in Settings.

**No surprises.** No telemetry, no accounts, no sync. Everything lives in a
local SQLite database and your OS keychain.

## Stack

- Rust core: agent loop, tools, providers, SQLite store
- Tauri v2 bridge and shell
- Vanilla JS/CSS UI, no framework, no build step

## Status

Early. Windows only, tested against a handful of models. If something breaks,
launch with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222`
and poke at the devtools on port 9222.
