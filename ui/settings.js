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
    const keyBadge = p.has_key ? `<span class="badge ok">key set</span>` : `<span class="badge warn">no key</span>`;
    row.innerHTML = `
      <div class="provider-head"><strong>${p.label}</strong> <span class="dim">${p.kind}</span> ${keyBadge}</div>
      <label>Base URL <input class="p-base" value="${p.base_url ?? ""}" placeholder="(default)"></label>
      <label>Models <input class="p-models" value="${p.models.join(", ")}"></label>
      <div class="provider-actions">
        <button class="p-save">Save</button>
        <button class="p-set-key">Set API key</button>
        ${p.has_key ? '<button class="p-del-key">Delete key</button>' : ""}
        <button class="p-delete">Delete provider</button>
      </div>`;
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
