import { api } from "./api.js";
import { state, renderSidebar, renderCenterScreen, guard, confirmDialog } from "./app.js";
import { makeDropdown } from "./dropdown.js";

const $ = (id) => document.getElementById(id);

// Captured at init - renderSettings is top-level but needs to refresh providers.
let refreshProvidersFn = async () => {};
let selectedProviderId = null;

/// Persist a provider from its row's current DOM state (auto-save).
/// Does NOT re-render settings (keeps scroll/focus); just refreshes the
/// composer picker and flashes "saved ✓" on the row.
async function saveProvider(p, row) {
  const checks = [...row.querySelectorAll(".models-wrap input[type=checkbox]")];
  p.base_url = row.querySelector(".p-base").value.trim() || null;
  p.models = checks.map((c) => c.dataset.model);
  p.disabled_models = checks.filter((c) => !c.checked).map((c) => c.dataset.model);
  await api.upsertProvider(p);
  await refreshProvidersFn();
  let flash = row.querySelector(".saved-flash");
  if (!flash) {
    flash = document.createElement("span");
    flash.className = "saved-flash";
    flash.textContent = "saved ✓";
    row.querySelector(".provider-head").appendChild(flash);
  }
  flash.classList.add("show");
  clearTimeout(flash._t);
  flash._t = setTimeout(() => flash.classList.remove("show"), 1200);
}

// Right-click context menu on model chips (Delete model).
let chipMenu = null;
function closeChipMenu() {
  chipMenu?.remove();
  chipMenu = null;
}
document.addEventListener("click", closeChipMenu);
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") closeChipMenu();
});

function showChipMenu(x, y, onDelete) {
  closeChipMenu();
  const menu = document.createElement("div");
  menu.className = "ctx-menu";
  const item = document.createElement("button");
  item.type = "button";
  item.className = "dropdown-item";
  item.textContent = "Delete model";
  item.onclick = guard(async () => {
    closeChipMenu();
    await onDelete();
  });
  menu.appendChild(item);
  menu.style.left = `${x}px`;
  menu.style.top = `${y}px`;
  document.body.appendChild(menu);
  chipMenu = menu;
}

/// Small dropdown for AG-style settings rows (control on the right side).
function makePrefDropdown(host, { value, options, groupLabel = "Options", onPick }) {
  const dd = makeDropdown({
    value,
    groups: [
      { label: groupLabel, options: options.map((o) => ({ value: o, label: o, current: o === value })) },
    ],
    onSelect: (v) => {
      dd.setValue(v);
      onPick(v);
    },
  });
  host.innerHTML = "";
  host.appendChild(dd.el);
  dd.el.classList.add("down");
}

