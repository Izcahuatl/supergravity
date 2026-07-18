import { api } from "./api.js";
import { renderMessages } from "./render.js";
import { initSettings } from "./settings.js";
import { handleAgentEvent, resetEventState, resumeLiveState } from "./events.js";

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
export const guard = (fn) => (e) => fn(e).catch((err) => {
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
  let restored = false;
  if (initial.config.last_conversation_id) {
    for (const convs of state.conversations.values()) {
      const found = convs.find((c) => c.id === initial.config.last_conversation_id);
      if (found) {
        await selectConversation(found);
        restored = true;
        break;
      }
    }
  }
  if (!restored) renderEmptyState();
}

/// Welcome screen when no conversation is selected.
export function renderEmptyState() {
  const el = $("messages");
  el.innerHTML = "";
  const div = document.createElement("div");
  div.className = "empty-state";
  const title = document.createElement("div");
  title.className = "empty-title";
  title.textContent = "supergravity";
  const sub = document.createElement("div");
  sub.className = "empty-sub";
  sub.textContent = "Agent mission control — any model, your rules.";
  const hints = document.createElement("ul");
  hints.className = "empty-hints";
  for (const hint of [
    "Pick a conversation on the left, or hit + New Conversation",
    "Set API keys in ⚙ Settings — or run a local model via Ollama",
    "Manual mode asks before every write or command; Auto just goes",
  ]) {
    const li = document.createElement("li");
    li.textContent = hint;
    hints.appendChild(li);
  }
  div.append(title, sub, hints);
  el.appendChild(div);
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
      const del = document.createElement("button");
      del.className = "conv-delete";
      del.textContent = "✕";
      del.title = "Delete conversation";
      del.onclick = guard(async (e) => {
        e.stopPropagation();
        if (!confirm(`Delete "${conv.title}"?`)) return;
        await api.deleteConversation(conv.id);
        state.conversations.set(ws.id, await api.listConversations(ws.id));
        if (state.active?.id === conv.id) {
          state.active = null;
          $("chat-title").textContent = "Select or create a conversation";
          $("chat-ws").textContent = "";
          $("composer").classList.add("hidden");
          renderEmptyState();
          $("stop-agent").classList.add("hidden");
        }
        renderSidebar();
      });
      el.appendChild(del);
      el.onclick = guard(() => selectConversation(conv));
      wsEl.appendChild(el);
    }
    list.appendChild(wsEl);
  }
}

function workspaceName(id) {
  return state.workspaces.find((w) => w.id === id)?.name ?? "";
}

export async function selectConversation(conv) {
  state.active = conv;
  renderSidebar(); // instant feedback, before the fetch
  $("chat-title").textContent = conv.title;
  $("chat-ws").textContent = `📁 ${workspaceName(conv.workspace_id)}`;
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
  const provider =
    state.providers.find((p) => p.has_key && p.models.length > 0) ||
    state.providers.find((p) => p.kind === "ollama") ||
    state.providers.find((p) => p.models.length > 0) ||
    state.providers[0];
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
