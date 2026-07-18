import { renderMarkdown } from "./markdown.js";

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

export function renderToolCallCard(call) {
  const card = document.createElement("div");
  card.className = "tool-card";
  card.innerHTML = `<div class="tool-head">🔧 ${call.name}</div><pre class="tool-args"></pre><div class="tool-status"></div>`;
  card.querySelector(".tool-args").textContent = prettyArgs(call.args_json);
  return card;
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

export function renderResultOnCard(card, result) {
  const status = card.querySelector(".tool-status");
  status.textContent = result.is_error ? `✗ ${result.content.slice(0, 200)}` : `✓ ${result.content.slice(0, 200)}`;
  status.className = "tool-status " + (result.is_error ? "err" : "ok");
  const pre = document.createElement("pre");
  pre.className = "tool-result";
  pre.textContent = result.content.length > 1000 ? result.content.slice(0, 1000) + "\n…" : result.content;
  card.appendChild(pre);
}

export function renderMessages(msgs) {
  const el = $("messages");
  el.innerHTML = "";
  for (const m of msgs) {
    if (m.role === "system") continue;
    if (m.role === "tool") {
      // attach results to the preceding assistant tool cards by order
      for (const p of m.parts) {
        if (p.type === "tool_result") {
          const card = document.querySelector(`[data-call-id="${p.tool_call_id}"]`);
          if (card) renderResultOnCard(card, p);
        }
      }
      continue;
    }
    const bubble = addBubble(m.role);
    for (const p of m.parts) {
      if (p.type === "text") renderTextPart(bubble, p.text);
      if (p.type === "tool_call") {
        const card = renderToolCallCard(p);
        card.dataset.callId = p.id;
        bubble.appendChild(card);
      }
    }
  }
  scrollToBottom();
}
