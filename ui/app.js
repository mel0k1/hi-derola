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
let sandboxOpen = false;
let SBX = { qemu: null, dir: "", list: [], attached: null, lastJson: "" };
let SBXW = null;
let sbxTimer = null;

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
  box: '<path d="M21 8l-9-5-9 5v8l9 5 9-5z"/><path d="M3 8l9 5 9-5"/><path d="M12 13v9"/>',
  play: '<path d="M7 4l13 8-13 8z"/>',
  stop: '<rect x="6" y="6" width="12" height="12" rx="1.5"/>',
  download: '<path d="M12 3v12"/><path d="M7 10l5 5 5-5"/><path d="M4 17v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-2"/>',
  warn: '<path d="M12 3L2 21h20z"/><path d="M12 10v5"/><path d="M12 18h.01"/>',
  terminal: '<path d="M4 17l6-5-6-5"/><path d="M12 19h8"/>',
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
    right.textContent = "thinking";
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
    ["/mcpres", "list mcp resources", "resources templates"],
    ["/mcpstatus", "per-server mcp status", "connected failed auth"],
    ["/mcplog", "recent mcp log messages", "logs diagnostics"],
    ["/jstools", "user JS tools", "list reload sandbox"],
  ];
  for (const [cmd, label, hint] of cmds) {
    items.push({ icon: "command", label, hint, run: () => runCmd(cmd) });
  }
  items.push({ icon: "sun", label: "toggle theme", hint: "dark / light", run: toggleTheme });
  items.push({ icon: "box", label: "open sandbox", hint: "local VMs, isolated agent workspace", run: () => toggleSandboxView(true) });
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
  if (!text || confirmOpen || askOpen || settingsOpen || filesOpen || sandboxOpen) return;
  hideMention();
  input.value = "";
  autosize();
  clearWelcome();
  const m = el("div", "msg you");
  m.appendChild(el("div", "who", "you"));
  m.appendChild(el("div", "body", text));
  $("chat-col").appendChild(m);
  autoscroll();
  const sendBtn = $("btn-send");
  sendBtn.classList.remove("ready");
  sendBtn.classList.add("sent");
  setTimeout(() => sendBtn.classList.remove("sent"), 350);
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

// a question may carry a raw json schema (mcp elicitation): render one
// control per property instead of a single free-text input
function elicitForm(q) {
  const props = q.schema && q.schema.properties;
  if (!props || typeof props !== "object") return null;
  const names = Object.keys(props);
  if (!names.length) return null;
  return {
    props,
    names,
    required: Array.isArray(q.schema.required) ? q.schema.required : [],
  };
}

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
  const forms = qs.map((q) => {
    const f = elicitForm(q);
    if (!f) return null;
    const vals = {};
    f.names.forEach((n) => {
      const d = f.props[n] || {};
      if (d.default !== undefined) vals[n] = d.type === "boolean" ? !!d.default : d.default;
      else if (d.type === "boolean") vals[n] = false;
    });
    return { ...f, vals };
  });
  askState = { qs, sel: qs.map(() => new Set()), custom: qs.map(() => ""), forms };
  const body = $("ask-body");
  body.replaceChildren();
  qs.forEach((q, i) => {
    const wrap = el("div", "ask-q");
    const header = (q.header || "").trim();
    if (header) wrap.appendChild(el("div", "q-header", header));
    wrap.appendChild(el("div", "q-text", q.question || ""));
    const form = forms[i];
    if (form) {
      const grid = el("div", "ask-form");
      form.names.forEach((name) => {
        const def = form.props[name] || {};
        const desc = (def.description || "").trim();
        const req = form.required.includes(name);
        const row = el("div", "ask-field");
        const lab = el("div", "f-lab");
        const nameSpan = el("span", "f-name", name);
        if (req) nameSpan.appendChild(el("span", "f-req", " *"));
        lab.appendChild(nameSpan);
        if (desc) {
          lab.title = desc;
          lab.appendChild(el("span", "f-desc", desc));
        }
        row.appendChild(lab);
        const ctl = document.createElement("div");
        ctl.className = "f-ctl";
        const record = (v) => { form.vals[name] = v; row.classList.remove("f-missing"); };
        if (Array.isArray(def.enum) && def.enum.length) {
          const names = Array.isArray(def.enumNames) ? def.enumNames : [];
          const sel = document.createElement("select");
          const empty = document.createElement("option");
          empty.value = "";
          empty.textContent = req ? "— pick —" : "— none —";
          sel.appendChild(empty);
          def.enum.forEach((v, k) => {
            const o = document.createElement("option");
            o.value = k;
            o.textContent = names[k] !== undefined && names[k] !== null ? String(names[k]) : v === null ? "null" : String(v);
            if (def.default !== undefined && String(def.default) === String(v)) o.selected = true;
            sel.appendChild(o);
          });
          sel.dataset.enum = name;
          sel.onchange = () => {
            record(sel.value === "" ? "" : def.enum[Number(sel.value)]);
          };
          if (def.default !== undefined) record(def.default);
          ctl.appendChild(sel);
        } else if (def.type === "array" && Array.isArray(def.items && def.items.enum) && def.items.enum.length) {
          // multiselect: checkbox group, the answer is an array of enum values
          const inames = Array.isArray(def.items.enumNames) ? def.items.enumNames : [];
          const box = el("div", "f-multi");
          const chosen = new Set();
          def.items.enum.forEach((v, k) => {
            const lab = document.createElement("label");
            const cb = document.createElement("input");
            cb.type = "checkbox";
            cb.value = k;
            cb.onchange = () => {
              cb.checked ? chosen.add(k) : chosen.delete(k);
              record(def.items.enum.filter((_, j) => chosen.has(j)));
              row.classList.remove("f-missing");
            };
            lab.appendChild(cb);
            lab.appendChild(document.createTextNode(inames[k] !== undefined && inames[k] !== null ? String(inames[k]) : String(v)));
            box.appendChild(lab);
          });
          ctl.appendChild(box);
        } else if (def.type === "boolean") {
          const box = document.createElement("input");
          box.type = "checkbox";
          box.checked = !!form.vals[name];
          box.onchange = () => record(box.checked);
          ctl.appendChild(box);
        } else if (def.type === "integer" || def.type === "number") {
          const inp = document.createElement("input");
          inp.type = "number";
          if (def.type === "integer") inp.step = "1";
          if (def.minimum !== undefined) inp.min = def.minimum;
          if (def.maximum !== undefined) inp.max = def.maximum;
          if (def.default !== undefined) inp.value = String(def.default);
          inp.placeholder = def.type;
          inp.oninput = () => record(inp.value === "" ? "" : Number(inp.value));
          ctl.appendChild(inp);
        } else {
          const inp = document.createElement("input");
          inp.type = "text";
          if (def.maxLength !== undefined) inp.maxLength = def.maxLength;
          if (def.format) inp.placeholder = def.format;
          if (def.default !== undefined) inp.value = String(def.default);
          inp.spellcheck = false;
          if (!inp.placeholder) inp.placeholder = "value";
          inp.oninput = () => record(inp.value);
          inp.onkeydown = (e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              submitAsk();
            }
          };
          ctl.appendChild(inp);
        }
        row.appendChild(ctl);
        grid.appendChild(row);
      });
      wrap.appendChild(grid);
    } else {
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
    }
    body.appendChild(wrap);
  });
  $("ask-overlay").classList.remove("hidden");
  $("input").blur();
  const first = body.querySelector(".ask-form input, .ask-form select, input");
  if (first) first.focus();
}

