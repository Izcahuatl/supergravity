# Supergravity
Supergravity is sorta like Google Antigravity, except it's not stuck to JUST Gemini/Claude. OpenAI, Anthropic, Gemini, your local Ollama stuff, or whatever random OpenAI-compatible endpoint you found.

---
## Features?
- Use (almost) any model. What else?
- Every chat gets a Workshop, a scratch folder for python and experiments
- Checkpoints on every file change
- Task plans with a checklist up top, `@file` mentions, diff previews
---
## Getting it
Grab the latest build from [Releases](../../releases). Duh.
> **Windows only for now.** I don't have a Linux machine to test, sorry!!
---
## First launch
You'll get a few provider presets and zero enabled models, because I'm not going to guess what you pay for. Open Settings, drop an API key on a provider (or point Ollama at your machine), flip on some models, add a project folder. Done. That's the whole setup.

---
## How to use this thing
Honestly most of it is self-explanatory, but here's the stuff that isn't obvious.

### The composer
- `@` attaches files. Type `@` and start typing a name, pick it, the file goes along with your message.
- `/` does quick actions: `/plan` (make it plan first), `/auto` and `/manual` (approval mode), `/model`, `/new`.

### Permissions
In Settings, split three ways:
- **Project**: file writes and shell commands, ask or allow separately.
- **External**: anything outside the project. Asks *every* time by default, even in Auto mode. You can also block it entirely.
- **Workshop**: (mostly) full access by default.

### Rewind
Right-click any of your own messages. "Rewind to here" deletes that message and everything after, puts the text back in the box, and **restores the files** to how they were at that point. The Review panel can also revert single files if you don't want to nuke the whole turn.

---
## Will it run on my grandma's life support?
The app itself is tiny. Rust + Tauri, system webview, one SQLite file. It'll be fine.

If you're running local models, that's on you and your VRAM. A 4b model on CPU is usable-ish, a 9b wants a GPU. Cloud models need nothing but internet and money.

---
## Building it yourself
If you're into that:
- [Rust](https://rustup.rs/) (latest stable)
```bash
git clone https://github.com/Izcahuatl/supergravity.git
cd supergravity/src-tauri
cargo tauri build
```
The exe lands in `target/release/`, installers in `target/release/bundle/`.

---
## Waaaahh it broke
**"No enabled models"**
- Settings, enable some models

**The model emits garbage tool calls or fused JSON**
- Weak model. It recovers by itself usually but if a whole conversation is stuck, rewind to before it went wrong

**Nothing happens when I send**
- Check the provider actually has a key and the model is enabled. Cloud free tiers also just suck sometimes
- Still nothing? Launch with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222` and look at the devtools on port 9222 like an Amish caveman

---
## Serious License Stuff
Copyright (C) 2026 Izcahuatl

This program is free software: you can redistribute it and/or modify
it under the terms of the GNU Affero General Public License as published
by the Free Software Foundation, either version 3 of the License, or
(at your option) any later version.

This program is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
GNU Affero General Public License for more details.

You should have received a copy of the GNU Affero General Public License
along with this program. It can also be found at <https://www.gnu.org/licenses/>.
