import { renderMarkdown } from "./markdown.js";
import { makeChange, openReview } from "./diffview.js";
import { icon, TOOL_ICONS } from "./icons.js";

const $ = (id) => document.getElementById(id);

function scrollToBottom() {
  const el = $("messages");
  el.scrollTop = el.scrollHeight;
}

export function addBubble(role) {
  const el = document.createElement("div");
  el.className = `bubble ${role}`;
  $("messages").appendChild(el);
  scrollToBottom();
  return el;
}

export function renderTextPart(container, text) {
  const div = document.createElement("div");
  div.className = "md";
  div.innerHTML = renderMarkdown(text);
  container.appendChild(div);
}

// ---------- attachment chips (from @mention expansion) ----------

const ATTACH_RE = /<attached path="([^"]+)">\n?([\s\S]*?)<\/attached>/g;

function makeAttachChip(path, content) {
  const chip = document.createElement("div");
  chip.className = "attach-chip";
  const head = document.createElement("div");
  head.className = "attach-head";
  head.innerHTML = `${icon("file", 12)}<span></span>`;
  head.querySelector("span").textContent = path;
  const body = document.createElement("pre");
  body.className = "attach-body hidden";
  body.textContent = content;
  head.onclick = () => body.classList.toggle("hidden");
  chip.append(head, body);
  return chip;
}

/// User text with <attached> blocks rendered as collapsible chips.
export function renderUserText(container, text) {
  ATTACH_RE.lastIndex = 0;
  let last = 0;
  let m;
  while ((m = ATTACH_RE.exec(text))) {
    if (m.index > last) renderTextPart(container, text.slice(last, m.index));
    container.appendChild(makeAttachChip(m[1], m[2].replace(/\n$/, "")));
    last = m.index + m[0].length;
  }
  if (last < text.length) renderTextPart(container, text.slice(last));
}

/// History tool row: same compact row as live, with the final status and the
/// result summary folded into the expandable detail (errors expanded).
function renderHistoryToolRow(call, result) {
  const card = renderToolCallCard(call);
  const status = card.querySelector(".tool-status");
  status.textContent = result?.is_error ? "✗" : "✓";
  status.className = "tool-status " + (result?.is_error ? "err" : "ok");
  const detail = card.querySelector(".tool-args");
  if (result && detail) {
    detail.textContent += `\n→ ${result.content.slice(0, 400)}`;
    if (result.is_error) detail.classList.remove("hidden");
  }
  const wrap = addBubble("assistant");
  wrap.classList.add("tool-wrap");
  wrap.appendChild(card);
  return wrap;
}

// ---------- task plan card ----------
const PLAN_STATUS_ICON = { done: "check-circle", in_progress: "play", pending: "circle" };

/// Checklist card for the agent-maintained plan (AG's "Task" artifact).
export function renderPlanCard(steps) {
  const card = document.createElement("div");
  card.className = "plan-card";
  const head = document.createElement("div");
  head.className = "plan-head";
  head.innerHTML = `${icon("list", 13)}<span>Task</span>`;
  card.appendChild(head);
  for (const s of steps) {
    const row = document.createElement("div");
    row.className = `plan-step ${s.status}`;
    row.innerHTML = `${icon(PLAN_STATUS_ICON[s.status] || "circle", 12)}<span></span>`;
    row.querySelector("span").textContent = s.text;
    card.appendChild(row);
  }
  return card;
}

export function parsePlanSteps(argsJson) {
  try {
    const v = JSON.parse(argsJson);
    return Array.isArray(v.steps) ? v.steps : [];
  } catch {
    return [];
  }
}

export function prettyArgs(argsJson) {
  try {
    const v = JSON.parse(argsJson);
    const s = JSON.stringify(v, null, 1);
    return s.length > 300 ? s.slice(0, 300) + "…" : s;
  } catch {
    return argsJson.slice(0, 300);
  }
}