function composeAsk() {
  const { qs, sel, custom, forms } = askState;
  const lines = [];
  for (let i = 0; i < qs.length; i++) {
    const q = qs[i];
    const form = forms[i];
    if (form) {
      // schema form: submit a json object, empty required fields block submit
      for (const name of form.names) {
        const v = form.vals[name];
        const missing =
          form.required.includes(name) &&
          (v === undefined || v === "" || v === null);
        if (missing) {
          const row = [...document.querySelectorAll("#ask-body .ask-field")].find(
            (r) => r.querySelector(".f-name") && r.querySelector(".f-name").textContent.trim() === name
          );
          if (row) {
            row.classList.add("f-missing");
            const c = row.querySelector("input, select");
            if (c) c.focus();
          }
          return null;
        }
      }
      const obj = {};
      for (const name of form.names) {
        const v = form.vals[name];
        if (v === undefined || v === "" || v === null) continue;
        if (Array.isArray(v) && !v.length) continue;
        obj[name] = v;
      }
      if (Object.keys(obj).length) lines.push(JSON.stringify(obj));
      continue;
    }
    const labels = [...sel[i]].map((j) => ((q.options || [])[j] || {}).label).filter(Boolean);
    const customText = (custom[i] || "").trim();
    let ans = customText || labels.join(", ");
    if (!ans) continue;
    const header = (q.header || "").trim();
    lines.push(header ? header + ": " + ans : ans);
  }
  return lines.join("\n");
}

function submitAsk() {
  const text = composeAsk();
  if (text === null) return; // required fields are highlighted, stay open
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
    box.textContent = "no servers yet — add one below or edit config.toml [[mcp]]";
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
          const msg = String(e);
          note(msg);
          // the flow started (verifier persisted) but the callback never
          // came: offer to finish with a pasted code
          if (msg.includes("mcpauth") && msg.includes("<code")) {
            const url = msg.split("\n").find((l) => l.startsWith("http"));
            if (url) window.open(url, "_blank");
            const code = prompt("paste the code from the redirect url:");
            if (code && code.trim()) {
              try {
                const logs = await invoke("mcp_auth", { name: m.name, code: code.trim() });
                for (const l of logs) note(l);
              } catch (e2) {
                note(String(e2));
              }
            }
          }
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
    const rm = el("button", "icon-btn");
    rm.innerHTML = icon("trash");
    rm.title = "remove from config";
    rm.style.cssText = "width:22px;height:22px;flex:none";
    rm.onclick = () => removeMcpServer(m.name, rm);
    row.append(rm);
    box.append(row);
  }
  box.style.whiteSpace = "";
}

function closePromptForm(det) {
  const f = det.querySelector(".prmargs");
  if (f) f.remove();
}

