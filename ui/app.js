const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);

let CFG = null;
let KEYS = {};
let MODEL = "";
let KIND = "openai";
let waiting = false;
let confirmOpen = false;
let askOpen = false;
let askState = null;
let settingsOpen = false;
let filesOpen = false;
let models = [];
let SID = "";
let SESSIONS = [];
let CWD = "";
let filesPath = "";
let filesParent = null;
let PLAN = false;
let CTX = { used: 0, limit: 0 };
let CHANGES = {};
let reviewOpen = false;
let reviewExpanded = new Set();
let paletteOpen = false;
let paletteItems = [];
let paletteIdx = 0;

let streamRaw = null;
let streamBody = null;
let renderTimer = null;
let think = null;
let tokens = { in: 0, out: 0, cost: 0 };

/* icons */

const ICONS = {
  spark: '<path d="M12 2l2.4 7.2L22 12l-7.6 2.8L12 22l-2.4-7.2L2 12l7.6-2.8z"/>',
  menu: '<path d="M4 6h16"/><path d="M4 12h16"/><path d="M4 18h16"/>',
  plus: '<path d="M12 5v14"/><path d="M5 12h14"/>',
  sliders: '<path d="M21 4h-7"/><path d="M10 4H3"/><path d="M21 12h-9"/><path d="M8 12H3"/><path d="M21 20h-5"/><path d="M12 20H3"/><path d="M14 2v4"/><path d="M8 10v4"/><path d="M16 18v4"/>',
  sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2v2"/><path d="M12 20v2"/><path d="M4.93 4.93l1.41 1.41"/><path d="M17.66 17.66l1.41 1.41"/><path d="M2 12h2"/><path d="M20 12h2"/><path d="M6.34 17.66l-1.41 1.41"/><path d="M19.07 4.93l-1.41 1.41"/>',
  moon: '<path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/>',
  paperclip: '<path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l8.57-8.57A4 4 0 1 1 18 8.84l-8.59 8.57a2 2 0 0 1-2.83-2.83l8.49-8.48"/>',
  send: '<path d="M12 19V5"/><path d="M5 12l7-7 7 7"/>',
  undo: '<path d="M9 14L4 9l5-5"/><path d="M4 9h10.5a5.5 5.5 0 0 1 0 11H11"/>',
  redo: '<path d="M15 14l5-5-5-5"/><path d="M20 9H9.5a5.5 5.5 0 0 0 0 11H13"/>',
  diff: '<path d="M12 3v6"/><path d="M9 6h6"/><path d="M12 21v-6"/><path d="M9 18h6"/><path d="M5 9.5v5a1.5 1.5 0 0 1 -1.5 1.5"/><path d="M19 14.5v-5A1.5 1.5 0 0 1 20.5 8"/>',
  command: '<path d="M9 9V6a3 3 0 1 0 -3 3h3zm0 0v6m0-6h6m-6 6H6a3 3 0 1 0 3 3v-3zm6-6h3a3 3 0 1 0 -3-3v3zm0 0v6m0 0h3a3 3 0 1 1 -3 3v-3z"/>',
  trash: '<path d="M3 6h18"/><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/>',
  folder: '<path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/>',
  file: '<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><path d="M14 2v6h6"/>',
  x: '<path d="M18 6L6 18"/><path d="M6 6l12 12"/>',
  up: '<path d="M12 19V5"/><path d="M5 12l7-7 7 7"/>',
  check: '<path d="M20 6L9 17l-5-5"/>',
};

function icon(name) {
  return `<svg viewBox="0 0 24 24">${ICONS[name] || ""}</svg>`;
}

document.querySelectorAll(".ic[data-icon]").forEach((n) => {
  n.innerHTML = icon(n.dataset.icon);
});

/* keys config */

const KEY_ACTIONS = [
  ["send", "send message"],
  ["newline", "insert newline"],
  ["stop", "stop generation"],
  ["new_session", "new session"],
  ["open_settings", "open settings"],
  ["toggle_sidebar", "toggle sidebar"],
  ["toggle_theme", "toggle theme"],
  ["toggle_plan", "toggle plan mode"],
  ["undo", "undo file changes"],
  ["redo", "redo file changes"],
  ["toggle_thinking", "toggle thinking"],
  ["palette", "command palette"],
];

const DEFAULT_KEYS = Object.fromEntries(KEY_ACTIONS.map(([k]) => [k, ""]));
Object.assign(DEFAULT_KEYS, {
  send: "enter",
  newline: "shift+enter",
  stop: "escape",
  new_session: "ctrl+n",
  open_settings: "ctrl+comma",
  toggle_sidebar: "ctrl+b",
  toggle_theme: "ctrl+shift+t",
  toggle_plan: "ctrl+shift+p",
  undo: "ctrl+z",
  redo: "ctrl+shift+z",
  toggle_thinking: "ctrl+t",
  palette: "ctrl+k",
});

/* theme */

function setTheme(name, persist) {
  const t = name === "light" ? "light" : "dark";
  document.body.dataset.theme = t;
  $("btn-theme").innerHTML = icon(t === "light" ? "moon" : "sun");
  if (persist) invoke("set_theme", { theme: t }).catch(() => {});
}

function toggleTheme() {
  setTheme(document.body.dataset.theme === "light" ? "dark" : "light", true);
}

function togglePlan() {
  PLAN = !PLAN;
  applyPlan();
  applyStatus();
  invoke("set_plan", { on: PLAN }).catch(() => {});
  note(PLAN ? "plan mode on: read-only research" : "plan mode off");
}

/* sidebar */

function toggleSidebar() {
  const sb = $("sidebar");
  sb.classList.toggle("hidden");
  localStorage.setItem("hiderola.sidebar", sb.classList.contains("hidden") ? "0" : "1");
}

function fmtRel(ts) {
  if (!ts) return "";
  const d = Date.now() / 1000 - ts;
  if (d < 60) return "now";
  if (d < 3600) return Math.floor(d / 60) + "m";
  if (d < 86400) return Math.floor(d / 3600) + "h";
  if (d < 7 * 86400) return Math.floor(d / 86400) + "d";
  const dt = new Date(ts * 1000);
  return dt.toISOString().slice(0, 10);
}

function renderSessions() {
  const box = $("session-list");
  box.replaceChildren();
  if (!SESSIONS.length) {
    box.appendChild(el("div", "sess-empty", "no history yet"));
    return;
  }
  for (const s of SESSIONS) {
    const item = el("div", "sess" + (s.id === SID ? " active" : ""));
    const main = el("div", "sess-main");
    main.appendChild(el("div", "sess-title", s.title || "new chat"));
    const meta = `${s.count} msgs · ${fmtRel(s.updated)}` + (s.cost > 0 ? ` · ${fmtCost(s.cost)}` : "");
    main.appendChild(el("div", "sess-meta", meta));
    item.appendChild(main);
    const del = el("button", "icon-btn sess-del");
    del.innerHTML = icon("trash");
    del.title = "delete";
    del.onclick = (e) => {
      e.stopPropagation();
      if (del.classList.contains("confirm")) {
        invoke("delete_session", { id: s.id }).catch((err) => note(String(err)));
      } else {
        del.classList.add("confirm");
        del.innerHTML = icon("check");
        setTimeout(() => {
          del.classList.remove("confirm");
          del.innerHTML = icon("trash");
        }, 2500);
      }
    };
    item.appendChild(del);
    item.onclick = () => openSession(s.id);
    box.appendChild(item);
  }
}