const STEP_VERBS = {
  read_file: (a) => `Read ${a.path ?? "file"}`,
  write_file: (a) => `Wrote ${a.path ?? "file"}`,
  edit_file: (a) => `Edited ${a.path ?? "file"}`,
  list_dir: (a) => `Listed ${a.path ?? "."}`,
  grep: (a) => `Searched ${a.pattern ?? ""}`,
  glob: (a) => `Found files matching ${a.pattern ?? ""}`,
  run_shell: (a) => `Ran ${(a.command ?? "").slice(0, 60)}`,
  list_external_dir: (a) => `Listed external ${a.path ?? "dir"}`,
};

export function toolVerb(name, argsJson) {
  return (STEP_VERBS[name] || (() => name))(parseArgs(argsJson));
}

/// Compact live tool row: icon + verb + right-aligned status. Args/result stay
/// hidden behind a click on the row (approvals force them open).
export function renderToolCallCard(call) {
  const card = document.createElement("div");
  card.className = "tool-card";
  const head = document.createElement("div");
  head.className = "tool-head";
  head.innerHTML = `${icon(TOOL_ICONS[call.name] || "edit", 13)}<span class="tool-verb"></span><span class="tool-status"></span>`;
  head.querySelector(".tool-verb").textContent = toolVerb(call.name, call.args_json);
  const detail = document.createElement("pre");
  detail.className = "tool-args hidden";
  detail.textContent = prettyArgs(call.args_json);
  card.append(head, detail);
  head.onclick = () => detail.classList.toggle("hidden");
  return card;
}

// ---------- Antigravity-style history: runs, worked-for, change cards ----------

function relDuration(secs) {
  if (secs < 1) return null;
  if (secs < 60) return `${Math.round(secs)}s`;
  if (secs < 3600) return `${Math.round(secs / 60)}m`;
  return `${Math.round(secs / 3600)}h`;
}

function lineCount(s) {
  return s === "" ? 0 : s.split("\n").length;
}

function parseArgs(argsJson) {
  try {
    return JSON.parse(argsJson);
  } catch {
    return {};
  }
}

/// Group MessageRow[] into runs: a user message plus everything after it up
/// to the next user message. Tool messages stay attached to their run.
function groupRuns(msgs) {
  const runs = [];
  for (const m of msgs) {
    if (m.role === "system") continue;
    if (m.role === "user") {
      runs.push({ user: m, items: [], end: m.created_at });
    } else if (runs.length) {
      runs[runs.length - 1].items.push(m);
      runs[runs.length - 1].end = m.created_at;
    }
  }
  return runs;
}

function collectCalls(run) {
  // Pair ToolCall parts with their ToolResult parts by tool_call_id.
  const results = new Map();
  for (const m of run.items) {
    if (m.role !== "tool") continue;
    for (const p of m.parts) {
      if (p.type === "tool_result") results.set(p.tool_call_id, p);
    }
  }
  const calls = [];
  for (const m of run.items) {
    if (m.role !== "assistant") continue;
    for (const p of m.parts) {
      if (p.type === "tool_call") {
        calls.push({ ...p, result: results.get(p.id) });
      }
    }
  }
  return calls;
}

function renderWorkedFor(run, calls) {
  const duration = relDuration(run.end - run.user.created_at);
  const wrap = document.createElement("div");
  wrap.className = "worked-for";
  const head = document.createElement("div");
  head.className = "worked-head dim";
  head.textContent = duration ? `Worked for ${duration}` : "Worked";
  const steps = document.createElement("div");
  steps.className = "worked-steps hidden";
  for (const call of calls) {
    if (call.name === "update_plan") continue; // shown as the plan card instead
    const verb = toolVerb(call.name, call.args_json);
    const failed = call.result?.is_error;
    const step = document.createElement("div");
    step.className = "worked-step dim" + (failed ? " err" : "");
    step.innerHTML = `${icon(failed ? "x-circle" : "check-circle", 12)}<span></span>`;
    step.querySelector("span").textContent = verb;
    steps.appendChild(step);
  }
  head.onclick = () => {
    steps.classList.toggle("hidden");
    head.classList.toggle("open");
  };
  wrap.append(head, steps);
  return wrap;
}

