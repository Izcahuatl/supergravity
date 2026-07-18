import { api } from "./api.js";
import { renderMessages } from "./render.js";
import { initSettings } from "./settings.js";
import { handleAgentEvent, resetEventState, resumeLiveState } from "./events.js";
import { initReview } from "./diffview.js";
import { icon } from "./icons.js";
import { makeDropdown } from "./dropdown.js";

export const state = {
  workspaces: [],
  providers: [],
  conversations: new Map(), // workspaceId -> ConversationRow[]
  active: null, // active ConversationRow
  running: new Set(), // conversation_ids with a live agent
  showAll: new Set(), // workspace_ids with expanded conversation lists
  lastWorkspaceId: null, // last active workspace (default for new conversations)
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
  initReview();
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
  if (!restored) renderCenterScreen();
}

/// Refetch + re-render the active conversation's history (e.g. after a run
/// completes, so the worked-for line and change cards appear).
export async function refreshMessages() {
  if (!state.active) return;
  const conv = state.active;
  const msgs = await api.getMessages(conv.id);
  if (state.active?.id !== conv.id) return;
  renderMessages(msgs);
}

function firstEnabledModel(p) {
  return p.models.find((m) => !(p.disabled_models ?? []).includes(m)) ?? null;
}

function preferredProvider() {
  return (
    state.providers.find((p) => p.has_key && firstEnabledModel(p)) ||
    state.providers.find((p) => p.kind === "ollama" && firstEnabledModel(p)) ||
    state.providers.find((p) => firstEnabledModel(p)) ||
    null
  );
}

/// Antigravity-style new-conversation screen: centered composer + project
/// picker. No conversation exists yet — it's created on the first message.
export function renderCenterScreen() {
  state.active = null;
  $("chat-title").textContent = "New conversation";
  $("chat-ws").textContent = "";
  $("composer").classList.add("hidden");
  $("messages").classList.add("hidden");
  $("messages").style.opacity = "";
  $("center-screen").classList.remove("hidden");
  // The center composer rises into place.
  const wrap = $("center-screen").querySelector(".center-wrap");
  wrap.style.animation = "none";
  requestAnimationFrame(() => {
    wrap.style.animation = "";
  });
  populateCenterProject();
}

function populateCenterProject() {
  const sel = $("center-project");
  sel.innerHTML = "";
  for (const ws of state.workspaces) {
    const opt = document.createElement("option");
    opt.value = ws.id;
    opt.textContent = ws.name;
    sel.appendChild(opt);
  }
  const sep = document.createElement("option");
  sep.value = "__new__";
  sep.textContent = "＋ New project (browse)…";
  sel.appendChild(sep);
  const last = state.workspaces.find((w) => w.id === state.lastWorkspaceId) ?? state.workspaces[0];
  if (last) sel.value = last.id;
  const p = preferredProvider();
  $("center-model").textContent = p
    ? `${p.label} · ${firstEnabledModel(p)}`
    : "no enabled models — open ⚙ Settings";
}

/// FLIP morph: the center composer travels to the bottom chat composer
/// (moving down and expanding), then the real composer takes over.
async function transitionCenterToComposer() {
  const center = $("center-screen");
  const card = center.querySelector(".center-composer");
  const composer = $("composer");
  if (!card || !composer) return;
  const start = card.getBoundingClientRect();
  // Measure the target without flashing it: reveal invisibly.
  composer.classList.remove("hidden");
  composer.style.opacity = "0";
  const end = composer.getBoundingClientRect();

  const ghost = card.cloneNode(true);
  const ta = ghost.querySelector("textarea");
  if (ta) ta.disabled = true;
  Object.assign(ghost.style, {
    position: "fixed",
    left: `${start.left}px`,
    top: `${start.top}px`,
    width: `${start.width}px`,
    height: `${start.height}px`,
    margin: "0",
    zIndex: 50,
    pointerEvents: "none",
    transformOrigin: "top left",
  });
  document.body.appendChild(ghost);

  // Fade the rest of the center screen away under the moving ghost.
  center.style.transition = "opacity 0.28s ease";
  center.style.opacity = "0";

  const dx = end.left - start.left;
  const dy = end.top - start.top;
  const anim = ghost.animate(
    [
      { transform: "translate(0px, 0px)", width: `${start.width}px`, height: `${start.height}px`, opacity: 1 },
      { transform: `translate(${dx}px, ${dy}px)`, width: `${end.width}px`, height: `${end.height}px`, opacity: 0.35 },
    ],
    { duration: 340, easing: "cubic-bezier(.22,.8,.26,1)", fill: "forwards" }
  );
  try {
    await anim.finished;
  } catch {
    /* aborted */
  }
  ghost.remove();
  center.classList.add("hidden");
  center.style.opacity = "";
  center.style.transition = "";
  composer.style.opacity = "";
}