async function refreshSessions() {
  SESSIONS = await invoke("list_sessions").catch(() => []);
  renderSessions();
}

async function openSession(id) {
  if (waiting || confirmOpen) return;
  let st;
  try {
    st = await invoke("open_session", { id });
  } catch (e) {
    note(String(e));
    return;
  }
  SID = st.id;
  tokens = { in: st.tokens_in || 0, out: st.tokens_out || 0, cost: st.cost || 0 };
  CTX.used = 0;
  CHANGES = {};
  reviewExpanded.clear();
  for (const c of st.changes || []) {
    CHANGES[c.path] = { adds: c.adds, dels: c.dels, rows: null };
  }
  renderReviewBadge();
  $("chat-col").replaceChildren();
  streamRaw = null;
  streamBody = null;
  think = null;
  for (const it of st.transcript) {
    if (it.k === "user") {
      const m = el("div", "msg you");
      m.appendChild(el("div", "who", "you"));
      m.appendChild(el("div", "body", it.s));
      $("chat-col").appendChild(m);
    } else if (it.k === "bot") {
      const m = el("div", "msg bot");
      m.appendChild(el("div", "who", "bot"));
      const b = el("div", "body md");
      b.innerHTML = md(it.s);
      m.appendChild(b);
      $("chat-col").appendChild(m);
    } else if (it.k === "tool") {
      const i = it.s.indexOf(" ");
      const name = i < 0 ? it.s : it.s.slice(0, i);
      const m = addMsg("tool");
      m.appendChild(el("div", "thead", `${name} ${i < 0 ? "" : it.s.slice(i + 1, i + 81)}`));
    } else if (it.k === "toolout") {
      const t = el("div", "toolout", it.s);
      t.onclick = () => t.classList.toggle("open");
      $("chat-col").appendChild(t);
    }
  }
  if (st.transcript.length) autoscroll();
  if ((st.queue || []).length) {
    note(`${st.queue.length} queued message(s) restored from the previous run — they will steer the next run`);
  }
  renderSessions();
}

function newChat() {
  if (waiting || confirmOpen) return;
  invoke("new_session").catch((e) => note(String(e)));
}

const WELCOME_HTML = $("welcome") ? $("welcome").outerHTML : "";

function clearChat() {
  $("chat-col").innerHTML = WELCOME_HTML;
  $("chat-col").querySelectorAll(".ic[data-icon]").forEach((n) => {
    n.innerHTML = icon(n.dataset.icon);
  });
  streamRaw = null;
  streamBody = null;
  think = null;
  tokens = { in: 0, out: 0, cost: 0 };
  applyStatus();
}

/* helpers */

function fmtTokens(n) {
  return n < 1000 ? String(n) : (n / 1000).toFixed(1) + "k";
}

function fmtCost(c) {
  return "$" + (c >= 1 ? c.toFixed(2) : c.toFixed(4));
}

function applyStatus() {
  $("status-left").textContent = `${KIND} · ${MODEL}${PLAN ? " · plan" : ""}`;
  const right = $("status-right");
  if (waiting) {
    right.textContent = "thinking...";
    right.classList.add("busy");
  } else {
    right.classList.remove("busy");
    let s = "";
    if (tokens.in || tokens.out) {
      s = `${fmtTokens(tokens.in)} in · ${fmtTokens(tokens.out)} out`;
      if (tokens.cached > 0) s += ` (${fmtTokens(tokens.cached)} cached)`;
    }
    if (tokens.cost > 0) s += (s ? " · " : "") + fmtCost(tokens.cost);
    right.textContent = s;
  }
  const pill = $("ctx-pill");
  if (CTX.limit > 0 && CTX.used > 0) {
    const pct = Math.min(100, Math.round((CTX.used / CTX.limit) * 100));
    pill.textContent = pct + "% ctx";
    pill.title = `context: ${fmtTokens(CTX.used)} of ${fmtTokens(CTX.limit)} tokens · click to /compact`;
    pill.classList.remove("hidden", "warn", "crit");
    if (pct >= 92) pill.classList.add("crit");
    else if (pct >= 80) pill.classList.add("warn");
  } else {
    pill.classList.add("hidden");
  }
}

function autoscroll() {
  const chat = $("chat");
  chat.scrollTop = chat.scrollHeight;
}

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

function clearWelcome() {
  const w = $("welcome");
  if (w) w.remove();
}

function addMsg(cls, text) {
  clearWelcome();
  const m = el("div", "msg " + cls);
  if (text !== undefined) m.textContent = text;
  $("chat-col").appendChild(m);
  autoscroll();
  return m;
}

const note = (s) => addMsg("note", s);

/* markdown */

