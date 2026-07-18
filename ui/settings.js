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