function renderChangeCard(calls, convId, userMsgId) {
  const changes = [];
  // Track per-file previous content within the run so overwrite diffs are real.
  const lastContent = new Map();
  for (const call of calls) {
    const a = parseArgs(call.args_json);
    if (call.name === "edit_file" && call.result && !call.result.is_error) {
      changes.push(makeChange(a.path ?? "file", a.old_string ?? "", a.new_string ?? ""));
    } else if (call.name === "write_file" && call.result && !call.result.is_error) {
      const before = lastContent.get(a.path) ?? "";
      changes.push(makeChange(a.path ?? "file", before, a.content ?? ""));
      lastContent.set(a.path, a.content ?? "");
    }
  }
  if (!changes.length) return null;
  // Checkpoint coordinates, so the Review panel can revert a single file.
  for (const ch of changes) {
    ch.convId = convId;
    ch.afterMessageId = userMsgId;
  }
  const added = changes.reduce((s, c) => s + c.added, 0);
  const removed = changes.reduce((s, c) => s + Math.max(c.removed, 0), 0);
  const card = document.createElement("div");
  card.className = "change-card";
  const label = document.createElement("span");
  label.textContent = `${changes.length} file${changes.length > 1 ? "s" : ""} changed`;
  const stats = document.createElement("span");
  stats.innerHTML = ` <span class="diff-add-text">+${added}</span> <span class="diff-del-text">-${removed}</span>`;
  const btn = document.createElement("button");
  btn.className = "review-btn";
  btn.textContent = "Review";
  btn.onclick = () => openReview(changes);
  card.append(label, stats, btn);
  return card;
}

/// Antigravity-style history: user bubble → worked-for (collapsible steps) →
/// assistant md bubble → change card.
export function renderMessages(msgs, convId) {
  const el = $("messages");
  el.innerHTML = "";
  for (const run of groupRuns(msgs)) {
    const userBubble = addBubble("user");
    userBubble.dataset.msgId = run.user.id; // for the right-click Rewind menu
    for (const p of run.user.parts) {
      if (p.type === "text") renderUserText(userBubble, p.text);
    }
    const calls = collectCalls(run);
    // Runs that used a plan get a pointer note (checklist lives top right).
    if (calls.some((c) => c.name === "update_plan")) {
      const line = document.createElement("div");
      line.className = "plan-note dim";
      line.textContent = "Task set! Check the top right to see progress.";
      el.appendChild(line);
    }
    if (calls.length) {
      el.appendChild(renderWorkedFor(run, calls));
    }
    // Interleaved flow: text bubbles and tool rows in the order they happened,
    // so the user sees what ran between outputs (AG-style).
    const results = new Map();
    for (const m of run.items) {
      if (m.role !== "tool") continue;
      for (const p of m.parts) {
        if (p.type === "tool_result") results.set(p.tool_call_id, p);
      }
    }
    for (const m of run.items) {
      if (m.role !== "assistant") continue;
      for (const p of m.parts) {
        if (p.type === "text") {
          const bubble = addBubble("assistant");
          // Right-click shows which model produced this message.
          if (m.model) bubble.dataset.model = m.model;
          if (m.provider_id) bubble.dataset.provider = m.provider_id;
          renderTextPart(bubble, p.text);
        } else if (p.type === "tool_call" && p.name !== "update_plan") {
          el.appendChild(renderHistoryToolRow(p, results.get(p.id)));
        }
      }
    }
    const card = renderChangeCard(calls, convId, run.user.id);
    if (card) el.appendChild(card);
  }
  scrollToBottom();
}
