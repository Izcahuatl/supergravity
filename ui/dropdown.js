// Custom themed dropdown (no native <select>): trigger + grouped popup,
// closes on outside click / Escape. Options can be marked current.

let openDropdown = null;

export function makeDropdown({ value, groups, onSelect, emptyNote }) {
  const root = document.createElement("div");
  root.className = "dropdown";

  const trigger = document.createElement("button");
  trigger.type = "button";
  trigger.className = "dropdown-trigger";
  const label = document.createElement("span");
  label.className = "dropdown-value";
  label.textContent = value;
  const chevron = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  chevron.setAttribute("width", "12");
  chevron.setAttribute("height", "12");
  chevron.setAttribute("viewBox", "0 0 24 24");
  chevron.innerHTML = '<path d="m6 9 6 6 6-6" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>';
  trigger.append(label, chevron);

  const popup = document.createElement("div");
  popup.className = "dropdown-popup hidden";

  let isOpen = false;
  const close = () => {
    if (!isOpen) return;
    isOpen = false;
    popup.classList.add("hidden");
    trigger.classList.remove("open");
    if (openDropdown === api) openDropdown = null;
  };
  const open = () => {
    if (openDropdown && openDropdown !== api) openDropdown.close();
    isOpen = true;
    popup.classList.remove("hidden");
    trigger.classList.add("open");
    openDropdown = api;
  };

  trigger.onclick = (e) => {
    e.stopPropagation();
    isOpen ? close() : open();
  };
  document.addEventListener("click", close);
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") close();
  });

  let anyOptions = false;
  for (const group of groups) {
    if (!group.options.length) continue;
    anyOptions = true;
    const head = document.createElement("div");
    head.className = "dropdown-group dim";
    head.textContent = group.label;
    popup.appendChild(head);
    for (const opt of group.options) {
      const item = document.createElement("button");
      item.type = "button";
      item.className = "dropdown-item" + (opt.current ? " current" : "");
      item.textContent = opt.label;
      item.onclick = (e) => {
        e.stopPropagation();
        onSelect(opt.value);
        close();
      };
      popup.appendChild(item);
    }
  }
  if (!anyOptions && emptyNote) {
    const note = document.createElement("div");
    note.className = "dropdown-group dim";
    note.textContent = emptyNote;
    popup.appendChild(note);
  }

  root.append(trigger, popup);
  const api = {
    el: root,
    close,
    setValue(v) {
      label.textContent = v;
    },
  };
  return api;
}