function esc(s) {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function inline(s) {
  return s
    .replace(/`([^`\n]+)`/g, "<code>$1</code>")
    .replace(/\*\*([^*]+)\*\*/g, "<b>$1</b>")
    .replace(/(^|[\s(])\*([^*\n]+)\*/g, "$1<i>$2</i>")
    .replace(/\[([^\]]+)\]\((https?:[^)\s]+)\)/g, '<a href="$2" target="_blank" rel="noreferrer">$1</a>');
}

function md(src) {
  const blocks = [];
  src = src.replace(/```(\w*)[^\S\n]*\n?([\s\S]*?)(```|$)/g, (m, lang, code) => {
    blocks.push([lang, code.replace(/\n$/, "")]);
    return "\x00" + (blocks.length - 1) + "\x00";
  });
  src = esc(src);
  const lines = src.split("\n");
  let html = "";
  let para = [];
  let list = null;
  let quote = null;
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
  for (const raw of lines) {
    const l = raw;
    const t = l.trim();
    const bm = t.match(/^\x00(\d+)\x00$/);
    if (bm) {
      flushPara();
      closeList();
      closeQuote();
      const [lang, code] = blocks[+bm[1]];
      html += `<pre><code${lang ? ` class="lang-${esc(lang)}"` : ""}>${esc(code)}</code></pre>`;
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
      let item = (ul || ol)[1];
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

/* streaming */

function ensureStream() {
  clearWelcome();
  if (!streamBody) {
    const m = el("div", "msg bot");
    m.appendChild(el("div", "who", "bot"));
    streamBody = el("div", "body md");
    m.appendChild(streamBody);
    $("chat-col").appendChild(m);
  }
  autoscroll();
}

function renderStream() {
  if (streamBody) streamBody.innerHTML = md(streamRaw);
}

function ensureThink() {
  if (!think) {
    const box = el("div", "think open streaming");
    const head = el("div", "think-head", "thinking ");
    head.appendChild(el("span", "chev", "▾"));
    const body = el("div", "think-body");
    head.onclick = () => box.classList.toggle("open");
    box.appendChild(head);
    box.appendChild(body);
    $("chat-col").appendChild(box);
    think = { box, body };
    autoscroll();
  }
  return think;
}

function closeThink() {
  if (think) {
    think.box.classList.remove("streaming", "open");
    think = null;
  }
}

function addDiff(container, rows) {
  if (!rows || !rows.length) return;
  const pre = el("pre", "diff");
  for (const r of rows) {
    const cls = r.tag === 1 ? "add" : r.tag === 2 ? "del" : "ctx";
    const sign = r.tag === 1 ? "+ " : r.tag === 2 ? "- " : "  ";
    pre.appendChild(el("span", cls, sign + r.text));
    pre.appendChild(document.createTextNode("\n"));
  }
  container.appendChild(pre);
  autoscroll();
}

/* attachments */

function chipName(p) {
  const base = p.replace(/\s*\(\d+ entries?\)\s*$/, "").replace(/[\\/]+$/, "");
  return base.split(/[\\/]/).pop() || p;
}

function renderChips(list) {
  const box = $("chips-inner");
  box.replaceChildren();
  list.forEach((p, i) => {
    const c = el("div", "chip");
    c.innerHTML = icon("file");
    c.appendChild(el("span", "chip-name", chipName(p)));
    const rm = el("button", "icon-btn");
    rm.innerHTML = icon("x");
    rm.title = "remove";
    rm.onclick = () => invoke("detach", { index: i });
    c.appendChild(rm);
    c.title = p;
    box.appendChild(c);
  });
}

/* file browser */

async function browse(path) {
  let d;
  try {
    d = await invoke("list_dir", { path: path || null });
  } catch (e) {
    note(String(e));
    return;
  }
  filesPath = d.path;
  filesParent = d.parent;
  $("files-crumb").textContent = d.path + (d.is_cwd ? "  ·  cwd" : "");
  const list = $("files-list");
  list.replaceChildren();
  if (!d.entries.length) list.appendChild(el("div", "sess-empty", "empty folder"));
  for (const e of d.entries) {
    const row = el("div", "frow");
    row.innerHTML = `<span class="ic ${e.dir ? "dir-ic" : "file-ic"}">${icon(e.dir ? "folder" : "file")}</span>`;
    row.appendChild(el("span", "fname", e.dir ? e.name + "/" : e.name));
    row.appendChild(el("span", "fsize", e.dir ? "" : fmtTokens(e.size) + "b"));
    row.onclick = async () => {
      if (e.dir) {
        browse(joinPath(filesPath, e.name));
      } else {
        await attach(joinPath(filesPath, e.name));
        closeFiles();
      }
    };
    list.appendChild(row);
  }
}

function joinPath(dir, name) {
  if (dir === "~") dir = "~/";
  return dir.endsWith("/") || dir.endsWith("\\") ? dir + name : dir + "/" + name;
}

async function attach(path) {
  try {
    await invoke("attach_path", { path });
  } catch (e) {
    note(String(e));
  }
}

function openFiles() {
  filesOpen = true;
  $("files-overlay").classList.remove("hidden");
  browse(CWD || null);
}

function closeFiles() {
  filesOpen = false;
  $("files-overlay").classList.add("hidden");
  $("input").focus();
}

/* session review */

function trackChanges(ev) {
  const paths = ev.paths || [];
  if (!paths.length || !ev.diff || !ev.diff.length) return;
  let adds = 0;
  let dels = 0;
  for (const r of ev.diff) {
    if (r.tag === 1) adds++;
    else if (r.tag === 2) dels++;
  }
  if (!adds && !dels) return;
  if (paths.length === 1) {
    const p = paths[0];
    const old = CHANGES[p] || { adds: 0, dels: 0, rows: null };
    CHANGES[p] = { adds: old.adds + adds, dels: old.dels + dels, rows: ev.diff };
  } else {
    for (const p of paths) {
      const old = CHANGES[p] || { adds: 0, dels: 0, rows: null };
      CHANGES[p] = { adds: old.adds + adds, dels: old.dels + dels, rows: null };
    }
    const key = "__patch__" + Date.now() + "_" + Math.random().toString(36).slice(2, 6);
    CHANGES[key] = { adds, dels, rows: ev.diff, label: `${ev.name}: ${ev.detail}` };
  }
  renderReviewBadge();
  if (reviewOpen) renderReviewList();
}

function changesTotals() {
  let adds = 0;
  let dels = 0;
  let files = 0;
  for (const [k, c] of Object.entries(CHANGES)) {
    if (k.startsWith("__patch__")) continue;
    files++;
    adds += c.adds;
    dels += c.dels;
  }
  return { adds, dels, files };
}

function renderReviewBadge() {
  const btn = $("btn-review");
  const badge = $("review-badge");
  const t = changesTotals();
  if (!t.files) {
    btn.classList.add("hidden");
    badge.classList.add("hidden");
    return;
  }
  btn.classList.remove("hidden");
  badge.classList.remove("hidden");
  badge.textContent = t.files;
  badge.classList.toggle("dirty", t.dels > 0);
}

function renderReviewList() {
  const box = $("review-list");
  box.replaceChildren();
  const files = Object.entries(CHANGES).filter(([k]) => !k.startsWith("__patch__"));
  const patches = Object.entries(CHANGES).filter(([k]) => k.startsWith("__patch__"));
  files.sort((a, b) => b[1].adds + b[1].dels - (a[1].adds + a[1].dels));
  const t = changesTotals();
  const pt = { adds: 0, dels: 0 };
  for (const [, c] of patches) {
    pt.adds += c.adds;
    pt.dels += c.dels;
  }
  $("review-total").textContent =
    `${t.files} file${t.files === 1 ? "" : "s"} · +${t.adds} −${t.dels}` +
    (patches.length ? ` · ${patches.length} multi-file patch${patches.length === 1 ? "" : "es"} (+${pt.adds} −${pt.dels})` : "");
  if (!files.length && !patches.length) {
    box.appendChild(el("div", "sess-empty", "no file changes yet"));
    return;
  }
  for (const [path, c] of files) {
    const item = el("div", "rev-item");
    const head = el("div", "rev-head");
    head.innerHTML = `<span class="ic file-ic">${icon("file")}</span>`;
    const name = el("span", "rev-path");
    const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
    if (slash > 0) {
      name.appendChild(el("span", "rev-dir", path.slice(0, slash + 1)));
    }
    name.appendChild(el("span", null, slash > 0 ? path.slice(slash + 1) : path));
    head.appendChild(name);
    const stat = el("span", "rev-stat");
    if (c.adds) stat.appendChild(el("span", "adds", "+" + c.adds));
    if (c.dels) stat.appendChild(el("span", "dels", "−" + c.dels));
    if (!c.rows) stat.appendChild(el("span", "hint", " (counts only)"));
    head.appendChild(stat);
    item.appendChild(head);
    if (c.rows) {
      const body = el("div", "rev-body");
      body.appendChild(buildDiffPre(c.rows));
      item.appendChild(body);
      head.classList.add("expandable");
      head.onclick = () => {
        if (reviewExpanded.has(path)) reviewExpanded.delete(path);
        else reviewExpanded.add(path);
        renderReviewList();
      };
      body.classList.toggle("open", reviewExpanded.has(path));
    }
    box.appendChild(item);
  }
  for (const [key, c] of patches) {
    const item = el("div", "rev-item");
    const head = el("div", "rev-head");
    head.appendChild(el("span", "rev-path", c.label || "patch"));
    const stat = el("span", "rev-stat");
    if (c.adds) stat.appendChild(el("span", "adds", "+" + c.adds));
    if (c.dels) stat.appendChild(el("span", "dels", "−" + c.dels));
    head.appendChild(stat);
    item.appendChild(head);
    const body = el("div", "rev-body");
    body.appendChild(buildDiffPre(c.rows));
    item.appendChild(body);
    if (c.rows) {
      head.classList.add("expandable");
      const pkey = key;
      head.onclick = () => {
        if (reviewExpanded.has(pkey)) reviewExpanded.delete(pkey);
        else reviewExpanded.add(pkey);
        renderReviewList();
      };
      body.classList.toggle("open", reviewExpanded.has(key));
    }
    box.appendChild(item);
  }
}

function buildDiffPre(rows) {
  const pre = el("pre", "diff");
  for (const r of rows) {
    const cls = r.tag === 1 ? "add" : r.tag === 2 ? "del" : "ctx";
    const sign = r.tag === 1 ? "+ " : r.tag === 2 ? "- " : "  ";
    pre.appendChild(el("span", cls, sign + r.text));
    pre.appendChild(document.createTextNode("\n"));
  }
  return pre;
}

function openReview() {
  reviewOpen = true;
  renderReviewList();
  $("review-overlay").classList.remove("hidden");
}

function closeReview() {
  reviewOpen = false;
  $("review-overlay").classList.add("hidden");
  $("input").focus();
}

/* command palette */

function paletteBuildItems() {
  const items = [];
  const cmds = [
    ["/help", "show help", "commands help"],
    ["/new", "new chat", "clear session"],
    ["/compact", "compact context", "summarize shrink tokens"],
    ["/plan", "toggle plan mode", "read-only research"],
    ["/undo", "undo file changes", "revert"],
    ["/redo", "redo file changes", "reapply"],
    ["/init", "write AGENTS.md", "init project"],
    ["/export", "export session to markdown", "save transcript"],
    ["/models", "list provider models", "fetch"],
  ];
  for (const [cmd, label, hint] of cmds) {
    items.push({ icon: "command", label, hint, run: () => runCmd(cmd) });
  }
  items.push({ icon: "sun", label: "toggle theme", hint: "dark / light", run: toggleTheme });
  items.push({ icon: "sliders", label: "open settings", hint: "provider, hotkeys, mcp, permissions", run: openSettings });
  items.push({ icon: "diff", label: "review session changes", hint: "changed files and diffs", run: openReview });
  items.push({ icon: "menu", label: "toggle sidebar", hint: "history panel", run: toggleSidebar });
  for (const s of SESSIONS.slice(0, 15)) {
    items.push({
      icon: "spark",
      label: s.title || "new chat",
      hint: `${s.count} msgs · ${fmtRel(s.updated)}`,
      session: true,
      run: () => openSession(s.id),
    });
  }
  for (const m of models) {
    if (m === MODEL) continue;
    items.push({ icon: "file", label: "model: " + m, hint: "switch model", run: () => runCmd("/model " + m) });
  }
  return items;
}

function paletteFilter(q) {
  const query = q.trim().toLowerCase();
  const all = paletteBuildItems();
  if (!query) return all.slice(0, 30);
  const scored = [];
  for (const it of all) {
    const l = it.label.toLowerCase();
    const idx = l.indexOf(query);
    if (idx === 0) scored.push([0, it]);
    else if (idx > 0) scored.push([1, it]);
    else if (it.hint && it.hint.toLowerCase().includes(query)) scored.push([2, it]);
  }
  scored.sort((a, b) => a[0] - b[0]);
  return scored.slice(0, 30).map(([, it]) => it);
}

function renderPalette() {
  const box = $("palette-list");
  box.replaceChildren();
  if (!paletteItems.length) {
    box.appendChild(el("div", "sess-empty", "nothing matches"));
    return;
  }
  paletteItems.forEach((it, i) => {
    const row = el("div", "prow" + (i === paletteIdx ? " active" : ""));
    row.innerHTML = `<span class="ic">${icon(it.icon)}</span>`;
    row.appendChild(el("span", "prow-label", it.label));
    if (it.hint) row.appendChild(el("span", "prow-hint", it.hint));
    row.onmousedown = (e) => {
      e.preventDefault();
      paletteIdx = i;
      runPalette();
    };
    box.appendChild(row);
  });
  const act = box.children[paletteIdx];
  if (act) act.scrollIntoView({ block: "nearest" });
}

function openPalette() {
  if (waiting || confirmOpen || askOpen || settingsOpen || filesOpen) return;
  paletteOpen = true;
  paletteIdx = 0;
  $("palette-input").value = "";
  paletteItems = paletteFilter("");
  renderPalette();
  $("palette-overlay").classList.remove("hidden");
  $("palette-input").focus();
}

function closePalette() {
  paletteOpen = false;
  $("palette-overlay").classList.add("hidden");
  $("input").focus();
}

function runPalette() {
  const it = paletteItems[paletteIdx];
  if (!it) return;
  closePalette();
  it.run();
}

/* events */

async function handleEvent(ev) {
  switch (ev.t) {
    case "chunk": {
      if (streamRaw === null) {
        streamRaw = "";
        ensureStream();
      }
      streamRaw += ev.s;
      if (!renderTimer) {
        renderTimer = setTimeout(() => {
          renderTimer = null;
          renderStream();
        }, 90);
      }
      break;
    }
    case "reasoning": {
      const t = ensureThink();
      t.body.textContent += ev.s;
      t.body.scrollTop = t.body.scrollHeight;
      break;
    }
    case "note": {
      if (ev.s) {
        if (streamRaw !== null) {
          streamRaw = null;
          streamBody = null;
        }
        note(ev.s);
        const m = ev.s.match(/task (bg-\d+) (finished|failed|was killed|killed)/);
        if (m) endBg(m[1]);
      }
      break;
    }
    case "tool": {
      closeThink();
      const m = addMsg("tool");
      const head = el("div", "thead", `${ev.name} ${ev.detail}`);
      m.appendChild(head);
      addDiff(m, ev.diff);
      trackChanges(ev);
      break;
    }
    case "confirm": {
      closeThink();
      confirmOpen = true;
      $("confirm-title").textContent = "run " + ev.name + "?";
      $("confirm-detail").textContent = ev.detail;
      $("confirm-diff").replaceChildren();
      addDiff($("confirm-diff"), ev.diff);
      $("c-feedback").value = "";
      $("confirm-overlay").classList.remove("hidden");
      $("input").blur();
      break;
    }
    case "ask":
      closeThink();
      openAsk(ev.args);
      break;
    case "todo": {
      closeThink();
      renderTodoCard(ev.s || "");
      break;
    }
    case "bgout":
      bgAppend(ev.id, ev.s);
      break;
    case "plan":
      PLAN = !!ev.on;
      applyPlan();
      applyStatus();
      break;
    case "usage":
      tokens.in += ev.input;
      tokens.out += ev.output;
      tokens.cached = (tokens.cached || 0) + (ev.cached || 0);
      if (typeof ev.cost === "number") tokens.cost = ev.cost;
      if (typeof ev.ctx_used === "number") CTX.used = ev.ctx_used;
      if (typeof ev.ctx_limit === "number" && ev.ctx_limit > 0) CTX.limit = ev.ctx_limit;
      applyStatus();
      break;
    case "attachments":
      renderChips(ev.list || []);
      break;
    case "sessions":
      SESSIONS = ev.list || [];
      if (ev.sid) SID = ev.sid;
      renderSessions();
      break;
    case "cleared":
      SID = "";
      CTX.used = 0;
      CHANGES = {};
      reviewExpanded.clear();
      renderReviewBadge();
      clearChat();
      break;
    case "done": {
      if (renderTimer) {
        clearTimeout(renderTimer);
        renderTimer = null;
      }
      closeThink();
      if (streamBody) {
        streamBody.innerHTML = md(ev.text);
      } else if (ev.text) {
        clearWelcome();
        const m = el("div", "msg bot");
        m.appendChild(el("div", "who", "bot"));
        const b = el("div", "body md");
        b.innerHTML = md(ev.text);
        m.appendChild(b);
        $("chat-col").appendChild(m);
      }
      streamRaw = null;
      streamBody = null;
      waiting = false;
      applyStatus();
      autoscroll();
      break;
    }
    case "failed":
      closeThink();
      addMsg("error", ev.s);
      waiting = false;
      applyStatus();
      break;
    case "idle":
      waiting = false;
      applyStatus();
      break;
    case "queued":
      waiting = true;
      applyStatus();
      break;
    case "model":
      MODEL = ev.name;
      KIND = ev.kind || KIND;
      fillModelSelect();
      applyStatus();
      break;
    case "theme":
      setTheme(ev.name, false);
      break;
  }
}

/* sending */

async function doSend() {
  const input = $("input");
  const text = input.value.trim();
  if (!text || confirmOpen || askOpen || settingsOpen || filesOpen) return;
  hideMention();
  input.value = "";
  autosize();
  clearWelcome();
  const m = el("div", "msg you");
  m.appendChild(el("div", "who", "you"));
  m.appendChild(el("div", "body", text));
  $("chat-col").appendChild(m);
  autoscroll();
  waiting = true;
  applyStatus();
  let res;
  try {
    res = await invoke("send", { text });
  } catch (e) {
    m.remove();
    waiting = false;
    applyStatus();
    note(String(e));
    if (String(e).includes("api key")) openSettings();
    return;
  }
  if (res.cmd) {
    m.remove();
    waiting = false;
    applyStatus();
    if (res.note) note(res.note);
  }
}

async function runCmd(text) {
  try {
    const res = await invoke("send", { text });
    if (res && res.note) note(res.note);
  } catch (e) {
    note(String(e));
  }
}

/* model select */

function fillModelSelect() {
  const sel = $("model-select");
  sel.replaceChildren();
  const list = [MODEL, ...models.filter((m) => m !== MODEL)];
  for (const m of list) {
    if (!m) continue;
    sel.appendChild(new Option(m, m));
  }
  sel.value = MODEL;
}

$("model-select").onchange = async (e) => {
  const v = e.target.value;
  if (v && v !== MODEL) runCmd("/model " + v);
};

/* confirm */

function resolveConfirm(ok, always = false) {
  confirmOpen = false;
  $("confirm-overlay").classList.add("hidden");
  const feedback = ok ? "" : $("c-feedback").value.trim();
  invoke("confirm", { ok, feedback, always });
  waiting = true;
  applyStatus();
  if (!ok) note(feedback ? "rejected with feedback" : "denied");
  if (ok && always) note("always allowed: rule saved to config");
  $("input").focus();
}

function allowAll() {
  confirmOpen = false;
  $("confirm-overlay").classList.add("hidden");
  invoke("allow_all");
  waiting = true;
  applyStatus();
  $("input").focus();
}

$("c-run").onclick = () => resolveConfirm(true);
$("c-deny").onclick = () => resolveConfirm(false);
$("c-always").onclick = () => resolveConfirm(true, true);
$("c-allow").onclick = () => allowAll();
$("c-feedback").onkeydown = (e) => {
  if (e.key === "Enter") {
    e.preventDefault();
    resolveConfirm(false);
  }
};

/* todo card */

function renderTodoCard(text) {
  const old = document.querySelector("#chat-col .todo-card");
  if (old) old.remove();
  if (!text || text === "(todo list is empty)") return;
  clearWelcome();
  const m = el("div", "msg todo-card");
  m.appendChild(el("div", "who", "todos"));
  m.appendChild(el("pre", "todo-body", text));
  $("chat-col").appendChild(m);
  autoscroll();
}

/* background task live output */

function bgAppend(id, chunk) {
  let card = document.querySelector(`#chat-col .bg-live[data-id="${id}"]`);
  if (!card) {
    closeThink();
    clearWelcome();
    card = el("div", "msg bg-live");
    card.dataset.id = id;
    const head = el("div", "thead");
    head.appendChild(el("span", null, id + " output"));
    const kill = el("button", "bg-kill", "kill");
    kill.title = "stop this background task";
    kill.onclick = () => invoke("task_kill", { id });
    head.appendChild(kill);
    card.appendChild(head);
    card.appendChild(el("pre", "bg-out", ""));
    $("chat-col").appendChild(card);
  }
  const pre = card.querySelector(".bg-out");
  pre.textContent += chunk;
  if (pre.textContent.length > 24000) pre.textContent = pre.textContent.slice(-18000);
  pre.scrollTop = pre.scrollHeight;
  autoscroll();
}

function endBg(id) {
  const card = document.querySelector(`#chat-col .bg-live[data-id="${id}"]`);
  if (!card) return;
  card.classList.add("ended");
  const k = card.querySelector(".bg-kill");
  if (k) k.remove();
}

/* plan mode */

function applyPlan() {
  $("btn-plan").classList.toggle("on", PLAN);
}

/* ask */

function openAsk(args) {
  let qs = [];
  try {
    qs = JSON.parse(args).questions || [];
  } catch {}
  if (!qs.length) {
    invoke("answer", { text: "" });
    return;
  }
  askOpen = true;
  askState = { qs, sel: qs.map(() => new Set()), custom: qs.map(() => "") };
  const body = $("ask-body");
  body.replaceChildren();
  qs.forEach((q, i) => {
    const wrap = el("div", "ask-q");
    const header = (q.header || "").trim();
    if (header) wrap.appendChild(el("div", "q-header", header));
    wrap.appendChild(el("div", "q-text", q.question || ""));
    const opts = el("div", "ask-opts");
    (q.options || []).forEach((o, j) => {
      const b = el("button", "ask-opt");
      b.type = "button";
      b.appendChild(el("span", "o-label", o.label || ""));
      if (o.description) b.appendChild(el("span", "o-desc", o.description));
      b.onclick = () => {
        const set = askState.sel[i];
        if (q.multiple) {
          set.has(j) ? set.delete(j) : set.add(j);
          b.classList.toggle("sel", set.has(j));
        } else {
          set.clear();
          set.add(j);
          [...opts.children].forEach((c, k) => c.classList.toggle("sel", k === j));
        }
      };
      opts.appendChild(b);
    });
    wrap.appendChild(opts);
    const inp = document.createElement("input");
    inp.placeholder = "type your own answer";
    inp.spellcheck = false;
    inp.oninput = () => { askState.custom[i] = inp.value; };
    inp.onkeydown = (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        submitAsk();
      }
    };
    wrap.appendChild(inp);
    body.appendChild(wrap);
  });
  $("ask-overlay").classList.remove("hidden");
  $("input").blur();
  const first = body.querySelector("input");
  if (first) first.focus();
}

