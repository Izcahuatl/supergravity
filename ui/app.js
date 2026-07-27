import { api } from "./api.js";
import { renderMessages, renderPlanCard, parsePlanSteps } from "./render.js";
import { initSettings } from "./settings.js";
import { handleAgentEvent, resetEventState, resumeLiveState } from "./events.js";
import { initReview } from "./diffview.js";
import { icon } from "./icons.js";
import { makeDropdown } from "./dropdown.js";
import { attachMentions } from "./mentions.js";

export const state = {
  workspaces: [],
  providers: [],
  conversations: new Map(), // workspaceId -> ConversationRow[]
  active: null, // active ConversationRow
  running: new Set(), // conversation_ids with a live agent
  runStarted: new Map(), // conversation_id -> Date.now() when the run began
  showAll: new Set(), // workspace_ids with expanded conversation lists
  lastWorkspaceId: null, // last active workspace (default for new conversations)
  centerWorkspaceId: null, // workspace picked in the center-screen project dropdown
  prefs: {
    defaultApprovalMode: "manual",
    notifications: true,
    externalPolicy: "ask",
    workshopPythonNoAsk: true,
  }, // app config (Agent + Permissions settings)
  activePlan: null, // latest update_plan steps for the active conversation
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
  // App preferences (Agent section in Settings).
  state.prefs = {
    defaultApprovalMode: initial.config.default_approval_mode ?? "manual",
    notifications: initial.config.notifications_enabled ?? true,
    externalPolicy: initial.config.external_policy ?? "ask",
    workshopPythonNoAsk: initial.config.workshop_python_no_ask ?? true,
  };
  // Static icon buttons.
  $("ws-add").innerHTML = icon("plus", 13);
  $("center-attach").innerHTML = icon("plus", 14);
  $("ws-add").onclick = guard(async () => {
    await addWorkspaceFromPicker();
  });
  $("center-attach").onclick = () => {
    const ta = $("center-input");
    const pos = ta.selectionStart ?? ta.value.length;
    ta.value = `${ta.value.slice(0, pos)}@${ta.value.slice(pos)}`;
    ta.setSelectionRange(pos + 1, pos + 1);
    ta.dispatchEvent(new Event("input")); // opens the mention popup
    ta.focus();
  };
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
  renderMessages(msgs, conv.id);
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
  state.activePlan = null;
  renderTaskButton();
  $("center-error").classList.add("hidden");
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

let centerDropdown = null;

function populateCenterProject() {
  const slot = $("center-project-slot");
  slot.innerHTML = "";
  const last = state.workspaces.find((w) => w.id === state.lastWorkspaceId) ?? state.workspaces[0];
  if (last) state.centerWorkspaceId = last.id;
  centerDropdown = makeDropdown({
    value: last ? last.name : "No projects",
    valueIcon: "folder",
    groups: [
      {
        label: "Projects",
        options: [
          ...state.workspaces.map((ws) => ({
            value: ws.id,
            label: ws.name,
            icon: "folder",
            current: ws.id === state.centerWorkspaceId,
          })),
          { value: "__new__", label: "New project (browse)…", icon: "plus", dim: true },
        ],
      },
    ],
    onSelect: guard(async (v) => {
      if (v === "__new__") {
        if (await addWorkspaceFromPicker()) populateCenterProject();
        return;
      }
      state.centerWorkspaceId = v;
      const ws = state.workspaces.find((w) => w.id === v);
      if (ws) centerDropdown.setValue(ws.name, "folder");
    }),
  });
  slot.appendChild(centerDropdown.el);
  centerDropdown.el.classList.add("down"); // popup opens downward here (mid-screen)
  populateCenterModel();
}

/// Pick a workspace folder via the OS dialog and register it (shared by the
/// center dropdown's "New project" and the sidebar's Projects + button).
async function addWorkspaceFromPicker() {
  const path = await api.pickFolder();
  if (!path) return null;
  const name = path.split(/[\\/]/).filter(Boolean).pop() || "project";
  const id = await api.addWorkspace(name, path);
  state.workspaces = await api.listWorkspaces();
  if (!state.conversations.has(id)) state.conversations.set(id, await api.listConversations(id));
  state.lastWorkspaceId = id;
  renderSidebar();
  return id;
}

// Model chosen in the center composer: { providerId, model } or null (auto).
let centerModel = null;
let centerModelDropdown = null;

function currentCenterModel() {
  if (centerModel && state.providers.some((p) => p.id === centerModel.providerId && firstEnabledModel(p) === centerModel.model)) {
    return centerModel;
  }
  const p = preferredProvider();
  return p ? { providerId: p.id, model: firstEnabledModel(p) } : null;
}

function populateCenterModel() {
  const slot = $("center-model-slot");
  slot.innerHTML = "";
  const groups = [];
  for (const p of state.providers) {
    const enabled = p.models.filter((m) => !(p.disabled_models ?? []).includes(m));
    if (!enabled.length) continue;
    groups.push({
      label: p.label,
      options: enabled.map((m) => ({
        value: `${p.id}/${m}`,
        label: m,
        current: currentCenterModel()?.providerId === p.id && currentCenterModel()?.model === m,
      })),
    });
  }
  const cur = currentCenterModel();
  const curProvider = cur && state.providers.find((p) => p.id === cur.providerId);
  centerModelDropdown = makeDropdown({
    value: curProvider ? `${curProvider.label} · ${cur.model}` : "no enabled models",
    groups,
    emptyNote: groups.length ? "" : "All models off — enable some in ⚙ Settings",
    onSelect: (v) => {
      const [providerId, ...rest] = v.split("/");
      const model = rest.join("/");
      centerModel = { providerId, model };
      const p = state.providers.find((x) => x.id === providerId);
      centerModelDropdown.setValue(p ? `${p.label} · ${model}` : model);
    },
  });
  slot.appendChild(centerModelDropdown.el);
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
  const ws = state.workspaces.find((w) => w.id === state.centerWorkspaceId);
  if (!ws) {
    // In-app nudge instead of a native alert dialog.
    const err = $("center-error");
    err.classList.remove("hidden");
    err.style.animation = "none";
    requestAnimationFrame(() => {
      err.style.animation = "";
    });
    clearTimeout(err._t);
    err._t = setTimeout(() => err.classList.add("hidden"), 4000);
    return;
  }
  const provider = currentCenterModel();
  if (!provider) {
    alert("No enabled models — enable some in Settings first.");
    return;
  }
  const id = await api.createConversation(ws.id, "New Conversation", provider.providerId, provider.model);
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

function elapsed(ms) {
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return `${s}s`;
  return `${Math.floor(s / 60)}m${String(s % 60).padStart(2, "0")}s`;
}

// Tick the live run timers in place — no full sidebar re-render per second.
setInterval(() => {
  for (const t of document.querySelectorAll(".conv-time[data-runstart]")) {
    t.textContent = elapsed(Date.now() - Number(t.dataset.runstart));
  }
}, 1000);

let convSearchQuery = "";

export function renderSidebar() {
  const list = $("workspace-list");
  list.innerHTML = "";
  const q = convSearchQuery.trim().toLowerCase();
  if (q) {
    // Flat search results across every workspace.
    let any = false;
    for (const ws of state.workspaces) {
      for (const conv of state.conversations.get(ws.id) || []) {
        if (!conv.title.toLowerCase().includes(q)) continue;
        any = true;
        const el = document.createElement("div");
        el.className = "conversation" + (state.active?.id === conv.id ? " active" : "");
        const title = document.createElement("span");
        title.className = "conv-title";
        title.textContent = conv.title;
        const tag = document.createElement("span");
        tag.className = "conv-time dim";
        tag.textContent = ws.name;
        el.append(title, tag);
        el.onclick = guard(() => selectConversation(conv));
        list.appendChild(el);
      }
    }
    if (!any) {
      const none = document.createElement("div");
      none.className = "dim search-empty";
      none.textContent = "No matching conversations";
      list.appendChild(none);
    }
    return;
  }
  for (const ws of state.workspaces) {
    const wsEl = document.createElement("div");
    wsEl.className = "workspace";
    const header = document.createElement("div");
    header.className = "workspace-header";
    header.innerHTML = `${icon("folder", 13)}<span class="ws-name"></span>`;
    header.querySelector(".ws-name").textContent = ws.name;
    header.title = ws.path;
    const rm = document.createElement("button");
    rm.className = "ws-remove";
    rm.textContent = "✕";
    rm.title = "Remove workspace (files on disk are kept)";
    rm.onclick = guard(async (e) => {
      e.stopPropagation();
      if (!confirm(`Remove workspace "${ws.name}" and all its conversations?\nFiles on disk are kept.`)) return;
      const wasActive = state.active?.workspace_id === ws.id;
      await api.removeWorkspace(ws.id);
      state.workspaces = state.workspaces.filter((w) => w.id !== ws.id);
      state.conversations.delete(ws.id);
      if (wasActive) renderCenterScreen();
      renderSidebar();
    });
    header.appendChild(rm);
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
      title.title = "Double-click to rename";
      title.ondblclick = (e) => {
        e.stopPropagation();
        const input = document.createElement("input");
        input.className = "conv-rename";
        input.value = conv.title;
        title.replaceWith(input);
        input.focus();
        input.select();
        const save = guard(async () => {
          const t = input.value.trim();
          if (t && t !== conv.title) {
            await api.renameConversation(conv.id, t);
            conv.title = t;
            if (state.active?.id === conv.id) $("chat-title").textContent = t;
          }
          renderSidebar();
        });
        input.onkeydown = (ev) => {
          ev.stopPropagation();
          if (ev.key === "Enter") save();
          if (ev.key === "Escape") renderSidebar();
        };
        input.onclick = (ev) => ev.stopPropagation();
        input.onblur = save;
      };
      el.appendChild(title);
      const time = document.createElement("span");
      time.className = "conv-time dim";
      if (state.running.has(conv.id)) {
        // Running rows show a live elapsed timer instead of the rel-time.
        const start = state.runStarted.get(conv.id) ?? Date.now();
        time.dataset.runstart = start;
        time.textContent = elapsed(Date.now() - start);
      } else {
        time.textContent = relTime(conv.updated_at);
      }
      el.appendChild(time);
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
    if (convs.length === 0) {
      const add = document.createElement("div");
      add.className = "conversation conv-new dim";
      add.innerHTML = `${icon("plus", 12)}<span class="conv-title">New</span>`;
      add.title = "Start a conversation in this project";
      add.onclick = () => {
        state.lastWorkspaceId = ws.id;
        renderCenterScreen();
      };
      wsEl.appendChild(add);
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
  $("chat-ws").innerHTML = `${icon("folder", 13)}<span></span><span class="dim">/</span>`;
  $("chat-ws").querySelector("span").textContent = workspaceName(conv.workspace_id);
  $("composer").classList.remove("hidden");
  renderModelPicker();
  renderModeToggle(conv.approval_mode);
  syncSendStop();
  const msgs = await api.getMessages(conv.id);
  // A newer click may have switched away while the fetch was in flight.
  if (state.active?.id !== conv.id) return;
  // Restore the latest plan for the header indicator.
  let planSteps = null;
  for (const m of msgs) {
    if (m.role !== "assistant") continue;
    for (const p of m.parts) {
      if (p.type === "tool_call" && p.name === "update_plan") planSteps = parsePlanSteps(p.args_json);
    }
  }
  state.activePlan = planSteps;
  renderTaskButton();
  // Fade the pane swap instead of a hard cut.
  msgEl.style.transition = "none";
  msgEl.style.opacity = "0";
  renderMessages(msgs, conv.id);
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

/// Header "Active Task" indicator: progress count, click opens the checklist.
export function renderTaskButton() {
  const btn = $("task-btn");
  const pop = $("task-popover");
  const steps = state.activePlan;
  if (!steps || !steps.length) {
    btn.classList.add("hidden");
    pop.classList.add("hidden");
    return;
  }
  const done = steps.filter((s) => s.status === "done").length;
  btn.innerHTML = `${icon("list", 13)}<span>Task ${done}/${steps.length}</span>`;
  btn.classList.remove("hidden");
  if (!pop.classList.contains("hidden")) {
    pop.innerHTML = "";
    pop.appendChild(renderPlanCard(steps));
  }
}

$("task-btn").onclick = (e) => {
  e.stopPropagation();
  const pop = $("task-popover");
  if (!pop.classList.contains("hidden")) {
    pop.classList.add("hidden");
    return;
  }
  pop.innerHTML = "";
  pop.appendChild(renderPlanCard(state.activePlan ?? []));
  pop.classList.remove("hidden");
};
document.addEventListener("click", (e) => {
  const pop = $("task-popover");
  if (!pop.classList.contains("hidden") && !pop.contains(e.target) && e.target !== $("task-btn")) {
    pop.classList.add("hidden");
  }
});

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

// @ file mentions in both composers — registered BEFORE the Enter-to-send
// handlers so an open popup can intercept Enter/Tab/Escape first.
attachMentions($("input"), () => state.active?.workspace_id);
attachMentions($("center-input"), () => state.centerWorkspaceId);

// --- slash commands: "/auto /manual /model /new" at the start of the input ---
const SLASH_COMMANDS = [
  {
    name: "auto",
    desc: "Approvals: Auto — no per-write prompts",
    run: async () => {
      if (!state.active) return;
      await api.setApprovalMode(state.active.id, "auto");
      state.active.approval_mode = "auto";
      renderModeToggle("auto");
    },
  },
  {
    name: "manual",
    desc: "Approvals: Manual — approve every write",
    run: async () => {
      if (!state.active) return;
      await api.setApprovalMode(state.active.id, "manual");
      state.active.approval_mode = "manual";
      renderModeToggle("manual");
    },
  },
  {
    name: "model",
    desc: "Open the model picker",
    run: async () => {
      document.querySelector("#model-slot .dropdown-trigger")?.click();
    },
  },
  { name: "new", desc: "Start a new conversation", run: async () => renderCenterScreen() },
];

function attachSlash(textarea) {
  const popup = document.createElement("div");
  popup.className = "mention-popup hidden"; // same look as the @ popup
  document.body.appendChild(popup);
  let matches = [];
  let activeIdx = 0;

  const close = () => {
    popup.classList.add("hidden");
    matches = [];
  };
  const render = () => {
    popup.innerHTML = "";
    matches.forEach((c, i) => {
      const it = document.createElement("button");
      it.type = "button";
      it.className = "dropdown-item" + (i === activeIdx ? " current" : "");
      it.innerHTML = `<span class="slash-name"></span><span class="dim slash-desc"></span>`;
      it.querySelector(".slash-name").textContent = `/${c.name}`;
      it.querySelector(".slash-desc").textContent = c.desc;
      it.onmousedown = (e) => {
        e.preventDefault();
        guard(exec)(i);
      };
      popup.appendChild(it);
    });
  };
  const exec = async (i) => {
    const cmd = matches[i];
    if (!cmd) return;
    close();
    textarea.value = "";
    textarea.dispatchEvent(new Event("input"));
    await cmd.run();
  };
  textarea.addEventListener("input", () => {
    const upto = textarea.value.slice(0, textarea.selectionStart);
    const m = upto.match(/^\/(\w*)$/);
    if (!m) return close();
    matches = SLASH_COMMANDS.filter((c) => c.name.startsWith(m[1].toLowerCase()));
    if (!matches.length) return close();
    activeIdx = 0;
    const r = textarea.getBoundingClientRect();
    popup.style.left = `${r.left}px`;
    popup.style.width = `${Math.min(420, r.width)}px`;
    popup.style.bottom = `${window.innerHeight - r.top + 6}px`;
    render();
    popup.classList.remove("hidden");
  });
  textarea.addEventListener("keydown", (e) => {
    if (popup.classList.contains("hidden")) return;
    if (e.key === "ArrowDown" || (e.key === "Tab" && !e.shiftKey)) {
      e.preventDefault();
      activeIdx = Math.min(activeIdx + 1, matches.length - 1);
      render();
    } else if (e.key === "ArrowUp" || (e.key === "Tab" && e.shiftKey)) {
      e.preventDefault();
      activeIdx = Math.max(activeIdx - 1, 0);
      render();
    } else if (e.key === "Enter") {
      e.preventDefault();
      e.stopImmediatePropagation();
      guard(exec)(activeIdx);
    } else if (e.key === "Escape") {
      e.stopImmediatePropagation();
      close();
    }
  });
  document.addEventListener("click", (e) => {
    if (!popup.contains(e.target) && e.target !== textarea) close();
  });
}
attachSlash($("input"));

$("new-conversation").onclick = () => renderCenterScreen();

$("conv-search").addEventListener("input", (e) => {
  convSearchQuery = e.target.value;
  renderSidebar();
});

// --- right-click Rewind on user messages ---
let ctxMenu = null;
function closeCtxMenu() {
  ctxMenu?.remove();
  ctxMenu = null;
}
document.addEventListener("click", closeCtxMenu);
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") closeCtxMenu();
});

$("messages").addEventListener("contextmenu", (e) => {
  const userBubble = e.target.closest(".bubble.user");
  const asstBubble = e.target.closest(".bubble.assistant");
  if (!state.active || (!userBubble?.dataset.msgId && !asstBubble)) return;
  e.preventDefault();
  closeCtxMenu();
  const menu = document.createElement("div");
  menu.className = "ctx-menu";
  if (userBubble?.dataset.msgId) {
    const item = document.createElement("button");
    item.type = "button";
    item.className = "dropdown-item";
    item.innerHTML = `${icon("undo", 13)}<span>Rewind to here</span>`;
    item.onclick = guard(async () => {
      closeCtxMenu();
      const convId = state.active.id;
      const text = userBubble.textContent.trim();
      await api.rewindConversation(convId, Number(userBubble.dataset.msgId));
      // The backend cancels a live agent for this conversation — mirror that.
      state.running.delete(convId);
      state.runStarted.delete(convId);
      syncSendStop();
      await refreshMessages();
      renderSidebar();
      const input = $("input");
      input.value = text;
      input.dispatchEvent(new Event("input")); // autoresize + enable Send
      input.focus();
    });
    menu.appendChild(item);
  } else {
    // Assistant bubble: which model answered (stamped at run time; fall back
    // to the conversation's current model for pre-stamp messages).
    const model = asstBubble.dataset.model || state.active.model;
    const pid = asstBubble.dataset.provider || state.active.provider_id;
    const label = state.providers.find((p) => p.id === pid)?.label ?? pid;
    const info = document.createElement("div");
    info.className = "dropdown-item dim ctx-info";
    info.textContent = `Answered by ${label} · ${model}`;
    menu.appendChild(info);
  }
  menu.style.left = `${e.clientX}px`;
  menu.style.top = `${e.clientY}px`;
  document.body.appendChild(menu);
  ctxMenu = menu;
});

$("center-send").onclick = guard(sendFromCenter);
$("center-input").addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey) {
    e.preventDefault();
    guard(sendFromCenter)();
  }
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

// Suppress WebView2's default context menus (page menu AND textbox edit menu)
// everywhere — our own context menus handle their targets.
document.addEventListener("contextmenu", (e) => {
  if (!(e.target instanceof Element)) return;
  e.preventDefault();
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
    if (!state.runStarted.has(cid)) state.runStarted.set(cid, Date.now());
    changed = true;
  }
  if (["message_done", "error", "cancelled"].includes(k) && state.running.has(cid)) {
    state.running.delete(cid);
    state.runStarted.delete(cid);
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
function bindAutoresize(el, { max = 160, min = 56 } = {}) {
  const fit = () => {
    el.style.height = "auto";
    const h = Math.max(min, Math.min(el.scrollHeight + 2, max));
    el.style.height = h + "px";
    el.style.overflowY = el.scrollHeight > max ? "auto" : "hidden";
  };
  el.addEventListener("input", fit);
  fit();
}
bindAutoresize($("input"));
bindAutoresize($("center-input"), { max: 200, min: 76 });

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
  state.runStarted.set(cid, Date.now());
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
    state.runStarted.delete(cid);
    syncSendStop();
    renderSidebar();
    const err = document.createElement("div");
    err.className = "bubble error";
    err.textContent = `Error: ${e}`;
    document.getElementById("messages").appendChild(err);
  }
}

