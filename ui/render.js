export function renderMessages(msgs) {
  const el = document.getElementById("messages");
  el.innerHTML = msgs
    .map((m) => `<div>${m.role}: ${m.parts.map((p) => p.text || p.content || p.name || "").join(" ")}</div>`)
    .join("");
}