function composeAsk() {
  const { qs, sel, custom } = askState;
  const lines = [];
  qs.forEach((q, i) => {
    const labels = [...sel[i]].map((j) => ((q.options || [])[j] || {}).label).filter(Boolean);
    const customText = (custom[i] || "").trim();
    let ans = customText || labels.join(", ");
    if (!ans) return;
    const header = (q.header || "").trim();
    lines.push(header ? header + ": " + ans : ans);
  });
  return lines.join("\n");
}

function submitAsk() {
  const text = composeAsk();
  closeAsk();
  invoke("answer", { text });
  note(text ? "answered" : "no answer given");
}

function closeAsk() {
  askOpen = false;
  askState = null;
  $("ask-overlay").classList.add("hidden");
  $("input").focus();
}

function skipAsk() {
  closeAsk();
  invoke("answer", { text: "" });
  note("skipped");
}

$("a-submit").onclick = () => submitAsk();
$("a-skip").onclick = () => skipAsk();

/* settings */

function comboToText(c) {
  if (!c) return "";
  const parts = [];
  if (c.ctrl) parts.push("ctrl");
  if (c.shift) parts.push("shift");
  if (c.alt) parts.push("alt");
  if (c.meta) parts.push("meta");
  parts.push(c.name);
  return parts.join("+");
}

