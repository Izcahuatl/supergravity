// @-mention autocomplete for the composers: typing `@query` searches workspace
// files and offers completions; picking one inserts the relative path.
import { api } from "./api.js";
import { icon } from "./icons.js";

const TOKEN_RE = /(?:^|\s)@([\w\-./\\]*)$/;

export function attachMentions(textarea, getConversationId) {
  const popup = document.createElement("div");
  popup.className = "mention-popup hidden";
  document.body.appendChild(popup);

  let items = [];
  let active = 0;
  let tokenStart = -1;
  let debounce = null;

  const close = () => {
    popup.classList.add("hidden");
    items = [];
    tokenStart = -1;
  };

  const currentToken = () => {
    const upto = textarea.value.slice(0, textarea.selectionStart);
    const m = upto.match(TOKEN_RE);
    if (!m) return null;
    return { query: m[1], start: upto.length - m[1].length - 1 };
  };

  const place = () => {
    const r = textarea.getBoundingClientRect();
    popup.style.left = `${r.left}px`;
    popup.style.width = `${Math.min(420, r.width)}px`;
    popup.style.bottom = `${window.innerHeight - r.top + 6}px`;
  };

  const render = () => {
    popup.innerHTML = "";
    items.forEach((p, i) => {
      const it = document.createElement("button");
      it.type = "button";
      it.className = "dropdown-item" + (i === active ? " current" : "");
      it.innerHTML = `${icon("file", 13)}<span></span>`;
      it.querySelector("span").textContent = p;
      it.onmousedown = (e) => {
        e.preventDefault(); // keep textarea focus
        pick(i);
      };
      popup.appendChild(it);
    });
    if (!items.length) {
      const note = document.createElement("div");
      note.className = "dropdown-group dim";
      note.textContent = "no matching files";
      popup.appendChild(note);
    }
  };

  const pick = (i) => {
    const path = items[i];
    if (path == null) return;
    const before = textarea.value.slice(0, tokenStart);
    const after = textarea.value.slice(textarea.selectionStart);
    textarea.value = `${before}@${path} ${after}`;
    const caret = before.length + path.length + 2;
    textarea.setSelectionRange(caret, caret);
    textarea.dispatchEvent(new Event("input"));
    textarea.focus();
    close();
  };

  const search = async () => {
    const t = currentToken();
    const convId = getConversationId();
    if (!t || !convId) return close();
    tokenStart = t.start;
    try {
      items = await api.searchWorkspaceFiles(convId, t.query);
    } catch {
      items = [];
    }
    // The caret may have moved past the token while the search flew.
    if (!currentToken()) return close();
    active = 0;
    place();
    render();
    popup.classList.remove("hidden");
  };

  textarea.addEventListener("input", () => {
    clearTimeout(debounce);
    if (!currentToken()) return close();
    debounce = setTimeout(search, 150);
  });
  textarea.addEventListener("keydown", (e) => {
    if (popup.classList.contains("hidden")) return;
    if (e.key === "ArrowDown" || (e.key === "Tab" && !e.shiftKey)) {
      e.preventDefault();
      active = Math.min(active + 1, items.length - 1);
      render();
    } else if (e.key === "ArrowUp" || (e.key === "Tab" && e.shiftKey)) {
      e.preventDefault();
      active = Math.max(active - 1, 0);
      render();
    } else if (e.key === "Enter") {
      e.preventDefault();
      e.stopImmediatePropagation(); // don't let the composer send
      pick(active);
    } else if (e.key === "Escape") {
      e.stopImmediatePropagation();
      close();
    }
  });
  textarea.addEventListener("blur", () => setTimeout(close, 150));
  document.addEventListener("click", (e) => {
    if (!popup.contains(e.target) && e.target !== textarea) close();
  });
}
