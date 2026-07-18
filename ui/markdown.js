// Minimal markdown: escapes HTML, then handles ``` fences, `code`, **bold**, *italic*, lists, paragraphs.
export function renderMarkdown(src) {
  const esc = (s) =>
    s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
  const inline = (s) =>
    esc(s)
      .replace(/`([^`]+)`/g, "<code>$1</code>")
      .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
      .replace(/\*([^*]+)\*/g, "<em>$1</em>")
      .replace(/\[([^\]]+)\]\((https?:\/\/[^)]+)\)/g, '<a href="$2" target="_blank" rel="noopener">$1</a>');
  // Split on ``` fences: odd indices are code blocks (first line = language hint, skipped).
  let html = "";
  const parts = src.split("```");
  for (let i = 0; i < parts.length; i++) {
    if (i % 2 === 1) {
      const nl = parts[i].indexOf("\n");
      const code = nl === -1 ? parts[i] : parts[i].slice(nl + 1);
      html += `<pre><code>${esc(code)}</code></pre>`;
    } else {
      const paragraphs = parts[i].split(/\n{2,}/);
      for (const p of paragraphs) {
        if (!p.trim()) continue;
        if (/^\s*[-*] /m.test(p)) {
          const items = p
            .split("\n")
            .filter((l) => /^\s*[-*] /.test(l))
            .map((l) => `<li>${inline(l.replace(/^\s*[-*] /, ""))}</li>`)
            .join("");
          html += `<ul>${items}</ul>`;
        } else {
          html += `<p>${inline(p.trim()).replace(/\n/g, "<br>")}</p>`;
        }
      }
    }
  }
  return html;
}
