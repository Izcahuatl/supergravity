import { icon } from "./icons.js";

// Review Changes panel: line-based diff rendering for edit_file/write_file changes.

const $ = (id) => document.getElementById(id);

/// One file's change: { path, added, removed, rows: [{type: 'same'|'add'|'del', text}] }
export function makeChange(path, oldText, newText) {
  const rows = lineDiff(oldText, newText);
  const added = rows.filter((r) => r.type === "add").length;
  const removed = rows.filter((r) => r.type === "del").length;
  return { path, added, removed, rows };
}

/// LCS line diff → rows of same/add/del. O(n*m), fine for edit hunks.
export function lineDiff(oldText, newText) {
  const a = oldText === "" ? [] : oldText.split("\n");
  const b = newText === "" ? [] : newText.split("\n");
  const n = a.length, m = b.length;
  const dp = Array.from({ length: n + 1 }, () => new Uint16Array(m + 1));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      dp[i][j] = a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
    }
  }
  const rows = [];
  let i = 0, j = 0;
  while (i < n && j < m) {
    if (a[i] === b[j]) {
      rows.push({ type: "same", text: a[i] });
      i++; j++;
    } else if (dp[i + 1][j] >= dp[i][j + 1]) {
      rows.push({ type: "del", text: a[i] });
      i++;
    } else {
      rows.push({ type: "add", text: b[j] });
      j++;
    }
  }
  while (i < n) rows.push({ type: "del", text: a[i++] });
  while (j < m) rows.push({ type: "add", text: b[j++] });
  // Collapse leading/trailing "same" rows to a context of 3 around changes.
  return rows;
}

export function openReview(changes) {
  const body = $("review-body");
  body.innerHTML = "";
  for (const ch of changes) {
    const card = document.createElement("div");
    card.className = "review-file";
    const head = document.createElement("div");
    head.className = "review-file-head";
    const name = document.createElement("span");
    name.className = "review-file-name";
    name.innerHTML = `${icon("file", 13)}<span></span>`;
    name.querySelector("span").textContent = ch.path;
    const stats = document.createElement("span");
    stats.className = "review-stats";
    const plus = document.createElement("span");
    plus.className = "diff-add-text";
    plus.textContent = `+${ch.added}`;
    const minus = document.createElement("span");
    minus.className = "diff-del-text";
    minus.textContent = ch.removed >= 0 ? ` -${ch.removed}` : " rewritten";
    stats.append(plus, minus);
    head.append(name, stats);

    const table = document.createElement("div");
    table.className = "diff-table";
    let oldLn = 0, newLn = 0;
    // compute starting line numbers from first non-same row
    for (const row of ch.rows) {
      const tr = document.createElement("div");
      tr.className = `diff-row diff-${row.type}`;
      const lnOld = document.createElement("span");
      lnOld.className = "diff-ln";
      const lnNew = document.createElement("span");
      lnNew.className = "diff-ln";
      const txt = document.createElement("span");
      txt.className = "diff-text";
      txt.textContent = row.text || " ";
      if (row.type !== "add") lnOld.textContent = ++oldLn;
      if (row.type !== "del") lnNew.textContent = ++newLn;
      tr.append(lnOld, lnNew, txt);
      table.appendChild(tr);
    }
    card.append(head, table);
    body.appendChild(card);
  }
  $("review-panel").classList.remove("hidden");
}

export function closeReview() {
  $("review-panel").classList.add("hidden");
}

export function initReview() {
  $("close-review").onclick = closeReview;
}