function parseCombo(s) {
  if (!s || s.trim().toLowerCase() === "none") return null;
  let ctrl = false, shift = false, alt = false, meta = false, name = null;
  for (const p of s.split("+")) {
    const l = p.trim().toLowerCase();
    if (!l) continue;
    if (l === "ctrl" || l === "control") ctrl = true;
    else if (l === "shift") shift = true;
    else if (l === "alt") alt = true;
    else if (l === "meta" || l === "cmd" || l === "super") meta = true;
    else name = l === "return" ? "enter" : l === "esc" ? "escape" : l === "comma" ? "," : l;
  }
  return name ? { ctrl, shift, alt, meta, name } : null;
}

function comboFromEvent(e) {
  const k = e.key;
  let name = null;
  if (k === "Enter") name = "enter";
  else if (k === "Escape") name = "escape";
  else if (k === "Tab") name = "tab";
  else if (k === "Backspace") name = "backspace";
  else if (k === " ") name = "space";
  else if (k === "ArrowUp") name = "up";
  else if (k === "ArrowDown") name = "down";
  else if (k === "ArrowLeft") name = "left";
  else if (k === "ArrowRight") name = "right";
  else if (k.length === 1) name = k.toLowerCase();
  else if (/^F\d+$/.test(k)) name = k.toLowerCase();
  else return null;
  return { name, ctrl: e.ctrlKey, shift: e.shiftKey, alt: e.altKey, meta: e.metaKey };
}

