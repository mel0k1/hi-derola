const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);

let CFG = null;
let KEYS = {};
let MODEL = "";
let KIND = "openai";
let waiting = false;
let confirmOpen = false;
let settingsOpen = false;
let models = [];

let streamRaw = null;
let streamBody = null;
let renderTimer = null;
let think = null;
let tokens = { in: 0, out: 0 };

const KEY_ACTIONS = [
  ["send", "send message"],
  ["newline", "insert newline"],
  ["stop", "stop generation"],
  ["new_session", "new session"],
  ["open_settings", "open settings"],
  ["undo", "undo file changes"],
  ["redo", "redo file changes"],
  ["toggle_thinking", "toggle thinking"],
];

const DEFAULT_KEYS = Object.fromEntries(KEY_ACTIONS.map(([k]) => [k, ""]));
Object.assign(DEFAULT_KEYS, {
  send: "enter",
  newline: "shift+enter",
  stop: "escape",
  new_session: "ctrl+n",
  open_settings: "ctrl+comma",
  undo: "ctrl+z",
  redo: "ctrl+shift+z",
  toggle_thinking: "ctrl+t",
});

function fmtTokens(n) {
  return n < 1000 ? String(n) : (n / 1000).toFixed(1) + "k";
}

function applyStatus() {
  $("status-left").textContent = `hi-derola · ${KIND} · ${MODEL}`;
  const right = $("status-right");
  if (waiting) {
    right.textContent = "thinking...";
    right.classList.add("busy");
  } else {
    right.classList.remove("busy");
    right.textContent =
      tokens.in || tokens.out
        ? `${fmtTokens(tokens.in)} in · ${fmtTokens(tokens.out)} out`
        : "";
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
    case "note":
      note(ev.s);
      break;
    case "tool": {
      closeThink();
      const m = addMsg("tool");
      const head = el("div", "thead", `${ev.name} ${ev.detail}`);
      m.appendChild(head);
      addDiff(m, ev.diff);
      break;
    }
    case "confirm": {
      closeThink();
      confirmOpen = true;
      $("confirm-title").textContent = "run " + ev.name + "?";
      $("confirm-detail").textContent = ev.detail;
      $("confirm-diff").replaceChildren();
      addDiff($("confirm-diff"), ev.diff);
      $("confirm-overlay").classList.remove("hidden");
      $("input").blur();
      break;
    }
    case "usage":
      tokens.in += ev.input;
      tokens.out += ev.output;
      applyStatus();
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
    case "model":
      MODEL = ev.name;
      KIND = ev.kind || KIND;
      fillModelSelect();
      applyStatus();
      break;
  }
}

/* sending */

async function doSend() {
  const input = $("input");
  const text = input.value.trim();
  if (!text || waiting || confirmOpen || settingsOpen) return;
  input.value = "";
  autosize();
  let res;
  try {
    res = await invoke("send", { text });
  } catch (e) {
    note(String(e));
    if (String(e).includes("api key")) openSettings();
    return;
  }
  if (res.cmd) {
    if (res.note) note(res.note);
    return;
  }
  clearWelcome();
  const m = el("div", "msg you");
  m.appendChild(el("div", "who", "you"));
  m.appendChild(el("div", "body", text));
  $("chat-col").appendChild(m);
  autoscroll();
  waiting = true;
  applyStatus();
}

async function runCmd(text) {
  try {
    const res = await invoke("send", { text });
    if (res && res.note) note(res.note);
  } catch (e) {
    note(String(e));
  }
}

function newSession() {
  $("chat-col").replaceChildren();
  const w = el("div", "welcome", "hi-derola · new session · /help for commands");
  w.id = "welcome";
  $("chat-col").appendChild(w);
  streamRaw = null;
  streamBody = null;
  runCmd("/clear");
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

function resolveConfirm(ok) {
  confirmOpen = false;
  $("confirm-overlay").classList.add("hidden");
  invoke(ok ? "confirm" : "confirm", { ok });
  waiting = true;
  applyStatus();
  if (!ok) note("denied");
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
$("c-allow").onclick = () => allowAll();

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
  box.textContent = list.length
    ? list
        .map((m) => `${m.name}: ${m.type === "remote" || m.url ? "remote " + (m.url || "") : (m.command || "") + " " + (m.args || []).join(" ")}`)
        .join("\n")
    : "no servers configured (config.toml [mcp])";
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
$("btn-settings").onclick = openSettings;
$("btn-new").onclick = newSession;
$("btn-undo").onclick = () => runCmd("/undo");
$("btn-redo").onclick = () => runCmd("/redo");
$("btn-send").onclick = doSend;

/* input */

function autosize() {
  const input = $("input");
  input.style.height = "auto";
  input.style.height = Math.min(input.scrollHeight, 220) + "px";
}

$("input").addEventListener("input", autosize);

/* hotkeys */

window.addEventListener("keydown", (e) => {
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
    newSession();
  } else if (hit("open_settings")) {
    e.preventDefault();
    openSettings();
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
  }
});

/* init */

(async function init() {
  const st = await invoke("init");
  CFG = st.cfg;
  KEYS = effectiveKeys(st.keys);
  MODEL = CFG.provider.model || "";
  KIND = CFG.provider.type || "openai";
  $("welcome").textContent = `hi-derola · ${st.cwd} · /help for commands`;
  fillModelSelect();
  if (!st.has_provider) note("no api key yet — press ctrl+comma or click settings to add one");
  applyStatus();
  await listen("ev", (e) => handleEvent(e.payload));
  $("input").focus();
})();