async function sendFromCenter() {
  const text = $("center-input").value.trim();
  if (!text) return;
  const wsId = $("center-project").value;
  const ws = state.workspaces.find((w) => w.id === wsId);
  if (!ws) {
    alert("Pick a project — or browse for a new one — first.");
    return;
  }
  const provider = preferredProvider();
  if (!provider) {
    alert("No enabled models — enable some in Settings first.");
    return;
  }
  const id = await api.createConversation(ws.id, "New Conversation", provider.id, firstEnabledModel(provider));
  state.conversations.set(ws.id, await api.listConversations(ws.id));
  renderSidebar();
  const conv = state.conversations.get(ws.id).find((c) => c.id === id);
  if (!conv) return;
  $("center-input").value = "";
  // Swap screens under the morph, then let the composer take focus.
  await transitionCenterToComposer();
  await selectConversation(conv);
  $("input").value = text;
  $("input").focus();
  await send();
}

export async function refreshProviders() {
  state.providers = await api.listProviders();
  renderModelPicker();
}

function relTime(ts) {
  const diff = Date.now() / 1000 - ts;
  if (diff < 60) return `${Math.round(diff)}s`;
  if (diff < 3600) return `${Math.round(diff / 60)}m`;
  if (diff < 86400) return `${Math.round(diff / 3600)}h`;
  if (diff < 86400 * 30) return `${Math.round(diff / 86400)}d`;
  return `${Math.round(diff / (86400 * 30))}mo`;
}

const CONV_CAP = 5;

export function renderSidebar() {
  const list = $("workspace-list");
  list.innerHTML = "";
  for (const ws of state.workspaces) {
    const wsEl = document.createElement("div");
    wsEl.className = "workspace";
    const header = document.createElement("div");
    header.className = "workspace-header";
    header.innerHTML = `${icon("folder", 13)}<span class="ws-name"></span>`;
    header.querySelector(".ws-name").textContent = ws.name;
    header.title = ws.path;
    wsEl.appendChild(header);
    const convs = state.conversations.get(ws.id) || [];
    const showAll = state.showAll.has(ws.id);
    const visible = showAll ? convs : convs.slice(0, CONV_CAP);
    for (const conv of visible) {
      const el = document.createElement("div");
      el.className = "conversation" + (state.active?.id === conv.id ? " active" : "");
      const title = document.createElement("span");
      title.className = "conv-title";
      title.textContent = conv.title;
      el.appendChild(title);
      const time = document.createElement("span");
      time.className = "conv-time dim";
      time.textContent = relTime(conv.updated_at);
      el.appendChild(time);
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
          renderCenterScreen();
        }
        renderSidebar();
      });
      el.appendChild(del);
      el.onclick = guard(() => selectConversation(conv));
      wsEl.appendChild(el);
    }
    if (convs.length > CONV_CAP) {
      const more = document.createElement("div");
      more.className = "conversation see-all dim";
      more.textContent = showAll ? "Show less" : `See all (${convs.length})`;
      more.onclick = () => {
        if (showAll) state.showAll.delete(ws.id);
        else state.showAll.add(ws.id);
        renderSidebar();
      };
      wsEl.appendChild(more);
    }
    list.appendChild(wsEl);
  }
}

function workspaceName(id) {
  return state.workspaces.find((w) => w.id === id)?.name ?? "";
}

export async function selectConversation(conv) {
  state.active = conv;
  state.lastWorkspaceId = conv.workspace_id;
  $("center-screen").classList.add("hidden");
  const msgEl = $("messages");
  msgEl.classList.remove("hidden");
  renderSidebar(); // instant feedback, before the fetch
  $("chat-title").textContent = conv.title;
  $("chat-ws").innerHTML = `${icon("folder", 13)}<span></span>`;
  $("chat-ws").querySelector("span").textContent = workspaceName(conv.workspace_id);
  $("composer").classList.remove("hidden");
  renderModelPicker();
  renderModeToggle(conv.approval_mode);
  syncSendStop();
  const msgs = await api.getMessages(conv.id);
  // A newer click may have switched away while the fetch was in flight.
  if (state.active?.id !== conv.id) return;
  // Fade the pane swap instead of a hard cut.
  msgEl.style.transition = "none";
  msgEl.style.opacity = "0";
  renderMessages(msgs);
  requestAnimationFrame(() => {
    msgEl.style.transition = "opacity 0.18s ease";
    msgEl.style.opacity = "1";
  });
  resumeLiveState(conv.id);
  api.setUiState(conv.workspace_id, conv.id).catch(() => {});
  $("input").focus();
}

