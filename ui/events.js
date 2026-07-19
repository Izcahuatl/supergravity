import { state, refreshMessages, syncSendStop } from "./app.js";
import { api } from "./api.js";
import { addBubble, renderTextPart, renderToolCallCard, prettyArgs } from "./render.js";
import { icon } from "./icons.js";

const $ = (id) => document.getElementById(id);

// Per-conversation live state, so switching conversations mid-run loses
// nothing: streamed text accumulates here (not only in the DOM), and pending
// approvals are remembered until resolved or finished.
const streamBuffers = new Map(); // conversation_id -> raw streamed text
export const pendingApprovals = new Map(); // conversation_id -> {request_id, tool_call_id, name, args_json}

let currentTextBubble = null;

export function handleAgentEvent(payload) {
  const { conversation_id, event } = payload;

  // Non-active conversations: track state only (no DOM).
  if (state.active?.id !== conversation_id) {
    if (event.kind === "text_delta") {
      streamBuffers.set(conversation_id, (streamBuffers.get(conversation_id) || "") + event.data);
    } else if (event.kind === "approval_requested") {
      pendingApprovals.set(conversation_id, { ...event.data });
    } else if (event.kind === "tool_call_finished") {
      pendingApprovals.delete(conversation_id);
    } else if (["message_done", "error", "cancelled"].includes(event.kind)) {
      streamBuffers.delete(conversation_id);
      pendingApprovals.delete(conversation_id);
    }
    return;
  }

  switch (event.kind) {
    case "text_delta": {
      const raw = (streamBuffers.get(conversation_id) || "") + event.data;
      streamBuffers.set(conversation_id, raw);
      // The bubble may have been detached by a history re-render — recreate it.
      if (!currentTextBubble || !currentTextBubble.isConnected) {
        currentTextBubble = addBubble("assistant");
        currentTextBubble.classList.add("streaming");
      }
      currentTextBubble.innerHTML = "";
      renderTextPart(currentTextBubble, raw);
      const el = document.getElementById("messages");
      el.scrollTop = el.scrollHeight;
      break;
    }
    case "tool_call_proposed": {
      currentTextBubble = null;
      const card = renderToolCallCard({ name: event.data.name, args_json: event.data.args_json });
      card.dataset.callId = event.data.tool_call_id;
      card.querySelector(".tool-status").textContent = "running…";
      addBubble("assistant").appendChild(card);
      break;
    }
    case "approval_requested": {
      currentTextBubble = null;
      pendingApprovals.set(conversation_id, { ...event.data });
      // One card per call: morph the proposed card into its approval state
      // instead of adding a second card for the same call.
      const existing = document.querySelector(`[data-call-id="${event.data.tool_call_id}"]`);
      if (existing) {
        existing.classList.add("approval-card");
        // Approving blind is useless — show the args that need a decision.
        existing.querySelector(".tool-args")?.classList.remove("hidden");
        const status = existing.querySelector(".tool-status");
        status.textContent = "";
        status.appendChild(buildApprovalButtons(conversation_id, event.data));
      } else {
        addBubble("assistant").appendChild(buildApprovalCard(conversation_id, event.data));
      }
      break;
    }
    case "tool_call_finished": {
      pendingApprovals.delete(conversation_id);
      const card = document.querySelector(`[data-call-id="${event.data.tool_call_id}"]`);
      if (card) {
        const status = card.querySelector(".tool-status");
        if (status) {
          status.textContent = event.data.ok ? "✓" : "✗";
          status.className = "tool-status " + (event.data.ok ? "ok" : "err");
        }
        const detail = card.querySelector(".tool-args");
        if (detail && event.data.summary) {
          detail.textContent += `\n→ ${event.data.summary.slice(0, 400)}`;
        }
        // Failures stay expanded so the error is visible without a click.
        if (!event.data.ok) detail?.classList.remove("hidden");
        card.querySelector(".approval-buttons")?.remove();
      }
      break;
    }
    case "message_done":
      currentTextBubble = null;
      streamBuffers.delete(conversation_id);
      pendingApprovals.delete(conversation_id);
      syncSendStop();
      // Persisted history lands just after message_done — refresh shortly after
      // so the worked-for line and change cards render from the store.
      setTimeout(() => refreshMessages(), 400);
      break;
    case "error": {
      currentTextBubble = null;
      streamBuffers.delete(conversation_id);
      pendingApprovals.delete(conversation_id);
      document.querySelectorAll(".approval-card .approval-buttons").forEach((b) => b.remove());
      const bubble = addBubble("error");
      bubble.textContent = `Error: ${event.data}`;
      syncSendStop();
      break;
    }
    case "cancelled": {
      currentTextBubble = null;
      streamBuffers.delete(conversation_id);
      pendingApprovals.delete(conversation_id);
      document.querySelectorAll(".approval-card .approval-buttons").forEach((b) => b.remove());
      const bubble = addBubble("error");
      bubble.textContent = "Cancelled.";
      syncSendStop();
      break;
    }
  }
}

export function buildApprovalButtons(conversationId, data) {
  const buttons = document.createElement("div");
  buttons.className = "approval-buttons";
  const approve = document.createElement("button");
  approve.className = "approve";
  approve.textContent = "Approve";
  approve.onclick = () => {
    api.resolveApproval(conversationId, data.request_id, true).catch(() => {});
    buttons.remove();
  };
  const deny = document.createElement("button");
  deny.className = "deny";
  deny.textContent = "Deny";
  deny.onclick = () => {
    api.resolveApproval(conversationId, data.request_id, false).catch(() => {});
    buttons.remove();
  };
  buttons.append(approve, deny);
  return buttons;
}

export function buildApprovalCard(conversationId, data) {
  const card = document.createElement("div");
  card.className = "approval-card";
  card.dataset.callId = data.tool_call_id;
  const head = document.createElement("div");
  head.className = "tool-head";
  head.innerHTML = `${icon("alert", 13)}<span></span>`;
  head.querySelector("span").textContent = `${data.name} needs approval`;
  const args = document.createElement("pre");
  args.className = "tool-args";
  args.textContent = prettyArgs(data.args_json);
  card.append(head, args, buildApprovalButtons(conversationId, data));
  return card;
}

/// Called by selectConversation after rendering history: re-render any live
/// approval card for this conversation (stream text resumes on next delta).
export function resumeLiveState(conversationId) {
  currentTextBubble = null;
  const pending = pendingApprovals.get(conversationId);
  if (pending) {
    addBubble("assistant").appendChild(buildApprovalCard(conversationId, pending));
  }
}

export function resetEventState() {
  currentTextBubble = null;
}
