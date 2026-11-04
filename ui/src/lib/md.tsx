import { copyText } from "@/lib/utils";

function esc(s: string) {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

function inline(s: string) {
  return s
    .replace(/`([^`\n]+)`/g, "<code>$1</code>")
    .replace(/\*\*([^*]+)\*\*/g, "<b>$1</b>")
    .replace(/(^|[\s(])\*([^*\n]+)\*/g, "$1<i>$2</i>")
    .replace(/\[([^\]]+)\]\((https?:[^)\s]+)\)/g, '<a href="$2" target="_blank" rel="noreferrer">$1</a>');
}

const COPY_SVG = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>';
const CHECK_SVG = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6L9 17l-5-5"/></svg>';
const WRAP_SVG = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18"/><path d="M3 12h13a3 3 0 1 1 0 6h-4"/><path d="M14 15l-2 3 2 3"/><path d="M3 18h5"/></svg>';
const NOWRAP_SVG = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18"/><path d="M3 12h13a3 3 0 1 1 0 6"/><path d="M14 15l-2 3 2 3"/><path d="M3 18h3"/><path d="M21 6v12"/></svg>';

// dsh CodeToolbar: language label left, wrap + copy actions right
function codeBlock(lang: string, code: string) {
  const label = lang ? esc(lang) : "code";
  return (
    `<div class="md-code-card">` +
    `<div class="md-code-head">` +
    `<span class="md-code-lang">${label}</span>` +
    `<span class="md-code-actions">` +
    `<button type="button" class="md-wrap" aria-label="toggle line wrap" aria-pressed="true" title="toggle wrap">` +
    `<span class="md-wrap-i md-wrap-on">${NOWRAP_SVG}</span><span class="md-wrap-i md-wrap-off">${WRAP_SVG}</span>` +
    `</button>` +
    `<button type="button" class="md-copy" aria-label="copy code">` +
    `<span class="md-copy-i md-copy-copy">${COPY_SVG}</span><span class="md-copy-i md-copy-check">${CHECK_SVG}</span></button>` +
    `</span></div>` +
    `<pre class="md-code-pre" data-wrap="true"><code${lang ? ` class="lang-${esc(lang)}"` : ""}>${esc(code)}</code></pre>` +
    `</div>`
  );
}

// one pipe row → trimmed cells (GFM tables; escaped pipes are not supported)
function splitRow(line: string): string[] {
  let t = line.trim();
  if (t.startsWith("|")) t = t.slice(1);
  if (t.endsWith("|")) t = t.slice(0, -1);
  return t.split("|").map((c) => c.trim());
}

// separator line → per-column text alignment
function rowAligns(line: string): (string | null)[] {
  return splitRow(line).map((c) => {
    const left = c.startsWith(":");
    const right = c.endsWith(":") && c.length > 1;
    if (left && right) return "center";
    if (right) return "right";
    if (left) return "left";
    return null;
  });
}

const isTableRow = (line: string) => /^\|.*\|/.test(line.trim());
const isTableSep = (line: string) => /^\|?[\s:|-]+\|?$/.test(line.trim()) && line.includes("-");

// dsh renderTable: <4 columns fill the message column, 4+ keep natural width
// and scroll inside the wrapper (md-table-wide hook)
function tableHtml(rows: string[], aligns: (string | null)[]): string {
  const [head, ...body] = rows;
  const cols = splitRow(head).length;
  const wide = cols >= 4;
  const cell = (c: string, tag: "th" | "td", i: number) => {
    const a = aligns[i] ? ` style="text-align:${aligns[i]}"` : "";
    return `<${tag}${a}>${inline(c)}</${tag}>`;
  };
  let html = `<div class="md-table-scroll${wide ? " md-table-wide" : " md-table-fill"}"><table>`;
  if (head) {
    html += `<thead><tr>${splitRow(head).map((c, i) => cell(c, "th", i)).join("")}</tr></thead>`;
  }
  if (body.length) {
    html += `<tbody>${body.map((r) => `<tr>${splitRow(r).map((c, i) => cell(c, "td", i)).join("")}</tr>`).join("")}</tbody>`;
  }
  return html + "</table></div>";
}

export function md(src: string) {
  const blocks: [string, string][] = [];
  src = src.replace(/```(\w*)[^\S\n]*\n?([\s\S]*?)(```|$)/g, (_m, lang, code) => {
    blocks.push([lang, code.replace(/\n$/, "")]);
    return "\x00" + (blocks.length - 1) + "\x00";
  });
  src = esc(src);
  const lines = src.split("\n");
  let html = "";
  let para: string[] = [];
  let list: string | null = null;
  let quote: boolean | null = null;
  const flushPara = () => {
    if (para.length) {
      html += "<p>" + para.join("<br>") + "</p>";
      para = [];
    }
  };
  const closeList = () => {
    if (list) {
      html += "</" + list + ">";
      list = null;
    }
  };
  const closeQuote = () => {
    if (quote) {
      html += "</blockquote>";
      quote = null;
    }
  };
  for (let li = 0; li < lines.length; li++) {
    const raw = lines[li];
    const l = raw;
    const t = l.trim();
    const bm = t.match(/^\x00(\d+)\x00$/);
    if (bm) {
      flushPara();
      closeList();
      closeQuote();
      const [lang, code] = blocks[+bm[1]];
      html += codeBlock(lang, code);
      continue;
    }
    const h = t.match(/^(#{1,6})\s+(.*)$/);
    if (h) {
      flushPara();
      closeList();
      closeQuote();
      const n = h[1].length;
      html += `<h${n}>${inline(h[2])}</h${n}>`;
      continue;
    }
    if (/^(---+|\*\*\*+|___+)\s*$/.test(t)) {
      flushPara();
      closeList();
      closeQuote();
      html += "<hr>";
      continue;
    }
    if (t.startsWith("&gt;") || t.startsWith(">")) {
      const text = t.replace(/^&gt;\s?/, "").replace(/^>\s?/, "");
      flushPara();
      closeList();
      if (!quote) {
        html += "<blockquote>";
        quote = true;
      }
      html += inline(text) + "<br>";
      continue;
    }
    closeQuote();
    // GFM pipe table: a row followed by a |---|---| separator
    if (isTableRow(l) && li + 1 < lines.length && isTableSep(lines[li + 1])) {
      flushPara();
      closeList();
      const aligns = rowAligns(lines[li + 1]);
      const rows = [l];
      li += 2;
      while (li < lines.length && isTableRow(lines[li])) {
        rows.push(lines[li]);
        li++;
      }
      li--;
      html += tableHtml(rows, aligns);
      continue;
    }
    const ul = l.match(/^\s*[-*+]\s+(.*)$/);
    const ol = l.match(/^\s*\d+[.)]\s+(.*)$/);
    if (ul || ol) {
      flushPara();
      const want = ul ? "ul" : "ol";
      if (list !== want) {
        closeList();
        html += "<" + want + ">";
        list = want;
      }
      const match = ul || ol;
      if (!match) continue;
      let item = match[1];
      const cb = item.match(/^\[( |x|X)\]\s+(.*)$/);
      if (cb) {
        item = (cb[1] === " " ? "&#9744; " : "&#9745; ") + inline(cb[2]);
      } else {
        item = inline(item);
      }
      html += "<li>" + item + "</li>";
      continue;
    }
    if (!t) {
      flushPara();
      closeList();
      continue;
    }
    para.push(inline(l));
  }
  flushPara();
  closeList();
  closeQuote();
  return html;
}

export function Md({ text, className }: { text: string; className?: string }) {
  // delegated clicks for the generated per-block toolbar buttons
  const onClick = (e: React.MouseEvent<HTMLDivElement>) => {
    const target = e.target as HTMLElement;
    const wrapBtn = target.closest("button.md-wrap");
    if (wrapBtn) {
      const pre = wrapBtn.closest(".md-code-card")?.querySelector("pre");
      if (pre) {
        const wrapped = pre.getAttribute("data-wrap") !== "false";
        pre.setAttribute("data-wrap", wrapped ? "false" : "true");
        wrapBtn.setAttribute("aria-pressed", wrapped ? "false" : "true");
      }
      return;
    }
    const btn = target.closest("button.md-copy");
    if (!btn) return;
    const code = btn.closest(".md-code-card")?.querySelector("code");
    if (!code) return;
    void copyText(code.textContent || "").then((ok) => {
      if (!ok) return;
      btn.classList.add("copied");
      setTimeout(() => btn.classList.remove("copied"), 1400);
    });
  };
  return <div className={"md " + (className || "")} onClick={onClick} dangerouslySetInnerHTML={{ __html: md(text) }} />;
}

export function DiffLines({ rows }: { rows: { tag: 0 | 1 | 2; text: string }[] }) {
  return (
    <pre className="diff-lines my-1 rounded-md border border-border bg-code px-2.5 py-1.5">
      {rows.map((r, i) => {
        const cls = r.tag === 1 ? "diff-add" : r.tag === 2 ? "diff-del" : "diff-ctx";
        const sign = r.tag === 1 ? "+ " : r.tag === 2 ? "- " : "  ";
        return (
          <span key={i} className={cls}>
            {sign + r.text}
            {"\n"}
          </span>
        );
      })}
    </pre>
  );
}
