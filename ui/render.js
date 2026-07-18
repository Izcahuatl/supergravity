import { renderMarkdown } from "./markdown.js";
import { makeChange, openReview } from "./diffview.js";

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

export function prettyArgs(argsJson) {
  try {
    const v = JSON.parse(argsJson);
    const s = JSON.stringify(v, null, 1);
    return s.length > 300 ? s.slice(0, 300) + "…" : s;
  } catch {
    return argsJson.slice(0, 300);
  }
}

export function renderToolCallCard(call) {
  const card = document.createElement("div");
  card.className = "tool-card";
  const head = document.createElement("div");
  head.className = "tool-head";
  head.textContent = `🔧 ${call.name}`;
  const args = document.createElement("pre");
  args.className = "tool-args";
  args.textContent = prettyArgs(call.args_json);
  const status = document.createElement("div");
  status.className = "tool-status";
  card.append(head, args, status);
  return card;
}

export function renderResultOnCard(card, result) {
  const status = card.querySelector(".tool-status");
  status.textContent = result.is_error ? `✗ ${result.content.slice(0, 200)}` : `✓ ${result.content.slice(0, 200)}`;
  status.className = "tool-status " + (result.is_error ? "err" : "ok");
  const pre = document.createElement("pre");
  pre.className = "tool-result";
  pre.textContent = result.content.length > 1000 ? result.content.slice(0, 1000) + "\n…" : result.content;
  card.appendChild(pre);
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

const STEP_VERBS = {
  read_file: (a) => `Analyzed ${a.path ?? "file"}`,
  write_file: (a) => `Wrote ${a.path ?? "file"}`,
  edit_file: (a) => `Edited ${a.path ?? "file"}`,
  list_dir: (a) => `Listed ${a.path ?? "."}`,
  grep: (a) => `Searched ${a.pattern ?? ""}`,
  glob: (a) => `Found files matching ${a.pattern ?? ""}`,
  run_shell: (a) => `Ran ${(a.command ?? "").slice(0, 60)}`,
};

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
    const a = parseArgs(call.args_json);
    const verb = (STEP_VERBS[call.name] || ((a) => call.name))(a);
    const step = document.createElement("div");
    step.className = "worked-step dim";
    const failed = call.result?.is_error;
    step.textContent = `${failed ? "✗" : "✓"} ${verb}`;
    if (failed) step.classList.add("err");
    steps.appendChild(step);
  }
  head.onclick = () => {
    steps.classList.toggle("hidden");
    head.classList.toggle("open");
  };
  wrap.append(head, steps);
  return wrap;
}

function renderChangeCard(calls) {
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
export function renderMessages(msgs) {
  const el = $("messages");
  el.innerHTML = "";
  for (const run of groupRuns(msgs)) {
    const userBubble = addBubble("user");
    for (const p of run.user.parts) {
      if (p.type === "text") renderTextPart(userBubble, p.text);
    }
    const calls = collectCalls(run);
    if (calls.length) {
      el.appendChild(renderWorkedFor(run, calls));
    }
    for (const m of run.items) {
      if (m.role !== "assistant") continue;
      const bubble = addBubble("assistant");
      for (const p of m.parts) {
        if (p.type === "text") renderTextPart(bubble, p.text);
      }
      if (!bubble.hasChildNodes()) bubble.remove();
    }
    const card = renderChangeCard(calls);
    if (card) el.appendChild(card);
  }
  scrollToBottom();
}