function sameCombo(a, b) {
  return a && b && a.name === b.name && a.ctrl === b.ctrl && a.shift === b.shift && a.alt === b.alt && a.meta === b.meta;
}

function buildKeysGrid(keys) {
  const grid = $("keys-grid");
  grid.replaceChildren();
  for (const [action, label] of KEY_ACTIONS) {
    const row = el("div", "krow");
    row.appendChild(el("div", "klabel", label));
    const cap = el("input", "kcap");
    cap.type = "text";
    cap.readOnly = true;
    cap.value = keys[action] || "none";
    cap.dataset.action = action;
    cap.onkeydown = (e) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") {
        cap.value = "none";
        return;
      }
      const c = comboFromEvent(e);
      if (c) cap.value = comboToText(c);
    };
    row.appendChild(cap);
    grid.appendChild(row);
  }
}

function readKeysGrid() {
  const out = {};
  for (const cap of document.querySelectorAll("#keys-grid .kcap")) {
    const v = cap.value.trim();
    out[cap.dataset.action] = v || "none";
  }
  return out;
}

function effectiveKeys(overrides) {
  const out = { ...DEFAULT_KEYS };
  for (const [k, v] of Object.entries(overrides || {})) {
    const s = String(v).trim().toLowerCase();
    if (s === "none") delete out[k];
    else if (s) out[k] = s;
  }
  return out;
}

function fillMcpList() {
  const box = $("s-mcp");
  const list = (CFG && CFG.mcp) || [];
  if (!list.length) {
    box.textContent = "no servers configured (config.toml [[mcp]])";
    box.style.whiteSpace = "pre-wrap";
    return;
  }
  box.replaceChildren();
  for (const m of list) {
    const row = el("div", "mcprow");
    const remote = m.type === "remote" || m.url;
    row.append(el("span", "", {
      text: `${m.name}: ${remote ? "remote " + (m.url || "") : (m.command || "") + " " + (m.args || []).join(" ")}`
    }));
    if (remote) {
      const btn = el("button", "ghost", { text: "auth" });
      btn.style.cssText = "margin-left:8px;padding:0 8px;font-size:11px";
      btn.onclick = async () => {
        btn.disabled = true;
        btn.textContent = "...";
        try {
          const logs = await invoke("mcp_auth", { name: m.name });
          for (const l of logs) note(l);
        } catch (e) {
          note(String(e));
        } finally {
          btn.disabled = false;
          btn.textContent = "auth";
        }
      };
      row.append(btn);
    }
    const resBtn = el("button", "ghost", { text: "res" });
    resBtn.title = "browse resources & prompts";
    resBtn.style.cssText = "margin-left:8px;padding:0 8px;font-size:11px";
    resBtn.onclick = () => toggleMcpRes(m, resBtn, row);
    row.append(resBtn);
    box.append(row);
  }
  box.style.whiteSpace = "";
}

async function toggleMcpRes(m, btn, row) {
  let det = row.nextElementSibling;
  if (det && det.classList.contains("mcpres")) {
    det.style.display = det.style.display === "none" ? "" : "none";
    return;
  }
  btn.disabled = true;
  btn.textContent = "...";
  const itemStyle = "padding:0 8px;font-size:11px;max-width:100%;overflow:hidden;text-overflow:ellipsis;white-space:nowrap";
  try {
    const [res, prm] = await Promise.all([invoke("mcp_resources"), invoke("mcp_prompts")]);
    const rl = res.filter((r) => r.server === m.name);
    const pl = prm.filter((p) => p.server === m.name);
    det = el("div", "mcpres");
    det.style.cssText = "margin:2px 0 4px 16px;display:flex;flex-direction:column;gap:2px;align-items:flex-start";
    det.append(el("span", "", { text: `resources (${rl.length})` }));
    for (const r of rl) {
      const rowEl = el("div");
      rowEl.style.cssText = "display:flex;gap:4px;align-items:center;max-width:100%";
      const b = el("button", "ghost", {
        text: r.name || r.uri,
        title: (r.description ? r.description + " — " : "") + r.uri + (r.mime ? " (" + r.mime + ")" : "")
      });
      b.style.cssText = itemStyle + ";flex:1;min-width:0";
      b.onclick = async () => {
        b.disabled = true;
        try {
          const out = await invoke("mcp_read_resource", { server: r.server, uri: r.uri });
          if (out.text.length > 100000) {
            note(`resource too big for the input (${out.text.length} chars) — ask the agent to read ${r.uri} via mcp_resource`);
          } else {
            $("input").value = out.text;
            $("input").focus();
          }
        } catch (e) {
          note(String(e));
        } finally {
          b.disabled = false;
        }
      };
      const sb = el("button", "ghost", { text: "sub", title: "toggle subscription — resource updates land in the chat" });
      sb.style.cssText = "padding:0 8px;font-size:11px;flex:none";
      sb.onclick = async () => {
        sb.disabled = true;
        try {
          if (sb.textContent === "sub") {
            await invoke("mcp_subscribe", { server: r.server, uri: r.uri });
            sb.textContent = "unsub";
          } else {
            await invoke("mcp_unsubscribe", { server: r.server, uri: r.uri });
            sb.textContent = "sub";
          }
        } catch (e) {
          note(String(e));
        } finally {
          sb.disabled = false;
        }
      };
      rowEl.append(b, sb);
      det.append(rowEl);
    }
    if (!rl.length) det.append(el("span", "", { text: "  none" }));
    det.append(el("span", "", { text: `prompts (${pl.length})` }));
    for (const p of pl) {
      const b = el("button", "ghost", { text: p.name, title: p.description || p.name });
      b.style.cssText = itemStyle;
      b.onclick = async () => {
        b.disabled = true;
        try {
          const msgs = await invoke("mcp_get_prompt", { server: p.server, name: p.name, args: {} });
          const text = msgs
            .map((x) => (x.role !== "user" ? "[" + x.role + "]\n" : "") + x.text)
            .join("\n\n");
          $("input").value = text;
          $("input").focus();
        } catch (e) {
          note(String(e));
        } finally {
          b.disabled = false;
        }
      };
      det.append(b);
    }
    if (!pl.length) det.append(el("span", "", { text: "  none" }));
    row.after(det);
  } catch (e) {
    note(String(e));
  } finally {
    btn.disabled = false;
    btn.textContent = "res";
  }
}