export function renderModeToggle(mode) {
  $("mode-toggle").textContent = mode === "auto" ? "Auto" : "Manual";
}

export function renderModelPicker() {
  const slot = $("model-slot");
  slot.innerHTML = "";
  if (!state.active) return;
  const conv = state.active;
  const groups = [];
  for (const p of state.providers) {
    const enabled = p.models.filter((m) => !(p.disabled_models ?? []).includes(m));
    if (!enabled.length) continue;
    groups.push({
      label: p.label,
      options: enabled.map((m) => ({
        value: `${p.id}/${m}`,
        label: m,
        current: p.id === conv.provider_id && m === conv.model,
      })),
    });
  }
  const current = state.providers.find((p) => p.id === conv.provider_id);
  const currentLabel = current ? `${current.label} · ${conv.model}` : conv.model;
  const anyEnabled = groups.length > 0;
  const dd = makeDropdown({
    value: currentLabel,
    groups,
    emptyNote: anyEnabled ? "" : "All models off — enable some in ⚙ Settings",
    onSelect: guard(async (v) => {
      const [providerId, ...rest] = v.split("/");
      const model = rest.join("/");
      await api.updateConversationModel(conv.id, providerId, model);
      conv.provider_id = providerId;
      conv.model = model;
      const p = state.providers.find((x) => x.id === providerId);
      dd.setValue(p ? `${p.label} · ${model}` : model);
    }),
  });
  slot.appendChild(dd.el);
}

$("new-conversation").onclick = () => renderCenterScreen();

$("center-send").onclick = guard(sendFromCenter);
$("center-input").addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey) {
    e.preventDefault();
    guard(sendFromCenter)();
  }
});

$("center-project").onchange = guard(async () => {
  const sel = $("center-project");
  if (sel.value !== "__new__") return;
  const path = await api.pickFolder();
  if (!path) {
    populateCenterProject();
    return;
  }
  const name = path.split(/[\\/]/).filter(Boolean).pop() || "project";
  const id = await api.addWorkspace(name, path);
  state.workspaces = await api.listWorkspaces();
  if (!state.conversations.has(id)) state.conversations.set(id, await api.listConversations(id));
  renderSidebar();
  populateCenterProject();
  sel.value = id;
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

// Copy buttons on code blocks (event delegation — buttons are inside .md).
document.addEventListener("click", (e) => {
  const btn = e.target.closest(".code-copy");
  if (!btn) return;
  const code = btn.parentElement.querySelector("code");
  if (code) {
    navigator.clipboard.writeText(code.textContent).then(() => {
      btn.textContent = "✓";
      btn.classList.add("copied");
      setTimeout(() => {
        btn.textContent = "⧉";
        btn.classList.remove("copied");
      }, 1200);
    });
  }
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
  if (changed) {
    renderSidebar();
    syncSendStop();
  }
  handleAgentEvent(payload);
});

$("input").addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey) {
    e.preventDefault();
    send();
  }
});

// Textarea autoresize + send/stop button state sync.
function bindAutoresize(el, max = 160) {
  const fit = () => {
    el.style.height = "auto";
    el.style.height = Math.min(el.scrollHeight, max) + "px";
  };
  el.addEventListener("input", fit);
  fit();
}
bindAutoresize($("input"));
bindAutoresize($("center-input"));

/// Send becomes Stop while the active conversation's agent runs.
export function syncSendStop() {
  const running = state.active && state.running.has(state.active.id);
  for (const btn of [$("send"), $("center-send")]) {
    btn.innerHTML = running ? icon("square", 11) : icon("send", 14);
    btn.classList.toggle("is-stop", !!running);
    btn.title = running ? "Stop" : "Send";
  }
  $("send").disabled = !running && !$("input").value.trim();
}
$("input").addEventListener("input", syncSendStop);
syncSendStop();

$("send").onclick = () => {
  if (state.active && state.running.has(state.active.id)) {
    api.cancelAgent(state.active.id).catch(() => {});
  } else {
    send();
  }
};

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
  syncSendStop();
  renderSidebar();
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
    syncSendStop();
    renderSidebar();
    const err = document.createElement("div");
    err.className = "bubble error";
    err.textContent = `Error: ${e}`;
    document.getElementById("messages").appendChild(err);
  }
}

