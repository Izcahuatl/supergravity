import { api } from "./api.js";
import { state, renderSidebar, renderEmptyState, guard } from "./app.js";

const $ = (id) => document.getElementById(id);

// Captured at init — renderSettings is top-level but needs to refresh providers.
let refreshProvidersFn = async () => {};

export function initSettings(_state, refreshProviders) {
  refreshProvidersFn = refreshProviders;
  $("open-settings").onclick = () => {
    renderSettings();
    renderWorkspaces();
    $("settings").classList.remove("hidden");
  };
  $("close-settings").onclick = () => $("settings").classList.add("hidden");

  $("custom-provider-form").onsubmit = guard(async (e) => {
    e.preventDefault();
    const label = $("cp-label").value.trim();
    const baseUrl = $("cp-base-url").value.trim();
    const models = $("cp-models").value.split(",").map((m) => m.trim()).filter(Boolean);
    const key = $("cp-key").value.trim();
    const id = label.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || `custom-${Date.now()}`;
    if (state.providers.some((p) => p.id === id)) {
      // Upsert would silently clobber the existing row (including has_key).
      alert(`A provider with id "${id}" already exists — edit it above instead.`);
      return;
    }
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
    await refreshProvidersFn();
    renderSettings();
  });

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
      renderWorkspaces();
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
    if (p.kind === "ollama") {
      const fetchBtn = mkBtn("p-fetch", "Fetch models");
      fetchBtn.onclick = guard(async () => {
        const models = await api.listLocalModels(p.id);
        if (models.length === 0) {
          alert("No models on the Ollama server — pull one first (ollama pull …).");
          return;
        }
        row.querySelector(".p-models").value = models.join(", ");
      });
      actions.appendChild(fetchBtn);
    }

    row.append(head, baseLabel, modelsLabel, actions);
    row.querySelector(".p-save").onclick = guard(async () => {
      p.base_url = row.querySelector(".p-base").value.trim() || null;
      p.models = row.querySelector(".p-models").value.split(",").map((m) => m.trim()).filter(Boolean);
      await api.upsertProvider(p);
      await refreshProvidersFn();
      renderSettings();
    });
    row.querySelector(".p-set-key").onclick = () => {
      // WebView2 doesn't support window.prompt — inline password input instead.
      const keyRow = document.createElement("div");
      keyRow.className = "provider-actions";
      const input = document.createElement("input");
      input.type = "password";
      input.placeholder = `API key for ${p.label}`;
      const save = document.createElement("button");
      save.textContent = "Save key";
      const cancelBtn = document.createElement("button");
      cancelBtn.textContent = "Cancel";
      keyRow.append(input, save, cancelBtn);
      actions.replaceWith(keyRow);
      input.focus();
      save.onclick = guard(async () => {
        const key = input.value.trim();
        if (!key) return;
        await api.setApiKey(p.id, key);
        p.has_key = true;
        renderSettings();
      });
      cancelBtn.onclick = () => renderSettings();
    };
    const delKey = row.querySelector(".p-del-key");
    if (delKey) {
      delKey.onclick = guard(async () => {
        await api.deleteApiKey(p.id);
        p.has_key = false;
        renderSettings();
      });
    }
    row.querySelector(".p-delete").onclick = guard(async () => {
      if (confirm(`Delete provider ${p.label}?`)) {
        await api.deleteProvider(p.id);
        await refreshProvidersFn();
        renderSettings();
      }
    });
    list.appendChild(row);
  }
}

export function renderWorkspaces() {
  const list = $("workspace-list-settings");
  list.innerHTML = "";
  for (const ws of state.workspaces) {
    const row = document.createElement("div");
    row.className = "provider-row";
    const label = document.createElement("div");
    label.className = "provider-head";
    const strong = document.createElement("strong");
    strong.textContent = ws.name;
    const path = document.createElement("span");
    path.className = "dim";
    path.textContent = ws.path;
    const btn = document.createElement("button");
    btn.textContent = "Remove";
    btn.onclick = guard(async () => {
      if (!confirm(`Remove workspace "${ws.name}" and ALL its conversations?`)) return;
      await api.removeWorkspace(ws.id);
      state.workspaces = await api.listWorkspaces();
      state.conversations.delete(ws.id);
      if (state.active?.workspace_id === ws.id) {
        state.active = null;
        $("chat-title").textContent = "Select or create a conversation";
        $("chat-ws").textContent = "";
        $("composer").classList.add("hidden");
        renderEmptyState();
        $("stop-agent").classList.add("hidden");
      }
      renderSidebar();
      renderWorkspaces();
    });
    label.append(strong, " ", path, " ", btn);
    row.appendChild(label);
    list.appendChild(row);
  }
}