function fillPermList() {
  const box = $("s-perm");
  const p = (CFG && CFG.permissions) || {};
  const lines = [];
  for (const k of ["edit", "write_file", "bash", "mcp"]) {
    if (p[k]) lines.push(k + " = " + p[k]);
  }
  for (const r of p.rules || []) {
    lines.push(`${r.tool} ${r.pattern || "*"} = ${r.permission}`);
  }
  box.textContent = lines.length
    ? lines.join("\n")
    : "default: mutations (write/edit/bash/mcp) ask, reads allowed";
  box.style.whiteSpace = "pre-wrap";
}

function openSettings() {
  const p = CFG.provider;
  $("s-type").value = p.type || "openai";
  $("s-base").value = p.base_url || "";
  $("s-key").value = p.api_key || "";
  $("s-key").type = "password";
  $("s-key-toggle").textContent = "show";
  $("s-model").value = p.model || "";
  $("s-max-tokens").value = p.max_tokens ?? "";
  $("s-temperature").value = p.temperature ?? "";
  $("s-top-p").value = p.top_p ?? "";
  $("s-stream").checked = p.stream !== false;
  $("s-model-status").textContent = "";
  $("s-msg").textContent = "";
  buildKeysGrid(KEYS);
  fillMcpList();
  fillPermList();
  settingsOpen = true;
  $("settings-overlay").classList.remove("hidden");
}

function closeSettings() {
  settingsOpen = false;
  $("settings-overlay").classList.add("hidden");
  $("input").focus();
}

$("s-key-toggle").onclick = () => {
  const k = $("s-key");
  const hidden = k.type === "password";
  k.type = hidden ? "text" : "password";
  $("s-key-toggle").textContent = hidden ? "hide" : "show";
};

$("s-load-models").onclick = async () => {
  const st = $("s-model-status");
  st.textContent = "loading...";
  try {
    models = await invoke("list_models", {
      kind: $("s-type").value,
      base_url: $("s-base").value.trim() || null,
      api_key: $("s-key").value.trim() || null,
    });
    st.textContent = models.length + " models";
    const dl = $("model-list");
    dl.replaceChildren();
    for (const m of models) dl.appendChild(new Option(m, m));
    fillModelSelect();
  } catch (e) {
    st.textContent = String(e);
  }
};

$("s-mcp-reconnect").onclick = async () => {
  const btn = $("s-mcp-reconnect");
  btn.disabled = true;
  try {
    const logs = await invoke("mcp_reconnect");
    for (const l of logs) note(l);
  } finally {
    btn.disabled = false;
  }
};

$("s-save").onclick = async () => {
  const num = (id) => {
    const v = $(id).value.trim();
    return v === "" ? null : Number(v);
  };
  const model = $("s-model").value.trim();
  if (!model) {
    $("s-msg").textContent = "model is required";
    return;
  }
  const cfg = {
    provider: {
      type: $("s-type").value,
      model,
      base_url: $("s-base").value.trim() || null,
      api_key: $("s-key").value,
      max_tokens: num("s-max-tokens"),
      temperature: num("s-temperature"),
      top_p: num("s-top-p"),
      stream: $("s-stream").checked,
    },
    mcp: (CFG && CFG.mcp) || [],
    keys: readKeysGrid(),
    ui: { theme: document.body.dataset.theme },
    agent: (CFG && CFG.agent) || undefined,
    permissions: (CFG && CFG.permissions) || undefined,
  };
  try {
    await invoke("save", { cfg });
  } catch (e) {
    $("s-msg").textContent = String(e);
    return;
  }
  CFG = cfg;
  KEYS = effectiveKeys(cfg.keys);
  models = [];
  fillModelSelect();
  closeSettings();
  note("settings saved");
};

$("s-cancel").onclick = closeSettings;
$("settings-close").onclick = closeSettings;

/* buttons */

$("btn-settings").onclick = openSettings;
$("btn-plan").onclick = togglePlan;
$("btn-new").onclick = newChat;
$("btn-undo").onclick = () => runCmd("/undo");
$("btn-redo").onclick = () => runCmd("/redo");
$("btn-send").onclick = doSend;
$("btn-sidebar").onclick = toggleSidebar;
$("btn-theme").onclick = toggleTheme;
$("btn-attach").onclick = openFiles;
$("btn-review").onclick = openReview;
$("btn-palette").onclick = openPalette;
$("review-close").onclick = closeReview;
$("ctx-pill").onclick = () => runCmd("/compact");
$("palette-input").addEventListener("input", () => {
  paletteIdx = 0;
  paletteItems = paletteFilter($("palette-input").value);
  renderPalette();
});
$("palette-input").addEventListener("keydown", (e) => {
  if (e.key === "ArrowDown") {
    e.preventDefault();
    paletteIdx = Math.min(paletteIdx + 1, paletteItems.length - 1);
    renderPalette();
  } else if (e.key === "ArrowUp") {
    e.preventDefault();
    paletteIdx = Math.max(paletteIdx - 1, 0);
    renderPalette();
  } else if (e.key === "Enter") {
    e.preventDefault();
    runPalette();
  } else if (e.key === "Escape") {
    e.preventDefault();
    closePalette();
  }
});
$("files-close").onclick = closeFiles;
$("files-up").onclick = () => filesParent && browse(filesParent);
$("files-here").onclick = async () => {
  await attach(filesPath);
  closeFiles();
};

/* input */

function autosize() {
  const input = $("input");
  input.style.height = "auto";
  input.style.height = Math.min(input.scrollHeight, 220) + "px";
}

/* file mentions */

let MFILES = null;
let MAGENTS = null;
let mention = null;
let mentionSeq = 0;

function mentionToken() {
  const input = $("input");
  const pos = input.selectionStart;
  const before = input.value.slice(0, pos);
  const m = before.match(/(?:^|\s)@([^\s@]*)$/);
  if (!m) return null;
  return { query: m[1], from: pos - m[1].length - 1, to: pos };
}

