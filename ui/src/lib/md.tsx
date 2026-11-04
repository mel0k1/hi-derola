import { useEffect, useRef } from "react";
import { copyText } from "@/lib/utils";
import { invoke } from "@/lib/tauri";
import { useStore } from "@/store/app";
import { openContextMenu } from "@/lib/menu";
import { openLightbox } from "@/lib/lightbox";
import { KEYBLOCK_RE, esc, maskedFor, maskSegments } from "@/lib/secrets";
import { iconSvg } from "@/lib/icons";
import katex from "katex";
import "katex/dist/katex.min.css";

// dsh math rendering: $…$, $$…$$, \(…\), \[…\] and ```math fences via KaTeX.
// Incomplete pairs (mid-stream) never match and stay literal — no error flash.
function unescapeTex(s: string) {
  return s.replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, '"').replace(/&amp;/g, "&");
}

function renderMath(tex: string, display: boolean): string {
  try {
    return katex.renderToString(unescapeTex(tex), { throwOnError: false, strict: "ignore", displayMode: display });
  } catch {
    return `<span class="katex-error">${esc(tex)}</span>`;
  }
}

function inline(s: string, blocks: string[]) {
  // code spans stash first so markup and math never touch their content;
  // math stashes second so $…$ is safe from bold/italic/link rules
  const codes: string[] = [];
  const maths: string[] = [];
  const out = s
    .replace(/`([^`\n]+)`/g, (_m, c) => "\x01" + (codes.push(c) - 1) + "\x01")
    .replace(/\\\[([\s\S]+?)\\\]/g, (_m, tex) => "\x04" + (maths.push(renderMath(tex, true)) - 1) + "\x04")
    .replace(/\$\$([^\n]+?)\$\$/g, (_m, tex) => "\x04" + (maths.push(renderMath(tex, true)) - 1) + "\x04")
    .replace(/\\\(([^\n]+?)\\\)/g, (_m, tex) => "\x04" + (maths.push(renderMath(tex, false)) - 1) + "\x04")
    // require a non-space first/last char so "$5 and $10" stays literal
    .replace(/\$(\S(?:[^\n$]*?[^\s$])?)\$/g, (_m, tex) => "\x04" + (maths.push(renderMath(tex, false)) - 1) + "\x04")
    .replace(/\*\*([^*]+)\*\*/g, "<b>$1</b>")
    .replace(/(^|[\s(])\*([^*\n]+)\*/g, "$1<i>$2</i>")
    // images before links, so ![alt](url) is not eaten by the link rule;
    // the tile opens the dsh ImageLightbox with the original resolution
    .replace(
      /!\[([^\]]*)\]\((https?:[^)\s]+)\)/g,
      '<span class="md-img" role="button" tabindex="0" data-src="$2" data-alt="$1"><img src="$2" alt="$1" loading="lazy" draggable="false"></span>',
    )
    .replace(/\[([^\]]+)\]\((https?:[^)\s]+)\)/g, '<a href="$2" target="_blank" rel="noreferrer">$1</a>');
  const restored = out
    .replace(/\x01(\d+)\x01/g, (_m, i) => "<code>" + esc(codes[+i] ?? "") + "</code>")
    .replace(/\x04(\d+)\x04/g, (_m, i) => maths[+i] ?? "");
  // dsh secret rule: credentials masked last so markup protects href/code, eye toggles reveal
  return maskSegments(restored, blocks, true);
}

const COPY_SVG = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>';
const CHECK_SVG = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6L9 17l-5-5"/></svg>';
const WRAP_SVG = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18"/><path d="M3 12h13a3 3 0 1 1 0 6h-4"/><path d="M14 15l-2 3 2 3"/><path d="M3 18h5"/></svg>';
const NOWRAP_SVG = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18"/><path d="M3 12h13a3 3 0 1 1 0 6"/><path d="M14 15l-2 3 2 3"/><path d="M3 18h3"/><path d="M21 6v12"/></svg>';
const EYE_SVG = iconSvg("eye");
const EYE_OFF_SVG = iconSvg("eyeoff");

// dsh SecretText: masked credential with an inline reveal control
function secretBlock(m: string): string {
  const lines = m.split("\n");
  const masked =
    lines.length > 2
      ? lines[0] + "\n" + lines.slice(1, -1).map((l) => (l ? "*".repeat(l.length) : l)).join("\n") + "\n" + lines[lines.length - 1]
      : maskedFor(m, 8);
  const stars = masked
    .split(/(\*+)/)
    .map((p) => (p ? (p[0] === "*" ? `<span class="md-secret-star">${p}</span>` : esc(p).replace(/\n/g, "<br>")) : ""))
    .join("");
  return (
    `<span class="md-secret" data-raw="${m}" data-masked="${masked}" data-masked-html="${esc(stars)}">` +
    `<span class="md-secret-value">${stars}</span>` +
    `<button type="button" class="md-eye" aria-pressed="false" aria-label="reveal value" title="reveal value">` +
    `<span class="md-eye-i md-eye-reveal">${EYE_SVG}</span>` +
    `<span class="md-eye-i md-eye-hide">${EYE_OFF_SVG}</span>` +
    `</button></span>`
  );
}

// dsh CodeToolbar: language label left, wrap + copy actions right.
// Long blocks get a scrollable body with an overview minimap on the right
const MINIMAP_MIN_LINES = 28;

function codeBlock(lang: string, code: string) {
  const label = lang ? esc(lang) : "code";
  const pre = `<pre class="md-code-pre" data-wrap="true"><code${lang ? ` class="lang-${esc(lang)}"` : ""}>${esc(code)}</code></pre>`;
  const body = code.split("\n").length >= MINIMAP_MIN_LINES ? `<div class="md-code-body">${pre}<canvas class="md-minimap" aria-hidden="true"></canvas></div>` : pre;
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
    body +
    `</div>`
  );
}

// dsh diff review: ```diff/patch fences render as a review card — add/del line
// tinting, old/new gutters tracked from the @@ hunk headers, +N −M stats in
// the toolbar; copy hands out the raw patch (data-code, not the gutters)
function diffBlock(code: string) {
  const lines = code.split("\n");
  if (lines.length && lines[lines.length - 1] === "") lines.pop();
  let adds = 0;
  let dels = 0;
  for (const l of lines) {
    if (l.startsWith("+") && !l.startsWith("+++")) adds++;
    else if (l.startsWith("-") && !l.startsWith("---")) dels++;
  }
  let o = 0;
  let n = 0;
  const row = (cls: string, no: string, nn: string, tx: string) =>
    `<span class="md-dl md-dl-${cls}"><span class="md-dl-no">${no}</span><span class="md-dl-nn">${nn}</span><span class="md-dl-tx">${tx}</span></span>`;
  const rows = lines
    .map((l) => {
      const t = esc(l);
      if (/^(---|\+\+\+)\s/.test(l) || l.startsWith("diff --git") || l.startsWith("index ") || l.startsWith("\\")) {
        return row("meta", "", "", t);
      }
      if (l.startsWith("@@")) {
        const m = l.match(/^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
        if (m) {
          o = parseInt(m[1], 10);
          n = parseInt(m[2], 10);
        }
        return row("hunk", "", "", t);
      }
      if (l.startsWith("+")) {
        const r = row("add", "", String(n), t);
        n++;
        return r;
      }
      if (l.startsWith("-")) {
        const r = row("del", String(o), "", t);
        o++;
        return r;
      }
      const r = row("ctx", String(o), String(n), t);
      o++;
      n++;
      return r;
    })
    .join("");
  return (
    `<div class="md-code-card md-diff">` +
    `<div class="md-code-head">` +
    `<span class="md-code-lang">diff</span>` +
    `<span class="md-code-actions">` +
    `<span class="md-diff-stat"><span class="md-diff-adds">+${adds}</span><span class="md-diff-dels">\u2212${dels}</span></span>` +
    `<button type="button" class="md-wrap" aria-label="toggle line wrap" aria-pressed="true" title="toggle wrap">` +
    `<span class="md-wrap-i md-wrap-on">${NOWRAP_SVG}</span><span class="md-wrap-i md-wrap-off">${WRAP_SVG}</span>` +
    `</button>` +
    `<button type="button" class="md-copy" aria-label="copy patch" data-code="${esc(code)}">` +
    `<span class="md-copy-i md-copy-copy">${COPY_SVG}</span><span class="md-copy-i md-copy-check">${CHECK_SVG}</span></button>` +
    `</span></div>` +
    `<pre class="md-code-pre md-diff-pre" data-wrap="true"><code>${rows}</code></pre>` +
    `</div>`
  );
}

// dsh minimap: overview strip of a long code card — one bar per line (width =
// line length), a band for the viewport, click/drag to scroll the code
function paintMinimap(cv: HTMLCanvasElement, pre: HTMLElement) {
  const w = cv.clientWidth;
  const h = cv.clientHeight;
  if (!w || !h) return;
  const dpr = window.devicePixelRatio || 1;
  cv.width = Math.round(w * dpr);
  cv.height = Math.round(h * dpr);
  const ctx = cv.getContext("2d");
  if (!ctx) return;
  ctx.scale(dpr, dpr);
  ctx.clearRect(0, 0, w, h);
  const lines = (pre.querySelector("code")?.textContent || "").split("\n");
  const total = lines.length;
  const slots = Math.max(1, Math.floor(h));
  const charW = Math.max(1, (w - 9) / 100);
  const color = getComputedStyle(pre).color || "#888";
  const slotW = new Array<number>(slots).fill(0);
  for (let i = 0; i < total; i++) {
    const s = Math.min(slots - 1, Math.floor((i * slots) / total));
    slotW[s] = Math.max(slotW[s], lines[i].replace(/\s+$/, "").length);
  }
  ctx.fillStyle = color;
  ctx.globalAlpha = 0.38;
  for (let s = 0; s < slots; s++) {
    if (!slotW[s]) continue;
    const bw = Math.min(w - 7, Math.max(1.5, slotW[s] * charW));
    ctx.fillRect(4, s + 0.5, bw, 1);
  }
  // viewport band + right edge indicator
  if (pre.scrollHeight > pre.clientHeight + 4) {
    const top = (pre.scrollTop / pre.scrollHeight) * h;
    const bh = Math.max(10, (pre.clientHeight / pre.scrollHeight) * h);
    ctx.globalAlpha = 0.08;
    ctx.fillRect(0, top, w, bh);
    ctx.globalAlpha = 0.5;
    ctx.fillRect(w - 3, top, 2, bh);
  }
}

// paint pass for every minimap under root; listeners bind once per pre
export function paintMinimaps(root: HTMLElement | null) {
  if (!root || !root.querySelectorAll) return;
  root.querySelectorAll<HTMLCanvasElement>(".md-minimap").forEach((cv) => {
    const pre = cv.closest(".md-code-card")?.querySelector("pre");
    if (!pre) return;
    const repaint = () => paintMinimap(cv, pre);
    repaint();
    const host = pre as HTMLElement & { __mdMini?: boolean };
    if (host.__mdMini) return;
    host.__mdMini = true;
    pre.addEventListener("scroll", repaint, { passive: true });
    const jump = (clientY: number) => {
      const r = cv.getBoundingClientRect();
      pre.scrollTop = ((clientY - r.top - pre.clientHeight / 2) / Math.max(1, cv.clientHeight)) * pre.scrollHeight;
    };
    cv.addEventListener("pointerdown", (e) => {
      e.preventDefault();
      e.stopPropagation();
      cv.setPointerCapture(e.pointerId);
      jump(e.clientY);
      const move = (ev: PointerEvent) => jump(ev.clientY);
      const up = () => {
        cv.removeEventListener("pointermove", move);
        cv.removeEventListener("pointerup", up);
      };
      cv.addEventListener("pointermove", move);
      cv.addEventListener("pointerup", up);
    });
    new ResizeObserver(repaint).observe(pre);
  });
}

// wrap toggle changes the pre height — refresh the band of that card's minimap
function repaintMinimap(card: Element | null | undefined) {
  const cv = card?.querySelector(".md-minimap");
  const pre = card?.querySelector("pre");
  if (cv && pre) paintMinimap(cv as HTMLCanvasElement, pre as HTMLElement);
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
function tableHtml(rows: string[], aligns: (string | null)[], blocks: string[]): string {
  const [head, ...body] = rows;
  const cols = splitRow(head).length;
  const wide = cols >= 4;
  const cell = (c: string, tag: "th" | "td", i: number) => {
    const a = aligns[i] ? ` style="text-align:${aligns[i]}"` : "";
    return `<${tag}${a}>${inline(c, blocks)}</${tag}>`;
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
  // display math spans lines — pull before line splitting (\[…\] and $$…$$)
  const dmath: string[] = [];
  src = src
    .replace(/\$\$([\s\S]+?)\$\$/g, (_m, tex) => {
      dmath.push(renderMath(tex, true));
      return "\n\x03" + (dmath.length - 1) + "\x03\n";
    })
    .replace(/\\\[([\s\S]+?)\\\]/g, (_m, tex) => {
      dmath.push(renderMath(tex, true));
      return "\n\x03" + (dmath.length - 1) + "\x03\n";
    });
  // dsh secret rule: private key blocks span lines — pull them out before line splitting
  const secrets: string[] = [];
  src = src.replace(KEYBLOCK_RE, (m) => {
    secrets.push(secretBlock(m));
    return "\x02" + (secrets.length - 1) + "\x02";
  });
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
      // dsh: ```math fences render as display TeX, not as a code card;
      // ```diff/patch render as a review card with gutters and stats
      html += /^math$/i.test(lang)
        ? `<div class="md-math">${renderMath(code, true)}</div>`
        : /^(diff|patch)$/i.test(lang)
          ? diffBlock(code)
          : codeBlock(lang, code);
      continue;
    }
    const dm = t.match(/^\x03(\d+)\x03$/);
    if (dm) {
      flushPara();
      closeList();
      closeQuote();
      html += `<div class="md-math">${dmath[+dm[1]] ?? ""}</div>`;
      continue;
    }
    const h = t.match(/^(#{1,6})\s+(.*)$/);
    if (h) {
      flushPara();
      closeList();
      closeQuote();
      const n = h[1].length;
      html += `<h${n}>${inline(h[2], secrets)}</h${n}>`;
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
      html += inline(text, secrets) + "<br>";
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
      html += tableHtml(rows, aligns, secrets);
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
        item = (cb[1] === " " ? "&#9744; " : "&#9745; ") + inline(cb[2], secrets);
      } else {
        item = inline(item, secrets);
      }
      html += "<li>" + item + "</li>";
      continue;
    }
    if (!t) {
      flushPara();
      closeList();
      continue;
    }
    para.push(inline(l, secrets));
  }
  flushPara();
  closeList();
  closeQuote();
  return html;
}

// delegated clicks for the generated per-block controls (toolbar, secret eye, links);
// shared with plain-text renderers that embed the same markup (user bubbles)
export function mdClickHandler(e: React.MouseEvent<HTMLElement>) {
  const target = e.target as HTMLElement;
  const imgTile = target.closest(".md-img");
  if (imgTile) {
    openLightbox(imgTile.getAttribute("data-src") || "", imgTile.getAttribute("data-alt") || "");
    return;
  }
  const eye = target.closest("button.md-eye");
  if (eye) {
    const box = eye.closest(".md-secret");
    const val = box?.querySelector(".md-secret-value");
    if (box && val) {
      const revealed = box.getAttribute("data-revealed") === "1";
      if (revealed) {
        val.innerHTML = box.getAttribute("data-masked-html") || "";
        box.setAttribute("data-revealed", "0");
        eye.setAttribute("aria-pressed", "false");
      } else {
        val.textContent = box.getAttribute("data-raw") || "";
        box.setAttribute("data-revealed", "1");
        eye.setAttribute("aria-pressed", "true");
      }
    }
    return;
  }
  const anchor = target.closest("a");
  if (anchor) {
    const href = anchor.getAttribute("href") || "";
    if (/^https?:/.test(href)) {
      e.preventDefault();
      invoke("open_url", { url: href }).catch(() => {});
    }
    return;
  }
  const wrapBtn = target.closest("button.md-wrap");
  if (wrapBtn) {
    const card = wrapBtn.closest(".md-code-card");
    const pre = card?.querySelector("pre");
    if (pre) {
      const wrapped = pre.getAttribute("data-wrap") !== "false";
      pre.setAttribute("data-wrap", wrapped ? "false" : "true");
      wrapBtn.setAttribute("aria-pressed", wrapped ? "false" : "true");
      repaintMinimap(card);
    }
    return;
  }
  const btn = target.closest("button.md-copy");
  if (!btn) return;
  const raw = btn.getAttribute("data-code");
  const code = raw !== null ? raw : btn.closest(".md-code-card")?.querySelector("code")?.textContent || "";
  void copyText(code).then((ok) => {
    if (!ok) return;
    btn.classList.add("copied");
    setTimeout(() => btn.classList.remove("copied"), 1400);
  });
}

// dsh open-in-app url menu: open externally or copy the link
export function mdContextMenu(e: React.MouseEvent<HTMLElement>) {
  const imgTile = (e.target as HTMLElement).closest(".md-img");
  if (imgTile) {
    const src = imgTile.getAttribute("data-src") || "";
    if (!/^https?:/.test(src)) return;
    openContextMenu(e, [
      { id: "open-img", label: "Open Image", icon: "globe", onSelect: () => invoke("open_url", { url: src }).catch(() => {}) },
      {
        id: "copy-img",
        label: "Copy Image Address",
        icon: "link",
        onSelect: () => {
          void copyText(src).then((ok) => ok && useStore.getState().toast("copied"));
        },
      },
    ]);
    return;
  }
  const anchor = (e.target as HTMLElement).closest("a");
  if (!anchor) return;
  const href = anchor.getAttribute("href") || "";
  if (!/^https?:/.test(href)) return;
  openContextMenu(e, [
    { id: "open", label: "Open Link", icon: "globe", onSelect: () => invoke("open_url", { url: href }).catch(() => {}) },
    {
      id: "copy",
      label: "Copy Link",
      icon: "link",
      onSelect: () => {
        void copyText(href).then((ok) => ok && useStore.getState().toast("copied"));
      },
    },
  ]);
}

export function Md({ text, className }: { text: string; className?: string }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    // fresh innerHTML — repaint overview strips and bind their scroll helpers
    paintMinimaps(ref.current);
  }, [text]);
  return <div ref={ref} className={"md " + (className || "")} onClick={mdClickHandler} onContextMenu={mdContextMenu} dangerouslySetInnerHTML={{ __html: md(text) }} />;
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
