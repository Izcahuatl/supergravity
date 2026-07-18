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
      el.onclick = () => selectConversation(conv);
      wsEl.appendChild(el);
    }
    list.appendChild(wsEl);
  }
}

export async function selectConversation(conv) {
  state.active = conv;
  $("chat-title").textContent = conv.title;
  $("composer").classList.remove("hidden");
  renderModelPicker();
  renderModeToggle(conv.approval_mode);
  const msgs = await api.getMessages(conv.id);
  renderMessages(msgs);
  renderSidebar();
  api.setUiState(conv.workspace_id, conv.id);
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
  select.onchange = async () => {
    const [providerId, ...rest] = select.value.split("/");
    const model = rest.join("/");
    await api.updateConversationModel(conv.id, providerId, model);
    conv.provider_id = providerId;
    conv.model = model;
  };
  slot.appendChild(select);
}

$("new-conversation").onclick = async () => {
  if (state.workspaces.length === 0) {
    alert("Add a workspace first (Settings → Add workspace).");
    return;
  }
  const ws = state.workspaces[0];
  const provider = state.providers.find((p) => p.models.length > 0) || state.providers[0];
  if (!provider) {
    alert("Add a provider first (Settings).");
    return;
  }
  const model = provider.models[0] || "";
  const id = await api.createConversation(ws.id, "New Conversation", provider.id, model);
  state.conversations.set(ws.id, await api.listConversations(ws.id));
  renderSidebar();
  const conv = state.conversations.get(ws.id).find((c) => c.id === id);
  if (conv) await selectConversation(conv);
};

$("mode-toggle").onclick = async () => {
  if (!state.active) return;
  const next = state.active.approval_mode === "auto" ? "manual" : "auto";
  await api.setApprovalMode(state.active.id, next);
  state.active.approval_mode = next;
  renderModeToggle(next);
};

boot().catch((e) => {
  document.getElementById("chat-title").textContent = `Boot failed: ${e}`;
});