/* inline form with one input per prompt argument (required ones starred) */
function buildPromptArgsForm(p, onRun) {
  const box = el("div", "prmargs");
  box.style.cssText = "display:flex;flex-direction:column;gap:2px;align-items:flex-start;margin:2px 0 0 8px";
  const inputs = [];
  for (const a of p.arguments) {
    const row = el("div");
    row.style.cssText = "display:flex;gap:4px;align-items:center;max-width:100%";
    const lab = el("label", "", (a.required ? "*" : "") + a.name);
    lab.title = a.description || a.name;
    lab.style.cssText = "font-size:11px;flex:none";
    const inp = el("input");
    inp.placeholder = a.description || a.name;
    inp.style.cssText = "font-size:11px;padding:1px 6px;width:240px;flex:1;min-width:0";
    inp.dataset.argname = a.name;
    inp.onkeydown = (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        run.click();
      }
    };
    row.append(lab, inp);
    box.append(row);
    inputs.push(inp);
  }
  const btns = el("div");
  btns.style.cssText = "display:flex;gap:4px";
  const run = el("button", "ghost", { text: "run" });
  run.title = "fetch the prompt with these arguments";
  run.style.cssText = "padding:0 8px;font-size:11px";
  const cancel = el("button", "ghost", { text: "cancel" });
  cancel.style.cssText = "padding:0 8px;font-size:11px";
  run.onclick = () => {
    const args = {};
    for (const inp of inputs) {
      const v = inp.value.trim();
      if (v) args[inp.dataset.argname] = v;
    }
    onRun(args);
  };
  cancel.onclick = () => box.remove();
  btns.append(run, cancel);
  box.append(btns);
  return box;
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
    const [res, prm, tpl, subs] = await Promise.all([
      invoke("mcp_resources"), invoke("mcp_prompts"), invoke("mcp_templates"), invoke("mcp_subscriptions"),
    ]);
    const rl = res.filter((r) => r.server === m.name);
    const pl = prm.filter((p) => p.server === m.name);
    const tl = tpl.filter((t) => t.server === m.name);
    const subSet = new Set(subs.map((s) => s.server + "|" + s.uri));
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
      const sb = el("button", "ghost", { title: "toggle subscription — resource updates land in the chat" });
      sb.textContent = subSet.has(r.server + "|" + r.uri) ? "unsub" : "sub";
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
    det.append(el("span", "", { text: `uri templates (${tl.length})` }));
    for (const t of tl) {
      const b = el("button", "ghost", {
        text: t.name || t.uri_template,
        title: (t.description ? t.description + " — " : "") + t.uri_template + (t.mime ? " (" + t.mime + ")" : "")
      });
      b.style.cssText = itemStyle;
      b.onclick = () => {
        // templates need real values in place of the braces: drop the read
        // command into the input so the user fills them in
        $("input").value = "/mcpread " + t.server + " " + t.uri_template;
        $("input").focus();
      };
      det.append(b);
    }
    if (!tl.length) det.append(el("span", "", { text: "  none" }));
    det.append(el("span", "", { text: `prompts (${pl.length})` }));
    for (const p of pl) {
      const b = el("button", "ghost", { text: p.name, title: p.description || p.name });
      b.style.cssText = itemStyle;
      b.onclick = async () => {
        // prompts that declare arguments open an inline form instead of
        // silently running with empty args
        if (p.arguments && p.arguments.length) {
          if (b.nextElementSibling && b.nextElementSibling.classList.contains("prmargs")) {
            b.nextElementSibling.remove();
            return;
          }
          closePromptForm(det);
          const form = buildPromptArgsForm(p, async (args) => {
            form.remove();
            b.disabled = true;
            try {
              const msgs = await invoke("mcp_get_prompt", { server: p.server, name: p.name, args });
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
          });
          b.after(form);
          form.querySelector("input").focus();
          return;
        }
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

/* add / remove mcp servers right from settings */

function parseKVLines(text, sep) {
  const out = {};
  for (const line of (text || "").split("\n")) {
    const t = line.trim();
    if (!t) continue;
    const i = t.indexOf(sep);
    if (i <= 0) continue;
    out[t.slice(0, i).trim()] = t.slice(i + 1).trim();
  }
  return out;
}

function applyMcpType() {
  const remote = $("sa-type").value === "remote";
  $("sa-stdio-fields").classList.toggle("hidden", remote);
  $("sa-remote-fields").classList.toggle("hidden", !remote);
}

$("s-mcp-add").onclick = () => {
  for (const id of ["sa-name", "sa-command", "sa-args", "sa-env", "sa-cwd", "sa-url", "sa-headers", "sa-timeout"]) {
    $(id).value = "";
  }
  $("sa-type").value = "stdio";
  applyMcpType();
  $("s-msg").textContent = "";
  $("s-mcp-form").classList.remove("hidden");
  $("sa-name").focus();
};
$("sa-type").onchange = applyMcpType;
$("sa-cancel").onclick = () => $("s-mcp-form").classList.add("hidden");

$("sa-save").onclick = async () => {
  const name = $("sa-name").value.trim();
  if (!name) {
    $("s-msg").textContent = "name is required";
    return;
  }
  if (((CFG && CFG.mcp) || []).some((m) => m.name === name)) {
    $("s-msg").textContent = "a server with this name already exists";
    return;
  }
  const remote = $("sa-type").value === "remote";
  const entry = { name, type: remote ? "remote" : "stdio" };
  if (remote) {
    const url = $("sa-url").value.trim();
    if (!url) {
      $("s-msg").textContent = "url is required for remote servers";
      return;
    }
    entry.url = url;
    const headers = parseKVLines($("sa-headers").value, ":");
    if (Object.keys(headers).length) entry.headers = headers;
  } else {
    const cmd = $("sa-command").value.trim();
    if (!cmd) {
      $("s-msg").textContent = "command is required for stdio servers";
      return;
    }
    entry.command = cmd;
    entry.args = $("sa-args").value.split("\n").map((s) => s.trim()).filter(Boolean);
    const env = parseKVLines($("sa-env").value, "=");
    if (Object.keys(env).length) entry.env = env;
    const cwd = $("sa-cwd").value.trim();
    if (cwd) entry.cwd = cwd;
  }
  const t = Number($("sa-timeout").value);
  if ($("sa-timeout").value.trim() && Number.isFinite(t) && t > 0) entry.timeout = t;
  const btn = $("sa-save");
  btn.disabled = true;
  $("s-msg").textContent = "";
  try {
    CFG = { ...(CFG || {}), mcp: [...((CFG && CFG.mcp) || []), entry] };
    await invoke("save", { cfg: buildCfgPayload() });
    const logs = await invoke("mcp_reconnect");
    for (const l of logs) note(l);
    fillMcpList();
    $("s-mcp-form").classList.add("hidden");
    $("s-msg").textContent = "server added";
  } catch (e) {
    $("s-msg").textContent = String(e);
  } finally {
    btn.disabled = false;
  }
};

async function removeMcpServer(name, btn) {
  // two clicks: first arms, second deletes (same pattern as session delete)
  if (!btn.classList.contains("confirm")) {
    btn.classList.add("confirm");
    btn.innerHTML = icon("check");
    setTimeout(() => {
      btn.classList.remove("confirm");
      btn.innerHTML = icon("trash");
    }, 2500);
    return;
  }
  CFG = { ...(CFG || {}), mcp: ((CFG && CFG.mcp) || []).filter((x) => x.name !== name) };
  try {
    await invoke("save", { cfg: buildCfgPayload() });
    const logs = await invoke("mcp_reconnect");
    for (const l of logs) note(l);
    note(`server ${name} removed from config`);
  } catch (e) {
    note(String(e));
  }
  fillMcpList();
}

function buildCfgPayload() {
  const num = (id) => {
    const v = $(id).value.trim();
    return v === "" ? null : Number(v);
  };
  return {
    provider: {
      type: $("s-type").value,
      model: $("s-model").value.trim(),
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
}

$("s-save").onclick = async () => {
  if (!$("s-model").value.trim()) {
    $("s-msg").textContent = "model is required";
    return;
  }
  const cfg = buildCfgPayload();
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

/* sandbox tab: local QEMU VMs for isolated agent work */

const SBX_IMAGES = [
  {
    id: "debian-trixie",
    label: "Debian 13 (trixie) minimal",
    size: "~700 MiB download",
    hint: "official minimal cloud image, boots fast; a cloud-init seed (your login + a generated ssh key) is created automatically",
  },
  {
    id: "debian-trixie-std",
    label: "Debian 13 (trixie) standard",
    size: "~800 MiB download",
    hint: "the full generic cloud image — more packages on board than the minimal one, same cloud-init seed and ssh wiring",
  },
  {
    id: "ubuntu-24.04",
    label: "Ubuntu 24.04 LTS minimal",
    size: "~300 MiB download",
    hint: "Canonical's trimmed cloud image — smaller and quicker to boot than the standard server image; the cloud-init seed works out of the box",
  },
  {
    id: "ubuntu-24.04-std",
    label: "Ubuntu 24.04 LTS standard",
    size: "~650 MiB download",
    hint: "the standard server cloud image — ships the full server toolset; the cloud-init seed works out of the box",
  },
  {
    id: "custom",
    label: "own image",
    size: "no download",
    hint: "an .iso boots as install media; a qcow2 / raw disk image boots directly (no seed — you configure the login yourself)",
  },
];

/* kinds that no longer appear in the wizard but may exist on disk */
const SBX_LEGACY_LABELS = { nixos: "NixOS minimal (legacy)" };

function toggleSandboxView(force) {
  const on = force === undefined ? !sandboxOpen : !!force;
  if (on === sandboxOpen && !on) return;
  sandboxOpen = on;
  $("sandbox-view").classList.toggle("hidden", !on);
  $("chat").classList.toggle("hidden", on);
  $("chips").classList.toggle("hidden", on);
  $("inputbar").classList.toggle("hidden", on);
  $("btn-sandbox").classList.toggle("on", on);
  if (on) {
    SBX.qemu = null; // re-detect every time the tab opens (QEMU may be installed meanwhile)
    closeWizard();
    showSbxMsg("");
    refreshSandbox();
  } else {
    stopSbxPoll();
    $("input").focus();
  }
}

function showSbxMsg(text) {
  const box = $("sbx-msg");
  box.textContent = text || "";
  box.classList.toggle("hidden", !text);
}

async function refreshSandbox() {
  try {
    const [qemu, list] = await Promise.all([
      SBX.qemu ? Promise.resolve(SBX.qemu) : invoke("sandbox_detect"),
      invoke("sandbox_list"),
    ]);
    SBX.qemu = qemu;
    SBX.dir = list.dir || "";
    SBX.list = list.sandboxes || [];
    SBX.attached = list.attached || null;
    SBX.lastJson = JSON.stringify(SBX.list);
    renderSbxQemu();
    renderSbxList();
    startSbxPoll();
  } catch (e) {
    showSbxMsg(String(e));
  }
}

function startSbxPoll() {
  if (sbxTimer) return;
  sbxTimer = setInterval(async () => {
    if (!sandboxOpen) return;
    try {
      const list = await invoke("sandbox_list");
      const j = JSON.stringify([list.sandboxes || [], list.attached || null]);
      if (j !== SBX.lastJson) {
        SBX.lastJson = j;
        SBX.list = list.sandboxes || [];
        SBX.attached = list.attached || null;
        renderSbxList();
      }
    } catch {}
  }, 900);
}

function stopSbxPoll() {
  if (sbxTimer) {
    clearInterval(sbxTimer);
    sbxTimer = null;
  }
}

function sbxKindLabel(kind) {
  const img = SBX_IMAGES.find((i) => i.id === kind);
  if (img) return img.label;
  return SBX_LEGACY_LABELS[kind] || kind;
}

function renderSbxQemu() {
  const box = $("sbx-qemu");
  const q = SBX.qemu;
  if (!q) {
    box.classList.add("hidden");
    return;
  }
  box.classList.remove("hidden");
  if (q.system_path && q.img_path) {
    const v = (q.system_version || "qemu found").replace(/^QEMU emulator version\s*/i, "v").trim();
    box.className = "sbx-qemu ok";
    box.innerHTML = `<span class="ic sm">${icon("check")}</span> qemu ${esc(v)} &middot; accel: ${esc(q.accel || "tcg")} &middot; qemu-img ok`;
  } else {
    box.className = "sbx-qemu warn";
    box.innerHTML = `<span class="ic sm">${icon("warn")}</span> <span>QEMU not found — install it (<b>winget install Software.QEMU</b> or <b>choco install qemu</b>, then restart the app) and the sandbox tab comes alive.</span>`;
  }
}

function renderSbxList() {
  const box = $("sbx-list");
  box.replaceChildren();
  if (!SBX.list.length) {
    const empty = el("div", "sbx-empty");
    empty.innerHTML = `no sandboxes yet — create one and keep the host clean.<br/>the agent works inside the VM while your files stay untouched.`;
    box.appendChild(empty);
    return;
  }
  for (const s of SBX.list) box.appendChild(sbxCard(s));
}

function fmtGiB(mib) {
  const g = mib / 1024;
  return (Number.isInteger(g) ? g : g.toFixed(1)) + " GiB";
}

function sbxCard(s) {
  const card = el("div", "sbx-card");
  card.title = s.dir;

  const head = el("div", "sbx-card-head");
  head.appendChild(el("div", "sbx-name", s.spec.name));
  head.insertAdjacentHTML("beforeend", `<span class="sbx-state ${s.state}">${s.state}</span>`);
  if (SBX.attached === s.spec.id) {
    head.insertAdjacentHTML(
      "beforeend",
      `<span class="sbx-state running" title="the chat's bash and file tools execute inside this VM">\u2190 bash here</span>`
    );
  }
  card.appendChild(head);

  const meta = el("div", "sbx-meta");
  meta.insertAdjacentHTML("beforeend", `<span class="sbx-kind">${esc(sbxKindLabel(s.spec.kind))}</span>`);
  meta.insertAdjacentHTML(
    "beforeend",
    `<span>${s.spec.disk_gib} GiB disk</span><span>${fmtGiB(s.spec.ram_mib)} ram</span><span>${s.spec.cpus} vcpu</span><span>ssh :${s.spec.ssh_port}</span><span>${s.spec.root ? "root allowed" : "no root"}</span>`
  );
  card.appendChild(meta);

  if (s.state === "downloading" && s.download) {
    const pct = s.download.total
      ? Math.min(100, Math.round((s.download.downloaded * 100) / s.download.total))
      : null;
    const bar = el("div", "sbx-progress" + (pct === null ? " indet" : ""));
    bar.innerHTML = `<div class="fill" style="width:${pct === null ? 40 : pct}%"></div>`;
    card.appendChild(bar);
    const line = el("div", "sbx-progress-label");
    line.textContent =
      pct === null
        ? `downloading\u2026 ${(s.download.downloaded / 1048576).toFixed(1)} MiB`
        : `${pct}% \u00b7 ${(s.download.downloaded / 1048576).toFixed(1)} / ${(s.download.total / 1048576).toFixed(1)} MiB`;
    card.appendChild(line);
  }

  // ssh + agent block: the backend only attaches s.ssh for cloud-init kinds
  const seed = !!s.ssh;
  if (seed && s.state === "running") card.appendChild(sbxSshBlock(s));

  if (s.error) card.appendChild(el("div", "sbx-error", s.error));

  const actions = el("div", "sbx-card-actions");
  if (seed && s.state === "running" && s.ssh.state === "ready") {
    const ag = s.ssh.agent || {};
    const term = el("button", "ghost sbx-btn");
    term.innerHTML = `${icon("terminal")} terminal`;
    term.title = "open a shell in the VM (a new terminal window)";
    term.onclick = () => sbxTerminal(s);
    actions.appendChild(term);
    if (ag.state === "installed") {
      const run = el("button", "ghost sbx-btn");
      run.innerHTML = `${icon("spark")} run agent`;
      run.title = "open the agent TUI inside the VM (a new terminal window)";
      run.onclick = () => sbxTerminal(s, true);
      actions.appendChild(run);
    }
    const inst = el("button", "ghost sbx-btn");
    if (ag.state === "installing") {
      inst.disabled = true;
      inst.innerHTML = `${icon("download")} installing\u2026`;
    } else {
      inst.innerHTML = `${icon("download")} ${ag.state === "installed" ? "reinstall agent" : "install agent"}`;
      inst.title = "build the agent inside the VM (apt tools + rustup + cargo install; needs root)";
      inst.onclick = () => sbxInstall(s);
    }
    actions.appendChild(inst);
    const att = el("button", "ghost sbx-btn");
    if (SBX.attached === s.spec.id) {
      att.innerHTML = `${icon("x")} detach chat`;
      att.title = "stop routing the chat's bash + file tools into this VM";
      att.onclick = () =>
        invoke("sandbox_detach")
          .then(refreshSandbox)
          .catch((e) => showSbxMsg(String(e)));
    } else {
      att.innerHTML = `${icon("box")} attach chat`;
      att.title = "run the chat's bash + file tools inside this VM over ssh";
      att.onclick = () =>
        invoke("sandbox_attach", { id: s.spec.id })
          .then(refreshSandbox)
          .catch((e) => showSbxMsg(String(e)));
    }
    actions.appendChild(att);
  }

  const primary = el("button", "ghost sbx-btn");
  if (s.state === "running") primary.innerHTML = `${icon("stop")} stop`;
  else if (s.state === "downloading") primary.innerHTML = `${icon("x")} cancel`;
  else primary.innerHTML = `${icon("play")} ${s.state === "failed" ? "retry" : "start"}`;
  primary.onclick = async () => {
    primary.disabled = true;
    const action =
      s.state === "running" ? "stop" : s.state === "downloading" ? "delete" : "start";
    try {
      await invoke("sandbox_action", { id: s.spec.id, action });
      await refreshSandbox();
    } catch (e) {
      showSbxMsg(String(e));
      primary.disabled = false;
    }
  };
  actions.appendChild(primary);

  const del = el("button", "icon-btn sbx-del");
  del.innerHTML = icon("trash");
  del.title = s.state === "downloading" ? "cancel and delete" : "delete sandbox (two clicks)";
  del.onclick = () => sbxDelete(s, del);
  actions.appendChild(del);
  card.appendChild(actions);
  return card;
}

/* ssh readiness + in-VM agent state for one running cloud sandbox */

function sbxSshBlock(s) {
  const wrap = el("div", "sbx-ssh-wrap");
  const info = s.ssh;
  if (info.state === "waiting") {
    const row = el("div", "sbx-ssh waiting");
    row.innerHTML = `<span class="ic sm">${icon("download")}</span> ssh waiting\u2026 ${info.elapsed_secs || 0}s <span class="hint">(first boot applies the cloud-init seed)</span>`;
    wrap.appendChild(row);
  } else if (info.state === "failed") {
    const row = el("div", "sbx-ssh failed");
    row.innerHTML = `<span class="ic sm">${icon("warn")}</span> ssh failed — ${esc(info.error || "unknown error")}`;
    row.title = "reboot the VM (stop / start) or check qemu.log";
    wrap.appendChild(row);
  } else if (info.state === "ready") {
    const row = el("div", "sbx-ssh ready");
    row.innerHTML = `<span class="ic sm">${icon("check")}</span> ssh ready · ${esc(info.user)}@127.0.0.1:${info.port} <span class="hint">· up in ${info.elapsed_secs || 0}s</span>`;
    wrap.appendChild(row);

    const ag = info.agent || {};
    const agRow = el("div", "sbx-agent");
    if (ag.state === "unknown") {
      agRow.innerHTML = `<span class="hint">the agent is not installed in this VM yet${s.spec.root ? "" : " — installing needs root, enable \"allow root\" at creation"}</span>`;
    } else if (ag.state === "installing") {
      agRow.innerHTML = `<span class="sbx-agent-state">installing agent\u2026</span>`;
    } else if (ag.state === "installed") {
      agRow.innerHTML = `<span class="ic sm">${icon("check")}</span> agent${ag.version ? ` v${esc(ag.version)}` : ""} ready inside the VM`;
    } else if (ag.state === "failed") {
      agRow.innerHTML = `<span class="ic sm">${icon("warn")}</span> install failed — ${esc(ag.error || "error")}`;
    }
    wrap.appendChild(agRow);

    if (ag.log && ag.log.length) {
      const log = el("pre", "sbx-log");
      const tail = ag.log.slice(-14);
      log.textContent =
        (ag.log.length > tail.length ? `\u2026 ${ag.log.length - tail.length} earlier lines\n` : "") +
        tail.join("\n");
      log.scrollTop = log.scrollHeight;
      wrap.appendChild(log);
    }
  }
  return wrap;
}

function sbxInstall(s) {
  invoke("sandbox_agent_install", { id: s.spec.id })
    .then(refreshSandbox)
    .catch((e) => showSbxMsg(String(e)));
}

function sbxTerminal(s, agent = false) {
  invoke("sandbox_ssh_terminal", { id: s.spec.id, agent })
    .catch((e) => showSbxMsg(String(e)));
}

function sbxDelete(s, btn) {
  // two clicks: first arms, second deletes (same pattern as mcp servers)
  if (!btn.classList.contains("confirm")) {
    btn.classList.add("confirm");
    btn.innerHTML = icon("check");
    setTimeout(() => {
      btn.classList.remove("confirm");
      btn.innerHTML = icon("trash");
    }, 2500);
    return;
  }
  invoke("sandbox_action", { id: s.spec.id, action: "delete" })
    .then(refreshSandbox)
    .catch((e) => showSbxMsg(String(e)));
}

/* sandbox wizard: image -> resources -> create */

function openWizard() {
  const used = new Set(SBX.list.map((s) => s.spec.ssh_port));
  let port = 2222;
  while (used.has(port)) port++;
  SBXW = { step: 1, kind: "debian-trixie", iso: "", name: "", login: "derola", disk: 20, ram: 2048, cpus: 2, root: false, port };
  $("sbx-wizard").classList.remove("hidden");
  renderWizard();
}

function sbxwLoginOk() {
  const l = (SBXW.login || "").trim();
  return /^[a-z][a-z0-9_-]{0,31}$/.test(l) && l !== "root";
}

function closeWizard() {
  SBXW = null;
  $("sbx-wizard").classList.add("hidden");
  $("sbxw-body").replaceChildren();
}

function renderWizard() {
  document.querySelectorAll("#sbx-wizard .sbxw-step").forEach((n) => {
    const step = +n.dataset.step;
    n.classList.toggle("active", step === SBXW.step);
    n.classList.toggle("done", step < SBXW.step);
  });
  const body = $("sbxw-body");
  body.replaceChildren();
  if (SBXW.step === 1) renderWizardStep1(body);
  else if (SBXW.step === 2) renderWizardStep2(body);
  else renderWizardStep3(body);
}

function sbxwField(label, build) {
  const lab = el("label", "sbxw-field");
  lab.appendChild(el("span", null, label));
  build(lab);
  return lab;
}

function sbxwNumField(label, val, min, max, set) {
  return sbxwField(label, (lab) => {
    const inp = el("input");
    inp.type = "number";
    inp.min = min;
    inp.max = max;
    inp.value = val;
    inp.oninput = () => {
      const n = Number(inp.value);
      if (inp.value !== "" && Number.isFinite(n)) set(n);
    };
    lab.appendChild(inp);
  });
}

function renderWizardStep1(body) {
  const grid = el("div", "sbxw-images");
  for (const img of SBX_IMAGES) {
    const card = el("button", "sbxw-image" + (SBXW.kind === img.id ? " sel" : ""));
    card.type = "button";
    card.innerHTML = `<span class="sbxw-img-label">${img.label}</span><span class="sbxw-img-size">${img.size}</span><span class="sbxw-img-hint">${img.hint}</span>`;
    card.onclick = () => {
      SBXW.kind = img.id;
      renderWizard();
    };
    grid.appendChild(card);
  }
  body.appendChild(grid);
  if (SBXW.kind === "custom") {
    const lab = sbxwField("image file path (iso or qcow2)", (lab) => {
      const inp = el("input");
      inp.value = SBXW.iso;
      inp.placeholder = "C:\\images\\debian-13-amd64.iso";
      inp.spellcheck = false;
      inp.oninput = () => {
        SBXW.iso = inp.value;
      };
      lab.appendChild(inp);
      setTimeout(() => inp.focus(), 0);
    });
    body.appendChild(lab);
  }
  sbxwNav(body, 1);
}

function renderWizardStep2(body) {
  const grid = el("div", "grid3");
  grid.appendChild(sbxwNumField("disk, GiB", SBXW.disk, 5, 512, (v) => (SBXW.disk = v)));
  grid.appendChild(sbxwNumField("ram, MiB", SBXW.ram, 256, 65536, (v) => (SBXW.ram = v)));
  grid.appendChild(sbxwNumField("cpu cores", SBXW.cpus, 1, 32, (v) => (SBXW.cpus = v)));
  body.appendChild(grid);
  const grid2 = el("div", "grid3");
  grid2.appendChild(sbxwNumField("ssh port on the host", SBXW.port, 1024, 65535, (v) => (SBXW.port = v)));
  grid2.appendChild(
    sbxwField("login inside the VM", (lab) => {
      const inp = el("input");
      inp.value = SBXW.login;
      inp.placeholder = "derola";
      inp.spellcheck = false;
      inp.oninput = () => {
        SBXW.login = inp.value;
      };
      lab.appendChild(inp);
    })
  );
  body.appendChild(grid2);

  const nameLab = sbxwField("name", (lab) => {
    const inp = el("input");
    inp.value = SBXW.name;
    inp.placeholder = "my-sandbox";
    inp.spellcheck = false;
    inp.oninput = () => {
      SBXW.name = inp.value;
    };
    lab.appendChild(inp);
  });
  body.appendChild(nameLab);

  const rootLab = el("label", "sbxw-check");
  const chk = el("input");
  chk.type = "checkbox";
  chk.checked = SBXW.root;
  chk.onchange = () => {
    SBXW.root = chk.checked;
  };
  rootLab.appendChild(chk);
  rootLab.insertAdjacentHTML(
    "beforeend",
    `<span>allow root inside the sandbox <span class="hint">(passwordless sudo — the in-VM agent needs it to install itself)</span></span>`
  );
  body.appendChild(rootLab);
  sbxwNav(body, 2);
}

function renderWizardStep3(body) {
  const sum = el("div", "sbxw-summary");
  const rows = [
    ["image", sbxKindLabel(SBXW.kind)],
    ...(SBXW.kind === "custom" ? [["file", SBXW.iso]] : []),
    ["disk", `${SBXW.disk} GiB`],
    ["ram", fmtGiB(SBXW.ram)],
    ["cpu", `${SBXW.cpus} vcpu`],
    ["ssh", `:${SBXW.port}`],
    ["login", SBXW.login.trim() || "derola"],
    ["root", SBXW.root ? "allowed (passwordless sudo)" : "denied"],
  ];
  for (const [k, v] of rows) {
    const row = el("div");
    row.appendChild(el("span", null, k));
    const b = el("b", null, v);
    row.appendChild(b);
    sum.appendChild(row);
  }
  body.appendChild(sum);

  const hint = el("div", "hint sbxw-hint");
  hint.textContent =
    ["debian-trixie", "debian-trixie-std", "ubuntu-24.04", "ubuntu-24.04-std"].includes(SBXW.kind)
      ? "downloads the official cloud image and generates a cloud-init seed (your login + a generated ssh key); the first boot sets the user up and the card shows when ssh is ready — then you can open a terminal, install the agent inside, or attach the chat to this VM (the attach button on the card, or /sandbox attach in the TUI)"
      : "boots from your file as-is; the VM opens its own window, guest ssh is forwarded to the host port above";
  body.appendChild(hint);

  const create = el("button", "accent");
  create.innerHTML = `${icon("download")} create sandbox`;
  create.onclick = async () => {
    if (!SBXW.name.trim()) {
      showSbxMsg("name is required");
      SBXW.step = 2;
      renderWizard();
      return;
    }
    if (!sbxwLoginOk()) {
      showSbxMsg('login must be a-z, digits, "-" or "_" — starting with a letter, not "root"');
      SBXW.step = 2;
      renderWizard();
      return;
    }
    create.disabled = true;
    try {
      await invoke("sandbox_create", {
        spec: {
          name: SBXW.name.trim(),
          kind: SBXW.kind,
          login: SBXW.login.trim() || "derola",
          iso_path: SBXW.kind === "custom" ? SBXW.iso.trim() : null,
          disk_gib: SBXW.disk,
          ram_mib: SBXW.ram,
          cpus: SBXW.cpus,
          root: SBXW.root,
          ssh_port: SBXW.port,
        },
      });
      closeWizard();
      await refreshSandbox();
    } catch (e) {
      showSbxMsg(String(e));
      create.disabled = false;
    }
  };
  sbxwNav(body, 3, create);
}

function sbxwNav(body, step, primaryBtn) {
  const row = el("div", "sbxw-nav");
  if (step > 1) {
    const back = el("button", "ghost");
    back.textContent = "back";
    back.onclick = () => {
      SBXW.step--;
      renderWizard();
    };
    row.appendChild(back);
  }
  row.appendChild(el("span", "sbxw-fill"));
  if (step < 3) {
    const next = el("button", "accent");
    next.textContent = "next";
    next.onclick = () => {
      if (SBXW.step === 1 && SBXW.kind === "custom" && !SBXW.iso.trim()) {
        showSbxMsg("pick an image file path first");
        return;
      }
      if (SBXW.step === 2 && !sbxwLoginOk()) {
        showSbxMsg('login must be a-z, digits, "-" or "_" — starting with a letter, not "root"');
        return;
      }
      showSbxMsg("");
      SBXW.step++;
      renderWizard();
    };
    row.appendChild(next);
  } else if (primaryBtn) {
    row.appendChild(primaryBtn);
  }
  body.appendChild(row);
}

/* buttons */

$("btn-settings").onclick = openSettings;
$("btn-sandbox").onclick = () => toggleSandboxView();
$("sbx-new").onclick = () => {
  showSbxMsg("");
  openWizard();
};
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
  $("btn-send").classList.toggle("ready", $("input").value.trim().length > 0);
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
  if (sandboxOpen && e.key === "Escape" && SBXW && !confirmOpen && !askOpen && !settingsOpen && !filesOpen && !paletteOpen) {
    e.preventDefault();
    closeWizard();
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
