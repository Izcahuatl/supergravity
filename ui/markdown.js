// Minimal markdown: escapes HTML, then handles ``` fences, `code`, **bold**, *italic*, lists, paragraphs.
// Also collapses <think>…</think> blocks (qwen3-style reasoning) into a details element.

// Tiny built-in syntax highlighter (no vendored lib). Runs on ESCAPED code, so
// string regexes match &quot; instead of ". Alternation order gives comments
// and strings priority — keywords inside them are never re-matched.
const HIGHLIGHT_LANGS = {
  js: {
    comment: String.raw`\/\/[^\n]*|\/\*[\s\S]*?\*\/`,
    string: String.raw`&quot;[^\n]*?&quot;|'[^'\n]*'|\`[^\`]*\``,
    keyword: String.raw`\b(?:const|let|var|function|return|if|else|for|while|class|new|import|export|from|async|await|try|catch|throw|typeof|instanceof|of|in|this|null|undefined|true|false)\b`,
    number: String.raw`\b\d+\.?\d*\b`,
  },
  rust: {
    comment: String.raw`\/\/[^\n]*`,
    string: String.raw`&quot;[^\n]*?&quot;`,
    keyword: String.raw`\b(?:fn|let|mut|pub|use|struct|enum|impl|match|if|else|for|while|loop|return|mod|crate|self|Self|type|trait|where|async|await|move|ref|const|static|in)\b`,
    number: String.raw`\b\d+\.?\d*\b`,
  },
  python: {
    comment: String.raw`#[^\n]*`,
    string: String.raw`&quot;[^\n]*?&quot;|'[^'\n]*'`,
    keyword: String.raw`\b(?:def|class|return|if|elif|else|for|while|import|from|as|with|try|except|finally|raise|lambda|pass|None|True|False|print|in|is|not|and|or)\b`,
    number: String.raw`\b\d+\.?\d*\b`,
  },
  bash: {
    comment: String.raw`#[^\n]*`,
    string: String.raw`&quot;[^\n]*?&quot;|'[^'\n]*'`,
    keyword: String.raw`\b(?:if|then|else|elif|fi|for|while|do|done|case|esac|function|echo|cd|export|local|return|exit|in)\b`,
    number: String.raw`\b\d+\.?\d*\b`,
  },
  json: {
    comment: String.raw`(?!)`,
    string: String.raw`&quot;[^\n]*?&quot;`,
    keyword: String.raw`\b(?:true|false|null)\b`,
    number: String.raw`-?\b\d+\.?\d*(?:[eE][+-]?\d+)?\b`,
  },
};
const LANG_ALIAS = {
  js: "js", javascript: "js", ts: "js", typescript: "js", jsx: "js", tsx: "js",
  rust: "rust", rs: "rust",
  python: "python", py: "python",
  bash: "bash", sh: "bash", shell: "bash", zsh: "bash",
  json: "json",
};

function highlight(code, hint) {
  const L = HIGHLIGHT_LANGS[LANG_ALIAS[hint] || hint];
  if (!L) return code;
  const re = new RegExp(`(${L.comment})|(${L.string})|(${L.keyword})|(${L.number})`, "g");
  return code.replace(
    re,
    (m, c, s, k) => `<span class="${c ? "tok-c" : s ? "tok-s" : k ? "tok-k" : "tok-n"}">${m}</span>`
  );
}

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
  // Split on ``` fences: odd indices are code blocks (first line = language hint).
  let html = "";
  const parts = safe.split("```");
  for (let i = 0; i < parts.length; i++) {
    if (i % 2 === 1) {
      const nl = parts[i].indexOf("\n");
      const hint = (nl === -1 ? parts[i] : parts[i].slice(0, nl)).trim().toLowerCase();
      const code = nl === -1 ? parts[i] : parts[i].slice(nl + 1);
      html += `<div class="codeblock"><button class="code-copy" title="Copy">⧉</button><pre><code>${highlight(code, hint)}</code></pre></div>`;
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
          const h = line.match(/^(#{1,6})\s+(.*)$/);
          if (h) {
            flushPara();
            flushList();
            flushTable();
            // Shift levels down so headings stay modest inside a chat bubble.
            const level = Math.min(h[1].length + 2, 6);
            html += `<h${level}>${inline(h[2])}</h${level}>`;
            continue;
          }
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
