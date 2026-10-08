/* dev preview bridge: emulates the tauri backend when running outside the app */
(function () {
  if (window.__TAURI__) return;

  const HOME = "/home/z/demo";
  const CFG_DEFAULT = {
    provider: {
      type: "openai",
      model: "gpt-4.1-mini",
      base_url: null,
      api_key: "sk-mock",
      max_tokens: 4096,
      temperature: null,
      top_p: null,
      stream: true,
    },
    mcp: [],
    keys: {},
    ui: { theme: "dark" },
  };

  let CFG = load("hiderola.mock.cfg") || CFG_DEFAULT;
  let THEME = CFG.ui.theme || "dark";
  let sessions = load("hiderola.mock.sessions") || [seedSession()];
  let sid = "";
  let title = "";
  let attachments = [];
  let allowAllFlag = false;
  let pendingConfirm = null;
  let runAbort = false;
  let running = false;
  let queue = [];
  const listeners = [];

  function load(k) {
    try {
      return JSON.parse(localStorage.getItem(k));
    } catch {
      return null;
    }
  }
  function saveState() {
    localStorage.setItem("hiderola.mock.cfg", JSON.stringify(CFG));
    localStorage.setItem("hiderola.mock.sessions", JSON.stringify(sessions));
  }
  function emit(payload) {
    for (const l of listeners) l({ payload });
  }
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

  function meta(s) {
    return { id: s.id, title: s.title, created: s.created, updated: s.updated, count: s.messages.length };
  }
  function emitSessions() {
    emit({ t: "sessions", list: sessions.map(meta).sort((a, b) => b.updated - a.updated), sid });
  }
  function emitAttachments() {
    emit({ t: "attachments", list: attachments.map((a) => a[0]) });
  }
  function newId() {
    return "s-" + Math.floor(Date.now() / 1000) + "-" + Math.random().toString(16).slice(2, 10);
  }
  function titleFrom(text) {
    const line = (text.split("\n")[0] || "").replace(/\s+/g, " ").trim();
    return line.length > 48 ? line.slice(0, 48) + "…" : line || "new chat";
  }
  function seedSession() {
    return {
      id: "s-1000-seed0001",
      title: "fix markdown rendering",
      created: Math.floor(Date.now() / 1000) - 7200,
      updated: Math.floor(Date.now() / 1000) - 7200,
      system: "",
      messages: [
        { role: "user", content: "fix markdown rendering for nested lists" },
        { role: "assistant", content: "Fixed. Nested lists now render with proper indentation.\n\n- before: flat\n- after: nested" },
      ],
    };
  }
  function transcript(s) {
    const out = [];
    for (const m of s.messages) {
      if (m.role === "user") out.push({ k: "user", s: m.content });
      else if (m.role === "assistant") {
        if (m.content.trim()) out.push({ k: "bot", s: m.content });
        for (const c of m.tool_calls || []) out.push({ k: "tool", s: c.name + " " + c.args });
      } else if (m.role === "tool" && m.content.trim()) {
        out.push({ k: "toolout", s: m.content.split("\n").slice(0, 6).join("\n") });
      }
    }
    return out;
  }
  function current() {
    return sessions.find((s) => s.id === sid);
  }

  const FS = {
    home: {
      z: {
        demo: {
          "README.md": "# demo\nA mock project used by the dev preview.\n",
          "Cargo.toml": "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
          src: {
            "main.rs": "fn main() {\n    println!(\"hello\");\n}\n",
            "tools.rs": "// tools\npub struct Tools;\n",
          },
          ui: {
            "index.html": "<html></html>\n",
            "app.js": "// app\n",
          },
        },
      },
    },
  };
  const HOME_DISPLAY = "~";

  function resolve(p) {
    let t = p.trim();
    if (t === "~") return HOME;
    if (t.startsWith("~/")) t = HOME + t.slice(1);
    const base = t.startsWith("/") ? [] : absDir().split("/").filter(Boolean);
    for (const part of t.split("/").filter((x) => x && x !== ".")) {
      if (part === "..") base.pop();
      else base.push(part);
    }
    return "/" + base.join("/");
  }
  function absDir() {
    return HOME;
  }
  function display(p) {
    return p === HOME ? HOME_DISPLAY : HOME_DISPLAY + p.slice(HOME.length);
  }
  function node(p) {
    const parts = p.split("/").filter(Boolean);
    let n = FS;
    for (const part of parts) {
      if (typeof n !== "object" || n === null || !(part in n)) return null;
      n = n[part];
    }
    return n;
  }
  function tree(n, depth, counter, out) {
    if (depth > 2 || counter.n > 200) return;
    for (const k of Object.keys(n).sort()) {
      counter.n++;
      if (typeof n[k] === "string") out.push("  " + k);
      else {
        out.push("  " + k + "/");
        tree(n[k], depth + 1, counter, out);
      }
    }
  }

  function fakeRun(text) {
    const s = current();
    const msgs = [];
    runAbort = false;
    running = true;
    (async () => {
      emit({ t: "usage", input: 1200, output: 0, ctx_used: 1200, ctx_limit: 115200 });
      for (const piece of ["Let me check the project layout first.\n", "The entry point is src/main.rs.\n"]) {
        if (runAbort) return;
        emit({ t: "reasoning", s: piece });
        await sleep(150);
      }
      if (runAbort) return;
      emit({ t: "tool", name: "read_file", detail: "src/main.rs", diff: [], paths: [] });
      await sleep(200);
      if (runAbort) return;
      emit({ t: "note", s: "src/main.rs (3 lines)" });
      if (/bash|run |build|ls /i.test(text)) {
        emit({
          t: "confirm",
          name: "bash",
          detail: "ls -la",
          diff: [],
        });
        const ok = await new Promise((res) => (pendingConfirm = res));
        pendingConfirm = null;
        if (!ok) {
          emit({ t: "note", s: "user denied this action" });
          msgs.push({ role: "tool", content: "user denied this action", tool_call_id: "t1" });
        } else {
          emit({ t: "tool", name: "bash", detail: "ls -la", diff: [], paths: [] });
          await sleep(150);
          if (runAbort) return;
          emit({ t: "note", s: "README.md\nCargo.toml\nsrc/\nui/" });
          msgs.push({ role: "assistant", content: "", tool_calls: [{ id: "t1", name: "bash", args: "{\"command\":\"ls -la\"}" }] });
          msgs.push({ role: "tool", content: "README.md\nCargo.toml\nsrc/\nui/", tool_call_id: "t1" });
        }
      }
      if (runAbort) return;
      if (/edit|write|fix /i.test(text)) {
        if (runAbort) return;
        emit({
          t: "tool",
          name: "edit",
          detail: "src/main.rs",
          paths: ["src/main.rs"],
          diff: [
            { tag: 0, text: "fn main() {" },
            { tag: 2, text: "    println!(\"hello\");" },
            { tag: 1, text: "    println!(\"hello from hi-derola\");" },
            { tag: 0, text: "}" },
          ],
        });
        await sleep(200);
      }
      if (runAbort) return;
      const answer = [
        "## Done\n",
        "Here is what I found in **demo**:\n\n",
        "- `src/main.rs` prints hello\n",
        "- `Cargo.toml` defines the package\n\n",
        "```rust\nfn main() {\n    println!(\"hello\");\n}\n```\n\n",
        "See the [docs](https://example.com) for more.",
      ].join("");
      const chunks = answer.match(/[\s\S]{1,24}/g) || [];
      for (const c of chunks) {
        if (runAbort) return;
        emit({ t: "chunk", s: c });
        await sleep(35);
      }
      emit({ t: "usage", input: 800, output: 350, ctx_used: 2350, ctx_limit: 115200 });
      msgs.push({ role: "assistant", content: answer });
      s.messages.push(...msgs);
      s.updated = Math.floor(Date.now() / 1000);
      if (!s.title) s.title = titleFrom(text);
      saveState();
      emitSessions();
      emit({ t: "done", text: answer });
      setTimeout(() => {
        if (!s.autoTitled) {
          s.autoTitled = true;
          s.title = "Demo: " + titleFrom(text).split(" ").slice(0, 3).join(" ");
          saveState();
          emitSessions();
        }
      }, 600);
    })().finally(() => {
      running = false;
      if (queue.length) {
        const next = queue.shift();
        const s2 = current();
        if (s2) {
          s2.messages.push({ role: "user", content: next });
          s2.updated = Math.floor(Date.now() / 1000);
          saveState();
          emitSessions();
        }
        emit({ t: "queued" });
        fakeRun(next);
      }
    });
  }

  const commands = {
    async init() {
      const st = sessions.find((s) => s.id === sid) || sessions.sort((a, b) => b.updated - a.updated)[0];
      sid = st ? st.id : newId();
      if (!st) sessions.push({ id: sid, title: "", created: 0, updated: 0, system: "", messages: [] });
      return {
        cfg: CFG,
        keys: {
          send: "enter",
          newline: "shift+enter",
          stop: "escape",
          new_session: "ctrl+n",
          open_settings: "ctrl+comma",
          undo: "ctrl+z",
          redo: "ctrl+shift+z",
          toggle_thinking: "ctrl+t",
          toggle_sidebar: "ctrl+b",
          toggle_theme: "ctrl+shift+t",
        },
        cwd: display(absDir()),
        config_path: "~/.config/hi-derola/config.toml",
        has_provider: true,
        sessions: sessions.map(meta).sort((a, b) => b.updated - a.updated),
        sid,
        title,
        theme: THEME,
      };
    },
    async set_theme({ theme }) {
      THEME = theme;
      CFG.ui.theme = theme;
      saveState();
      return { ok: true, theme };
    },
    async list_sessions() {
      return sessions.map(meta).sort((a, b) => b.updated - a.updated);
    },
    async open_session({ id }) {
      const s = sessions.find((x) => x.id === id);
      if (!s) throw "no session: " + id;
      sid = id;
      return { id: s.id, title: s.title, created: s.created, updated: s.updated, transcript: transcript(s), changes: s.changes || [], queue: s.queue || [] };
    },
    async new_session() {
      const cur = current();
      if (cur && cur.messages.length) {
        cur.updated = Math.floor(Date.now() / 1000);
        if (!cur.title) cur.title = titleFrom("new chat");
      }
      sid = newId();
      sessions.push({ id: sid, title: "", created: 0, updated: 0, system: "", messages: [] });
      saveState();
      emit({ t: "cleared" });
      emitSessions();
    },
    async delete_session({ id }) {
      sessions = sessions.filter((s) => s.id !== id);
      if (sid === id) {
        sid = newId();
        sessions.push({ id: sid, title: "", created: 0, updated: 0, system: "", messages: [] });
        emit({ t: "cleared" });
      }
      saveState();
      emitSessions();
      return { ok: true, current: sid === id };
    },
    async send({ text }) {
      const t = text.trim();
      if (!t) return { cmd: true, note: "" };
      if (t.startsWith("/")) {
        const [cmd, arg] = [t.slice(1).split(" ")[0], t.slice(t.indexOf(" ") + 1)];
        switch (cmd) {
          case "help":
          case "h":
            return { cmd: true, note: "commands: /file /model /models /undo /redo /clear /help" };
          case "clear":
          case "new":
            await commands.new_session();
            return { cmd: true, note: "new session" };
          case "model":
            if (arg && arg !== "") {
              CFG.provider.model = arg.trim();
              saveState();
              emit({ t: "model", name: CFG.provider.model, kind: CFG.provider.type });
            }
            return { cmd: true, note: "model: " + CFG.provider.model };
          case "models":
            setTimeout(() => emit({ t: "note", s: "models (3):\ngpt-4.1-mini\ngpt-4.1\ngpt-4o" }), 400);
            return { cmd: true, note: "fetching models..." };
          case "file":
            if (!arg) return { cmd: true, note: "usage: /file <path>" };
            attachments.push([display(resolve(arg)), "mock content of " + arg]);
            emitAttachments();
            return { cmd: true, note: "attached " + arg };
          case "undo":
          case "redo":
            return { cmd: true, note: "nothing to " + cmd };
          default:
            return { cmd: true, note: "unknown command: /" + cmd + ", try /help" };
        }
      }
      let s = current();
      if (!s) {
        s = { id: sid, title: "", created: 0, updated: 0, system: "", messages: [] };
        sessions.push(s);
      }
      if (s.messages.length === 0 && !s.title) {
        s.title = titleFrom(text);
        s.created = Math.floor(Date.now() / 1000);
      }
      if (s.created === 0) s.created = Math.floor(Date.now() / 1000);
      const composed = attachments.map(([p, c]) => "[file: " + p + "]\n" + c + "\n\n").join("") + text;
      attachments = [];
      emitAttachments();
      const attached = [];
      const missing = [];
      const seen = new Set();
      const re = /(^|\s)@([^\s@]+)/g;
      let m2;
      while ((m2 = re.exec(text))) {
        const p = m2[2].replace(/[.,;:)\]!]+$/, "");
        if (!p || seen.has(p)) continue;
        seen.add(p);
        const n2 = node(resolve(p));
        if (typeof n2 === "string" && attached.length < 8) {
          attached.push(p);
        } else {
          missing.push(p);
        }
      }
      if (attached.length || missing.length) {
        let line = "";
        if (attached.length) line += "@mentions attached: " + attached.join(", ");
        if (missing.length) line += (line ? " · " : "") + "not found: " + missing.join(", ");
        emit({ t: "note", s: line });
      }
      if (running) {
        queue.push(composed);
        emit({ t: "note", s: "queued: will steer the current run" });
        return { cmd: false, queued: true };
      }
      s.messages.push({ role: "user", content: composed });
      saveState();
      emitSessions();
      fakeRun(composed);
      return { cmd: false };
    },
    async confirm({ ok }) {
      if (pendingConfirm) pendingConfirm(ok);
    },
    async allow_all() {
      allowAllFlag = true;
      if (pendingConfirm) pendingConfirm(true);
    },
    async stop() {
      runAbort = true;
      if (pendingConfirm) {
        pendingConfirm(false);
        pendingConfirm = null;
      }
      emit({ t: "note", s: "cancelled" });
      emit({ t: "idle" });
    },
    async list_models() {
      await sleep(400);
      return ["gpt-4.1-mini", "gpt-4.1", "gpt-4o", "o4-mini"];
    },
    async save({ cfg }) {
      CFG = cfg;
      if (cfg.ui && cfg.ui.theme) THEME = cfg.ui.theme;
      saveState();
      emit({ t: "model", name: cfg.provider.model, kind: cfg.provider.type });
      if (cfg.ui && cfg.ui.theme) emit({ t: "theme", name: cfg.ui.theme });
      return { ok: true };
    },
    async mcp_reconnect() {
      return ["mcp: no servers configured"];
    },
    async list_project_files() {
      const out = [];
      const walk = (n, prefix) => {
        for (const k of Object.keys(n).sort()) {
          if (typeof n[k] === "string") out.push(prefix + k);
          else walk(n[k], prefix + k + "/");
        }
      };
      walk(FS.home.z.demo, "");
      return out;
    },
    async list_dir({ path }) {
      const raw = path ? path : display(absDir());
      const target = resolve(raw);
      const n = node(target);
      if (n === null) throw "no such directory: " + raw;
      if (typeof n === "string") throw "not a directory";
      const names = Object.keys(n).sort((a, b) => {
        const da = typeof n[a] === "object";
        const db = typeof n[b] === "object";
        return da === db ? a.toLowerCase().localeCompare(b.toLowerCase()) : da ? -1 : 1;
      });
      const parts = target.split("/").filter(Boolean);
      parts.pop();
      return {
        path: display(target),
        cwd: display(absDir()),
        is_cwd: target === absDir(),
        parent: parts.length ? "/" + parts.join("/") : "/",
        entries: names.map((name) => ({
          name,
          dir: typeof n[name] === "object",
          size: typeof n[name] === "string" ? n[name].length : 0,
        })),
      };
    },
    async attach_path({ path }) {
      const target = resolve(path);
      const n = node(target);
      if (n === null) throw display(target) + ": no such file or directory";
      if (typeof n === "object") {
        const out = ["[folder: " + target + "]"];
        const counter = { n: 0 };
        tree(n, 0, counter, out);
        attachments.push([display(target) + "/ (" + counter.n + " entries)", out.join("\n")]);
        emitAttachments();
        return { ok: true, kind: "folder", entries: counter.n };
      }
      attachments.push([display(target), n]);
      emitAttachments();
      return { ok: true, kind: "file", size: n.length };
    },
    async detach({ index }) {
      attachments.splice(index, 1);
      emitAttachments();
    },
  };

  /* new panels: minimal mock data so the dev preview renders them */
  commands.host_list = async () => ({ hosts: [], attached: null });
  commands.host_add = async (a) => ({ id: "h-mock", name: a.name || "host", label: `${a.user || "root"}@${a.host || "host"}:${a.port || 22}`, state: "new", error: null, checked: null, agent: null });
  commands.host_del = async () => {};
  commands.host_check = async () => ({ id: "h-mock", name: "host", label: "root@host:22", state: "failed", error: "mock: unreachable", checked: 0, agent: null });
  commands.host_pubkey = async () => "ssh-ed25519 AAAA... mock";
  commands.host_attach = async () => "attached (mock)";
  commands.host_detach = async () => {};
  commands.host_terminal = async () => {};
  commands.host_exec = async () => ({ code: 0, stdout: "mock", stderr: "" });
  commands.skills_list = async () => ({ skills: [{ name: "review", description: "Review code like a senior", path: "/mock/review/SKILL.md", enabled: true }] });
  commands.skills_toggle = async () => {};
  commands.skills_install = async () => "installed \"mock\" — 2 skills discovered";
  commands.crew_state = async () => ({ none: true, running: false });
  commands.crew_list = async () => ({ crews: [], running: false });
  commands.crew_new = async () => ({ none: true });
  commands.crew_open = async () => ({ none: true });
  commands.crew_drop = async () => {};
  commands.crew_add = async () => ({ none: true });
  commands.crew_del = async () => ({ none: true });
  commands.crew_send = async () => ({ none: true });
  commands.crew_step = async () => ({ none: true });
  commands.crew_auto = async () => ({ none: true });
  commands.crew_stop = async () => {};
  commands.crew_usage = async () => ({ rows: [], total: { input: 0, output: 0, cost: 0 } });
  commands.crew_limit = async () => ({ none: true });
  commands.crew_budget = async () => ({ none: true });
  commands.crew_review = async () => ({ none: true });
  commands.crew_memo = async () => ({ none: true });
  commands.crew_forget = async () => ({ none: true });
  commands.set_plan = async () => {};
  commands.answer = async () => {};
  commands.task_kill = async () => {};
  commands.list_agents = async () => ({ agents: [] });
  commands.mcp_auth = async () => {};
  commands.mcp_resources = async () => ({ resources: [] });
  commands.mcp_read_resource = async () => ({ text: "" });
  commands.mcp_subscribe = async () => {};
  commands.mcp_unsubscribe = async () => {};
  commands.mcp_prompts = async () => ({ prompts: [] });
  commands.mcp_templates = async () => ({ templates: [] });
  commands.mcp_subscriptions = async () => ({ subs: [] });
  commands.mcp_get_prompt = async () => ({ text: "" });
  commands.sandbox_detect = async () => ({ vms: [] });
  commands.sandbox_list = async () => ({ dir: "", sandboxes: [], attached: null });
  commands.sandbox_create = async () => ({});
  commands.sandbox_action = async () => {};
  commands.sandbox_attach = async () => {};
  commands.sandbox_detach = async () => {};
  commands.sandbox_agent_install = async () => {};
  commands.sandbox_ssh_terminal = async () => {};
  commands.sandbox_ssh_exec = async () => ({ code: 0, stdout: "", stderr: "mock: no sandbox" });
  commands.sandbox_fwd_add = async () => { throw new Error("mock: create a sandbox first"); };
  commands.sandbox_fwd_del = async () => { throw new Error("mock: create a sandbox first"); };
  commands.sandbox_fetch_vm = async () => { throw new Error("mock: create a sandbox first"); };

  window.__TAURI__ = {
    core: {
      invoke: (cmd, args) => {
        const fn = commands[cmd];
        if (!fn) return Promise.reject("unknown command: " + cmd);
        return fn(args || {}).catch((e) => Promise.reject(typeof e === "string" ? e : String(e && e.message || e)));
      },
    },
    event: {
      listen: async (_name, cb) => {
        listeners.push(cb);
        setTimeout(() => emit({ t: "note", s: "dev preview: backend is mocked (ui/mock.js)" }), 250);
        return () => {};
      },
    },
  };
})();
