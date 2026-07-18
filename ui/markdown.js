// Minimal markdown: escapes HTML, then handles ``` fences, `code`, **bold**, *italic*, lists, paragraphs.
// Also collapses <think>…</think> blocks (qwen3-style reasoning) into a details element.
export function renderMarkdown(src) {
  const esc = (s) =>
    s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
  // Escape everything first; all downstream passes work on escaped text.
  let safe = esc(src);
  // <think> blocks → collapsible "Thinking…" (unclosed block while streaming = open).
  safe = safe.replace(
    /&lt;think&gt;([\s\S]*?)(&lt;\/think&gt;|$)/g,
    (_, inner, close) =>
      `<details class="think"${close ? "" : " open"}><summary>Thinking…</summary><pre>${inner}</pre></details>`
  );
  const inline = (s) => {
    // Protect code spans from later passes (bold/italic must not apply inside code).
    const codes = [];
    let out = s.replace(/`([^`]+)`/g, (_, c) => {
      codes.push(c);
      return `\u0000${codes.length - 1}\u0000`;
    });
    out = out
      .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
      .replace(/\*([^*]+)\*/g, "<em>$1</em>")
      .replace(/\[([^\]]+)\]\((https?:\/\/[^)]+)\)/g, '<a href="$2" target="_blank" rel="noopener">$1</a>');
    return out.replace(/\u0000(\d+)\u0000/g, (_, i) => `<code>${codes[Number(i)]}</code>`);
  };
  // Split on ``` fences: odd indices are code blocks (first line = language hint, skipped).
  let html = "";
  const parts = safe.split("```");
  for (let i = 0; i < parts.length; i++) {
    if (i % 2 === 1) {
      const nl = parts[i].indexOf("\n");
      const code = nl === -1 ? parts[i] : parts[i].slice(nl + 1);
      html += `<div class="codeblock"><button class="code-copy" title="Copy">⧉</button><pre><code>${code}</code></pre></div>`;
    } else {
      const paragraphs = parts[i].split(/\n{2,}/);
      for (const p of paragraphs) {
        if (!p.trim()) continue;
        // Line-by-line: consecutive list lines form a <ul>; pipe lines form a
        // <table>; other lines form paragraphs — mixed blocks keep everything.
        let listItems = [];
        let para = [];
        let tableRows = [];
        const flushPara = () => {
          if (para.length) {
            // Single newlines become <br> — LLM output uses them structurally.
            html += `<p>${para.map((l) => inline(l)).join("<br>")}</p>`;
            para = [];
          }
        };
        const flushList = () => {
          if (listItems.length) {
            html += `<ul>${listItems.join("")}</ul>`;
            listItems = [];
          }
        };
        const flushTable = () => {
          if (!tableRows.length) return;
          const cells = (row) =>
            row.replace(/^\s*\|/, "").replace(/\|\s*$/, "").split("|").map((c) => c.trim());
          const isSep = (row) => /^[\s|:-]+$/.test(row);
          let out = "<table>";
          let headerDone = false;
          for (const row of tableRows) {
            if (isSep(row)) continue;
            const tag = headerDone ? "td" : "th";
            out += `<tr>${cells(row).map((c) => `<${tag}>${inline(c)}</${tag}>`).join("")}</tr>`;
            headerDone = true;
          }
          html += out + "</table>";
          tableRows = [];
        };
        for (const line of p.split("\n")) {
          const m = line.match(/^\s*[-*] (.*)$/);
          if (m) {
            flushPara();
            flushTable();
            listItems.push(`<li>${inline(m[1])}</li>`);
          } else if (/^\s*\|/.test(line)) {
            flushPara();
            flushList();
            tableRows.push(line);
          } else {
            flushList();
            flushTable();
            para.push(line);
          }
        }
        flushList();
        flushTable();
        flushPara();
      }
    }
  }
  return html;
}