export function initSettings(_state, refreshProviders) {
  refreshProvidersFn = refreshProviders;
  $("open-settings").onclick = () => {
    renderSettings();
    renderWorkspaces();
    $("settings").classList.remove("hidden");
  };
  $("close-settings").onclick = () => $("settings").classList.add("hidden");

  for (const btn of document.querySelectorAll(".settings-nav-item")) {
    btn.onclick = () => {
      document.querySelectorAll(".settings-nav-item").forEach((b) => b.classList.toggle("active", b === btn));
      for (const sec of ["providers", "agent", "permissions", "workspaces"]) {
        $(`section-${sec}`).classList.toggle("hidden", btn.dataset.section !== sec);
      }
    };
  }

  // Prefs: AG-style rows with a dropdown on the right.
  makePrefDropdown($("pref-approval-slot"), {
    value: state.prefs.defaultApprovalMode === "auto" ? "Auto" : "Manual",
    options: ["Manual", "Auto"],
    onPick: guard(async (label) => {
      const mode = label.toLowerCase();
      state.prefs.defaultApprovalMode = mode;
      await api.setAppPrefs({ defaultApprovalMode: mode });
    }),
  });
  makePrefDropdown($("pref-notif-slot"), {
    value: state.prefs.notifications ? "On" : "Off",
    options: ["On", "Off"],
    onPick: guard(async (label) => {
      state.prefs.notifications = label === "On";
      await api.setAppPrefs({ notificationsEnabled: state.prefs.notifications });
    }),
  });
  const ASK_ALLOW = ["Ask every time", "Allow without asking"];
  makePrefDropdown($("pref-files-slot"), {
    value: state.prefs.projectFilesNoAsk ? ASK_ALLOW[1] : ASK_ALLOW[0],
    options: ASK_ALLOW,
    groupLabel: "File writes",
    onPick: guard(async (label) => {
      state.prefs.projectFilesNoAsk = label === ASK_ALLOW[1];
      await api.setAppPrefs({ projectFilesNoAsk: state.prefs.projectFilesNoAsk });
    }),
  });
  makePrefDropdown($("pref-shell-slot"), {
    value: state.prefs.projectShellNoAsk ? ASK_ALLOW[1] : ASK_ALLOW[0],
    options: ASK_ALLOW,
    groupLabel: "Shell commands",
    onPick: guard(async (label) => {
      state.prefs.projectShellNoAsk = label === ASK_ALLOW[1];
      await api.setAppPrefs({ projectShellNoAsk: state.prefs.projectShellNoAsk });
    }),
  });
  const EXT_LABELS = { ask: "Always ask", allow: "Allow without asking", block: "Block external tools" };
  const EXT_VALUES = Object.fromEntries(Object.entries(EXT_LABELS).map(([k, v]) => [v, k]));
  makePrefDropdown($("pref-external-slot"), {
    value: EXT_LABELS[state.prefs.externalPolicy] ?? EXT_LABELS.ask,
    options: Object.values(EXT_LABELS),
    groupLabel: "External policy",
    onPick: guard(async (label) => {
      state.prefs.externalPolicy = EXT_VALUES[label] ?? "ask";
      await api.setAppPrefs({ externalPolicy: state.prefs.externalPolicy });
    }),
  });
  makePrefDropdown($("pref-workshop-slot"), {
    value: state.prefs.workshopFullAccess ? "Full access" : "Project rules",
    options: ["Full access", "Project rules"],
    groupLabel: "Workshop",
    onPick: guard(async (label) => {
      state.prefs.workshopFullAccess = label === "Full access";
      await api.setAppPrefs({ workshopFullAccess: state.prefs.workshopFullAccess });
    }),
  });

  $("custom-provider-form").onsubmit = guard(async (e) => {
    e.preventDefault();
    const label = $("cp-label").value.trim();
    const baseUrl = $("cp-base-url").value.trim();
    const models = $("cp-models").value.split(",").map((m) => m.trim()).filter(Boolean);
    const key = $("cp-key").value.trim();
    const id = label.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || `custom-${Date.now()}`;
    if (state.providers.some((p) => p.id === id)) {
      // Upsert would silently clobber the existing row (including has_key).
      alert(`A provider with id "${id}" already exists - edit it above instead.`);
      return;
    }
    await api.upsertProvider({
      id,
      label,
      kind: "open_ai_compatible",
      base_url: baseUrl,
      has_key: false,
      models,
      disabled_models: [...models], // new providers start fully off
      extra_headers: [],
    });
    if (key) await api.setApiKey(id, key);
    e.target.reset();
    await refreshProvidersFn();
    // Open the freshly added provider straight away.
    selectedProviderId = id;
    renderSettings();
  });

  $("ws-browse").onclick = guard(async () => {
    const path = await api.pickFolder();
    if (!path) return;
    $("ws-path").value = path;
    if (!$("ws-name").value.trim()) {
      $("ws-name").value = path.split(/[\\/]/).filter(Boolean).pop() || "project";
    }
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
  const slot = $("provider-picker-slot");
  slot.innerHTML = "";
  const newForm = $("provider-new");
  if (!state.providers.length) {
    $("provider-detail").innerHTML = "";
    newForm.classList.remove("hidden");
    return;
  }
  if (!state.providers.some((p) => p.id === selectedProviderId)) {
    selectedProviderId = state.providers[0].id;
  }
  const dd = makeDropdown({
    value: state.providers.find((p) => p.id === selectedProviderId)?.label ?? "Select",
    groups: [
      {
        label: "Providers",
        options: [
          ...state.providers.map((p) => ({
            value: p.id,
            label: p.label,
            current: p.id === selectedProviderId,
          })),
          { value: "__new__", label: "New provider…", icon: "plus", dim: true },
        ],
      },
    ],
    onSelect: (v) => {
      if (v === "__new__") {
        $("provider-detail").innerHTML = "";
        newForm.classList.remove("hidden");
        dd.setValue(state.providers.find((p) => p.id === selectedProviderId)?.label ?? "Select");
        return;
      }
      selectedProviderId = v;
      newForm.classList.add("hidden");
      dd.setValue(state.providers.find((p) => p.id === v)?.label ?? v);
      renderProviderDetail();
    },
  });
  slot.appendChild(dd.el);
  dd.el.classList.add("down"); // top of the settings page - popup opens downward
  renderProviderDetail();
}

/// One provider's config panel - models, base URL, API key, danger zone.
function renderProviderDetail() {
  const p = state.providers.find((x) => x.id === selectedProviderId);
  const host = $("provider-detail");
  host.innerHTML = "";
  if (!p) return;
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

  // Models editor: checkbox per model (checked = enabled in the picker).
  const modelsWrap = document.createElement("div");
  modelsWrap.className = "models-wrap";
  const modelsHead = document.createElement("div");
  modelsHead.className = "dim models-head";
  modelsHead.textContent = "Models (checked = shown in picker)";
  const modelsGrid = document.createElement("div");
  modelsGrid.className = "models-grid";
  modelsWrap.append(modelsHead, modelsGrid);
  const disabled = new Set(p.disabled_models ?? []);
  const addModelRow = (name) => {
    const mrow = document.createElement("label");
    mrow.className = "model-check";
    const cb = document.createElement("input");
    cb.type = "checkbox";
    cb.checked = !disabled.has(name);
    cb.dataset.model = name;
    const txt = document.createElement("span");
    txt.textContent = name;
    mrow.append(cb, txt);
    mrow.title = "Toggle in picker - right-click to delete";
    mrow.oncontextmenu = (e) => {
      e.preventDefault();
      e.stopPropagation();
      showChipMenu(e.clientX, e.clientY, async () => {
        p.models = p.models.filter((m) => m !== name);
        mrow.remove();
        await saveProvider(p, row);
      });
    };
    modelsGrid.appendChild(mrow);
    return mrow;
  };
  for (const m of p.models) addModelRow(m);
  const addRow = document.createElement("div");
  addRow.className = "model-add";
  const addInput = document.createElement("input");
  addInput.placeholder = "add model…";
  const addBtn = document.createElement("button");
  addBtn.type = "button";
  addBtn.textContent = "Add";
  addBtn.onclick = () => {
    const name = addInput.value.trim();
    if (!name) return;
    if ([...modelsWrap.querySelectorAll("input[type=checkbox]")].some((c) => c.dataset.model === name)) return;
    addModelRow(name);
    addInput.value = "";
  };
  addRow.append(addInput, addBtn);
  if (p.kind === "ollama") {
    const fetchBtn = document.createElement("button");
    fetchBtn.type = "button";
    fetchBtn.textContent = "Fetch";
    fetchBtn.title = "Fetch models from the Ollama server";
    fetchBtn.onclick = guard(async () => {
      const models = await api.listLocalModels(p.id);
      if (models.length === 0) {
        alert("No models on the Ollama server - pull one first (ollama pull …).");
        return;
      }
      for (const m of models) {
        if (![...modelsWrap.querySelectorAll("input[type=checkbox]")].some((c) => c.dataset.model === m)) {
          addModelRow(m);
        }
      }
      await saveProvider(p, row);
    });
    addRow.appendChild(fetchBtn);
  }
  modelsWrap.appendChild(addRow);

  const baseLabel = document.createElement("label");
  baseLabel.textContent = "Base URL ";
  const baseInput = document.createElement("input");
  baseInput.className = "p-base";
  baseInput.value = p.base_url ?? "";
  baseInput.placeholder = "(default)";
  baseLabel.appendChild(baseInput);

  // API key: inline field, always visible.
  const keyWrap = document.createElement("div");
  keyWrap.className = "key-row";
  const keyInput = document.createElement("input");
  keyInput.type = "password";
  keyInput.placeholder = p.has_key ? "Replace API key" : "API key";
  const keySave = document.createElement("button");
  keySave.textContent = "Save key";
  keySave.onclick = guard(async () => {
    const key = keyInput.value.trim();
    if (!key) return;
    await api.setApiKey(p.id, key);
    p.has_key = true;
    renderProviderDetail();
  });
  keyWrap.append(keyInput, keySave);
  if (p.has_key) {
    const keyDel = document.createElement("button");
    keyDel.textContent = "Delete key";
    keyDel.onclick = guard(async () => {
      await api.deleteApiKey(p.id);
      p.has_key = false;
      renderProviderDetail();
    });
    keyWrap.appendChild(keyDel);
  }

  const actions = document.createElement("div");
  actions.className = "provider-actions";
  const del = document.createElement("button");
  del.className = "p-delete";
  del.textContent = "Delete provider";
  del.onclick = guard(async () => {
    if (confirm(`Delete provider ${p.label}?`)) {
      await api.deleteProvider(p.id);
      selectedProviderId = null;
      await refreshProvidersFn();
      renderSettings();
    }
  });
  actions.appendChild(del);

  // Auto-save on any change.
  modelsWrap.addEventListener("change", (e) => {
    if (e.target.matches("input[type=checkbox]")) guard(() => saveProvider(p, row))();
  });
  baseInput.addEventListener("change", () => guard(() => saveProvider(p, row))());

  row.append(head, modelsWrap, baseLabel, keyWrap, actions);
  host.appendChild(row);
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
      if (!(await confirmDialog("HEY! You sure you want to delete this project? Its chats will go too!"))) return;
      await api.removeWorkspace(ws.id);
      state.workspaces = await api.listWorkspaces();
      state.conversations.delete(ws.id);
      if (state.active?.workspace_id === ws.id) {
        renderCenterScreen();
      }
      renderSidebar();
      renderWorkspaces();
    });
    label.append(strong, " ", path, " ", btn);
    row.appendChild(label);
    list.appendChild(row);
  }
}
