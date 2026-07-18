import { state } from "./app.js";
import { addBubble, renderTextPart, renderToolCallCard, prettyArgs } from "./render.js";

const $ = (id) => document.getElementById(id);
let currentTextBubble = null;

function finishTextBubble() {
  currentTextBubble = null;
}

// NOTE: the `state.running` bookkeeping lives in app.js's onAgentEvent wrapper —
// this module only renders events for the ACTIVE conversation.
export function handleAgentEvent(payload) {
  const { conversation_id, event } = payload;
  if (state.active?.id !== conversation_id) return;

  switch (event.kind) {
    case "text_delta": {
      if (!currentTextBubble) {
        currentTextBubble = addBubble("assistant");
        currentTextBubble._raw = "";
      }
      currentTextBubble._raw += event.data;
      currentTextBubble.innerHTML = "";
      renderTextPart(currentTextBubble, currentTextBubble._raw);
      const el = document.getElementById("messages");
      el.scrollTop = el.scrollHeight;
      break;
    }
    case "tool_call_proposed": {
      finishTextBubble();
      const card = renderToolCallCard({ name: event.data.name, args_json: event.data.args_json });
      card.dataset.callId = event.data.tool_call_id;
      card.querySelector(".tool-status").textContent = "running…";
      addBubble("assistant").appendChild(card);
      break;
    }
    case "approval_requested": {
      finishTextBubble();
      const card = document.createElement("div");
      card.className = "approval-card";
      card.innerHTML = `<div class="tool-head">⚠ ${event.data.name} needs approval</div><pre class="tool-args"></pre>
        <div class="approval-buttons"><button class="approve">Approve</button><button class="deny">Deny</button></div>`;
      card.querySelector(".tool-args").textContent = prettyArgs(event.data.args_json);
      card.querySelector(".approve").onclick = () => {
        window.__TAURI__.core.invoke("resolve_approval", {
          conversationId: conversation_id,
          requestId: event.data.request_id,
          allow: true,
        });
        card.querySelector(".approval-buttons").remove();
      };
      card.querySelector(".deny").onclick = () => {
        window.__TAURI__.core.invoke("resolve_approval", {
          conversationId: conversation_id,
          requestId: event.data.request_id,
          allow: false,
        });
        card.querySelector(".approval-buttons").remove();
      };
      addBubble("assistant").appendChild(card);
      break;
    }
    case "tool_call_finished": {
      const card = document.querySelector(`[data-call-id="${event.data.tool_call_id}"]`);
      if (card) {
        const status = card.querySelector(".tool-status");
        status.textContent = (event.data.ok ? "✓ " : "✗ ") + event.data.summary.slice(0, 200);
        status.className = "tool-status " + (event.data.ok ? "ok" : "err");
      }
      break;
    }
    case "message_done":
      finishTextBubble();
      $("stop-agent").classList.add("hidden");
      break;
    case "error": {
      finishTextBubble();
      const bubble = addBubble("error");
      bubble.textContent = `Error: ${event.data}`;
      $("stop-agent").classList.add("hidden");
      break;
    }
    case "cancelled": {
      finishTextBubble();
      const bubble = addBubble("error");
      bubble.textContent = "Cancelled.";
      $("stop-agent").classList.add("hidden");
      break;
    }
  }
}

export function resetEventState() {
  finishTextBubble();
}