function showMention() {
  const t = mentionToken();
  const seq = ++mentionSeq;
  if (!t) {
    hideMention();
    return;
  }
  const proceed = async () => {
    if (!MFILES) {
      MFILES = await invoke("list_project_files").catch(() => []);
      if (MFILES.length > 800) MFILES.length = 800;
    }
    if (!MAGENTS) {
      const r = await invoke("list_agents").catch(() => ({ agents: [] }));
      MAGENTS = (r.agents || []).map((a) => ({
        v: a.name,
        label: a.description ? a.name + " — " + a.description : a.name,
        agent: true,
      }));
      MAGENTS.push({ v: "general", label: "general — full tool access", agent: true });
      MAGENTS.push({ v: "explore", label: "explore — read-only exploration", agent: true });
    }
    if (seq !== mentionSeq) return;
    const q = t.query.toLowerCase();
    const agentItems = MAGENTS.filter((a) => !q || a.v.toLowerCase().startsWith(q)).slice(0, 4);
    const fileItems = (q ? MFILES.filter((f) => f.toLowerCase().includes(q)) : MFILES)
      .slice(0, 7)
      .map((f) => ({ v: f, label: f, agent: false }));
    const items = [...agentItems, ...fileItems].slice(0, 7);
    if (!items.length) {
      hideMention();
      return;
    }
    mention = { ...t, items, idx: 0 };
    renderMention();
  };
  proceed();
}

function hideMention() {
  mention = null;
  $("mention").classList.add("hidden");
}

function renderMention() {
  if (!mention) return;
  const box = $("mention");
  box.replaceChildren();
  mention.items.forEach((p, i) => {
    const row = el("div", "mrow" + (i === mention.idx ? " active" : ""));
    row.innerHTML = `<span class="ic">${icon(p.agent ? "spark" : "file")}</span>`;
    row.appendChild(el("span", null, p.label));
    row.onmousedown = (e) => {
      e.preventDefault();
      mention.idx = i;
      completeMention();
    };
    box.appendChild(row);
  });
  box.classList.remove("hidden");
}

function completeMention() {
  if (!mention) return;
  const input = $("input");
  const path = mention.items[mention.idx];
  const start = mention.from;
  const end = mention.to;
  input.value = input.value.slice(0, start) + "@" + path + " " + input.value.slice(end);
  const caret = start + path.length + 2;
  input.setSelectionRange(caret, caret);
  hideMention();
  autosize();
  input.focus();
}

$("input").addEventListener("input", () => {
  autosize();
  showMention();
});
$("input").addEventListener("click", hideMention);

/* hotkeys */

window.addEventListener("keydown", (e) => {
  if (askOpen) {
    if (e.key === "Escape") {
      e.preventDefault();
      skipAsk();
    } else if (e.key === "Enter" && e.target.tagName !== "INPUT") {
      e.preventDefault();
      submitAsk();
    }
    return;
  }
  if (confirmOpen) {
    const c = comboFromEvent(e);
    if (!c) return;
    if (c.name === "y" && !c.ctrl && !c.alt && !c.meta) {
      e.preventDefault();
      resolveConfirm(true);
    } else if ((c.name === "n" || (c.name === "escape" && sameCombo(c, parseCombo(KEYS.stop)))) && !c.ctrl && !c.alt && !c.meta) {
      e.preventDefault();
      resolveConfirm(false);
    } else if (c.name === "a" && !c.ctrl && !c.alt && !c.meta) {
      e.preventDefault();
      allowAll();
    } else if (c.name === "w" && !c.ctrl && !c.alt && !c.meta) {
      e.preventDefault();
      resolveConfirm(true, true);
    }
    return;
  }
  if (filesOpen) {
    if (e.key === "Escape") {
      e.preventDefault();
      closeFiles();
    }
    return;
  }
  if (settingsOpen) {
    if (e.key === "Escape" && e.target.tagName !== "INPUT") {
      e.preventDefault();
      closeSettings();
    }
    return;
  }
  if (reviewOpen) {
    if (e.key === "Escape") {
      e.preventDefault();
      closeReview();
    }
    return;
  }
  if (paletteOpen) {
    if (e.key === "Escape") {
      e.preventDefault();
      closePalette();
    }
    return;
  }
  if (mention && e.target === $("input")) {
    if (e.key === "ArrowDown" || e.key === "ArrowUp" || e.key === "Tab" || e.key === "Enter" || e.key === "Escape") {
      e.preventDefault();
      if (e.key === "ArrowDown") {
        mention.idx = (mention.idx + 1) % mention.items.length;
        renderMention();
      } else if (e.key === "ArrowUp") {
        mention.idx = (mention.idx - 1 + mention.items.length) % mention.items.length;
        renderMention();
      } else if (e.key === "Tab" || e.key === "Enter") {
        completeMention();
      } else {
        hideMention();
      }
      return;
    }
  }
  const hit = (action) => sameCombo(comboFromEvent(e), parseCombo(KEYS[action]));
  if (waiting && hit("stop")) {
    e.preventDefault();
    invoke("stop");
    return;
  }
  if (e.target === $("input")) {
    if (hit("send")) {
      e.preventDefault();
      doSend();
      return;
    }
    if (hit("newline")) {
      e.preventDefault();
      const i = $("input");
      const s = i.selectionStart, epos = i.selectionEnd;
      i.value = i.value.slice(0, s) + "\n" + i.value.slice(epos);
      i.selectionStart = i.selectionEnd = s + 1;
      autosize();
      return;
    }
  }
  if (hit("new_session")) {
    e.preventDefault();
    newChat();
  } else if (hit("open_settings")) {
    e.preventDefault();
    openSettings();
  } else if (hit("toggle_sidebar")) {
    e.preventDefault();
    toggleSidebar();
  } else if (hit("toggle_theme")) {
    e.preventDefault();
    toggleTheme();
  } else if (hit("toggle_plan")) {
    e.preventDefault();
    togglePlan();
  } else if (hit("undo")) {
    e.preventDefault();
    runCmd("/undo");
  } else if (hit("redo")) {
    e.preventDefault();
    runCmd("/redo");
  } else if (hit("toggle_thinking")) {
    e.preventDefault();
    const boxes = document.querySelectorAll(".think");
    if (boxes.length) {
      const last = boxes[boxes.length - 1];
      last.classList.toggle("open");
    }
  } else if (hit("palette")) {
    e.preventDefault();
    openPalette();
  }
});

/* init */

(async function init() {
  await listen("ev", (e) => handleEvent(e.payload));
  try {
    const st = await invoke("init");
    CFG = st.cfg;
    KEYS = effectiveKeys(st.keys);
    MODEL = CFG.provider.model || "";
    KIND = CFG.provider.type || "openai";
    CWD = st.cwd;
    SID = st.sid || "";
    SESSIONS = st.sessions || [];
    setTheme(st.theme || "dark", false);
    $("sb-cwd").textContent = st.cwd;
    $("sb-cwd").title = st.cwd;
    if (localStorage.getItem("hiderola.sidebar") === "0" || window.innerWidth < 900) {
      $("sidebar").classList.add("hidden");
    }
    fillModelSelect();
    renderSessions();
    renderTodoCard(st.todos || "");
    PLAN = !!st.plan;
    applyPlan();
    if (!st.has_provider) note("no api key yet — press ctrl+comma or click the sliders icon to add one");
    applyStatus();
  } catch (e) {
    note("init failed: " + e);
    applyStatus();
  }
  $("input").focus();
})();
