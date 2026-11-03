import type { Cfg } from "./lib/types";

interface MockSession {
  id: string;
  title: string;
  created: number;
  updated: number;
  system: string;
  messages: Record<string, unknown>[];
  changes?: unknown[];
  autoTitled?: boolean;
}

const HOME = "/home/z/demo";

function load(k: string): unknown {
  try {
    return JSON.parse(localStorage.getItem(k) || "null");
  } catch {
    return null;
  }
}

const CFG_DEFAULT: Cfg = {
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

function seedSession(): MockSession {
  return {
    id: "s-1000-seed0001",
    title: "fix markdown rendering",
    created: Math.floor(Date.now() / 1000) - 7200,
    updated: Math.floor(Date.now() / 1000) - 7200,
    system: "",
    messages: [
      { role: "user", content: "fix markdown rendering for nested lists" },
      {
        role: "assistant",
        content:
          "Fixed. Nested lists now render with proper indentation.\n\n- before: flat\n- after: nested",
      },
    ],
  };
}

export function installMock() {
  if (window.__TAURI__) return;
  // dev-preview hooks: drive queue/phase states from the console or e2e checks
  (window as unknown as Record<string, unknown>).__mock = {
    queue: (texts: string[]) => {
      queue.push(...texts);
      emit({ t: "queue", list: [...queue] });
    },
    queueClear: () => {
      queue.length = 0;
      emit({ t: "queue", list: [] });
    },
  };

  let CFG: Cfg = (load("hiderola.mock.cfg") as Cfg) || CFG_DEFAULT;
  let THEME: string = CFG.ui.theme || "dark";
  let sessions: MockSession[] = (load("hiderola.mock.sessions") as MockSession[]) || [seedSession()];
  let sid = "";
  let attachments: [string, string][] = [];
  let pendingConfirm: ((ok: boolean) => void) | null = null;
  let runAbort = false;
  let running = false;
  let queue: string[] = [];
  const listeners: ((e: { payload: unknown }) => void)[] = [];

  function saveState() {
    localStorage.setItem("hiderola.mock.cfg", JSON.stringify(CFG));
    localStorage.setItem("hiderola.mock.sessions", JSON.stringify(sessions));
  }
  function emit(payload: unknown) {
    for (const l of listeners) l({ payload });
  }
  const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

  function meta(s: MockSession) {
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
  function titleFrom(text: string) {
    const line = (text.split("\n")[0] || "").replace(/\s+/g, " ").trim();
    return line.length > 48 ? line.slice(0, 48) + "…" : line || "new chat";
  }
  function current(): MockSession | undefined {
    return sessions.find((s) => s.id === sid);
  }
  function blankSession(): MockSession {
    return { id: sid, title: "", created: 0, updated: 0, system: "", messages: [] };
  }

  const FS: Record<string, unknown> = {
    home: {
      z: {
        demo: {
          "README.md": "# demo\nA mock project used by the dev preview.\n",
          "Cargo.toml": '[package]\nname = "demo"\nversion = "0.1.0"\n',
          src: {
            "main.rs": 'fn main() {\n    println!("hello");\n}\n',
            "tools.rs": "pub struct Tools;\n",
          },
          ui: {
            "index.html": "<html></html>\n",
            "app.tsx": "export default 1;\n",
          },
        },
      },
    },
  };
  const HOME_DISPLAY = "~";

  function absDir() {
    return HOME;
  }
  function resolve(p: string) {
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
  function display(p: string) {
    return p === HOME ? HOME_DISPLAY : HOME_DISPLAY + p.slice(HOME.length);
  }
  function node(p: string): unknown {
    const parts = p.split("/").filter(Boolean);
    let n: unknown = FS;
    for (const part of parts) {
      if (typeof n !== "object" || n === null || !(part in (n as Record<string, unknown>))) return null;
      n = (n as Record<string, unknown>)[part];
    }
    return n;
  }
  function tree(n: Record<string, unknown>, depth: number, counter: { n: number }, out: string[]) {
    if (depth > 2 || counter.n > 200) return;
    for (const k of Object.keys(n).sort()) {
      counter.n++;
      if (typeof n[k] === "string") out.push("  " + k);
      else {
        out.push("  " + k + "/");
        tree(n[k] as Record<string, unknown>, depth + 1, counter, out);
      }
    }
  }

  function fakeRun(text: string) {
    const s = current();
    const msgs: Record<string, unknown>[] = [];
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
        emit({ t: "confirm", name: "bash", detail: "ls -la", diff: [] });
        const ok = await new Promise<boolean>((res) => (pendingConfirm = res));
        pendingConfirm = null;
        if (!ok) {
          emit({ t: "note", s: "user denied this action" });
          msgs.push({ role: "tool", content: "user denied this action", tool_call_id: "t1" });
        } else {
          emit({ t: "tool", name: "bash", detail: "ls -la", diff: [], paths: [] });
          await sleep(150);
          if (runAbort) return;
          emit({ t: "note", s: "README.md\nCargo.toml\nsrc/\nui/" });
          msgs.push({
            role: "assistant",
            content: "",
            tool_calls: [{ id: "t1", name: "bash", args: '{"command":"ls -la"}' }],
          });
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
            { tag: 2, text: '    println!("hello");' },
            { tag: 1, text: '    println!("hello from hi-derola");' },
            { tag: 0, text: "}" },
          ],
        });
        await sleep(200);
      }
      if (runAbort) return;
      const s2 = s;
      if (!s2) return;
      const answer = [
        "## Done\n",
        "Here is what I found in **demo**:\n\n",
        "- `src/main.rs` prints hello\n",
        "- `Cargo.toml` defines the package\n\n",
        '```rust\nfn main() {\n    println!("hello");\n}\n```\n\n',
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
      s2.messages.push(...msgs);
      s2.updated = Math.floor(Date.now() / 1000);
      if (!s2.title) s2.title = titleFrom(text);
      saveState();
      emitSessions();
      emit({ t: "done", text: answer });
      setTimeout(() => {
        if (!s2.autoTitled) {
          s2.autoTitled = true;
          s2.title = "Demo: " + titleFrom(text).split(" ").slice(0, 3).join(" ");
          saveState();
          emitSessions();
        }
      }, 600);
    })().finally(() => {
      running = false;
      if (queue.length) {
        const next = queue.shift()!;
        emit({ t: "queue", list: [...queue] });
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

  const commands: Record<string, (args?: Record<string, unknown>) => Promise<unknown>> = {
    async init() {
      const st =
        sessions.find((s) => s.id === sid) || sessions.sort((a, b) => b.updated - a.updated)[0];
      sid = st ? st.id : newId();
      if (!st) sessions.push(blankSession());
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
        title: "",
        theme: THEME,
      };
    },
    async set_theme(a) {
      THEME = String(a?.theme);
      CFG.ui.theme = THEME;
      saveState();
      return { ok: true, theme: THEME };
    },
    async list_sessions() {
      return sessions.map(meta).sort((a, b) => b.updated - a.updated);
    },
    async open_session(a) {
      const s = sessions.find((x) => x.id === a?.id);
      if (!s) throw "no session: " + a?.id;
      sid = s.id;
      const out: Record<string, unknown>[] = [];
      for (const m of s.messages) {
        const role = m.role as string;
        const content = (m.content as string) || "";
        if (role === "user") out.push({ k: "user", s: content });
        else if (role === "assistant") {
          if (content.trim()) out.push({ k: "bot", s: content });
          for (const c of (m.tool_calls as { name: string; args: string }[]) || [])
            out.push({ k: "tool", s: c.name + " " + c.args });
        } else if (role === "tool" && content.trim()) {
          out.push({ k: "toolout", s: content.split("\n").slice(0, 6).join("\n") });
        }
      }
      return {
        id: s.id,
        title: s.title,
        created: s.created,
        updated: s.updated,
        transcript: out,
        changes: s.changes || [],
        queue: [],
      };
    },
    async new_session() {
      const cur = current();
      if (cur && cur.messages.length) {
        cur.updated = Math.floor(Date.now() / 1000);
        if (!cur.title) cur.title = titleFrom("new chat");
      }
      sid = newId();
      sessions.push(blankSession());
      saveState();
      emit({ t: "cleared" });
      emitSessions();
    },
    async delete_session(a) {
      sessions = sessions.filter((s) => s.id !== a?.id);
      if (sid === a?.id) {
        sid = newId();
        sessions.push(blankSession());
        emit({ t: "cleared" });
      }
      saveState();
      emitSessions();
      return { ok: true };
    },
    async send(a) {
      const text = String(a?.text || "");
      const t = text.trim();
      if (!t) return { cmd: true, note: "" };
      if (t.startsWith("/")) {
        const sp = t.indexOf(" ");
        const cmd = t.slice(1, sp < 0 ? undefined : sp);
        const arg = sp < 0 ? "" : t.slice(sp + 1);
        switch (cmd) {
          case "help":
          case "h":
            return { cmd: true, note: "commands: /file /model /models /undo /redo /clear /help" };
          case "clear":
          case "new":
            await commands.new_session();
            return { cmd: true, note: "new session" };
          case "model":
            if (arg) {
              CFG.provider.model = arg.trim();
              saveState();
              emit({ t: "model", name: CFG.provider.model, kind: CFG.provider.type });
            }
            return { cmd: true, note: "model: " + CFG.provider.model };
          case "models":
            setTimeout(
              () => emit({ t: "note", s: "models (3):\ngpt-4.1-mini\ngpt-4.1\ngpt-4o" }),
              400
            );
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
        s = blankSession();
        sessions.push(s);
      }
      if (s.messages.length === 0 && !s.title) {
        s.title = titleFrom(text);
        s.created = Math.floor(Date.now() / 1000);
      }
      if (s.created === 0) s.created = Math.floor(Date.now() / 1000);
      const composed =
        attachments.map(([p, c]) => "[file: " + p + "]\n" + c + "\n\n").join("") + text;
      attachments = [];
      emitAttachments();
      const attached: string[] = [];
      const missing: string[] = [];
      const seen = new Set<string>();
      const re = /(^|\s)@([^\s@]+)/g;
      let m2: RegExpExecArray | null;
      while ((m2 = re.exec(text))) {
        const p = m2[2].replace(/[.,;:)\]!]+$/, "");
        if (!p || seen.has(p)) continue;
        seen.add(p);
        const n2 = node(resolve(p));
        if (typeof n2 === "string" && attached.length < 8) attached.push(p);
        else missing.push(p);
      }
      if (attached.length || missing.length) {
        let line = "";
        if (attached.length) line += "@mentions attached: " + attached.join(", ");
        if (missing.length) line += (line ? " · " : "") + "not found: " + missing.join(", ");
        emit({ t: "note", s: line });
      }
      if (running) {
        queue.push(composed);
        emit({ t: "queue", list: [...queue] });
        emit({ t: "note", s: "queued: will steer the current run" });
        return { cmd: false, queued: true };
      }
      s.messages.push({ role: "user", content: composed });
      saveState();
      emitSessions();
      fakeRun(composed);
      return { cmd: false };
    },
    async confirm(a) {
      if (pendingConfirm) pendingConfirm(!!a?.ok);
    },
    async allow_all() {
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
    async queue_remove(a) {
      const i = Number(a?.index);
      if (i >= 0 && i < queue.length) queue.splice(i, 1);
      emit({ t: "queue", list: [...queue] });
      return {};
    },
    async queue_move(a) {
      const i = Number(a?.index);
      const to = Math.max(0, Math.min(Number(a?.to) || 0, queue.length));
      if (i >= 0 && i < queue.length) {
        const [it] = queue.splice(i, 1);
        queue.splice(to, 0, it);
      }
      emit({ t: "queue", list: [...queue] });
      return {};
    },
    async queue_edit(a) {
      const i = Number(a?.index);
      const text = String(a?.text || "").trim();
      if (i >= 0 && i < queue.length && text) queue[i] = text;
      emit({ t: "queue", list: [...queue] });
      return {};
    },
    async retry() {
      if (running) throw "wait for the current run to finish";
      const s = current();
      if (!s || !s.messages.length) throw "nothing to retry";
      while (s.messages.length && s.messages[s.messages.length - 1].role !== "user") s.messages.pop();
      if (!s.messages.length || s.messages[s.messages.length - 1].role !== "user") throw "nothing to retry";
      saveState();
      emitSessions();
      fakeRun(s.messages[s.messages.length - 1].content as string);
      return {};
    },
    async list_models() {
      await sleep(400);
      return ["gpt-4.1-mini", "gpt-4.1", "gpt-4o", "o4-mini"];
    },
    async save(a) {
      CFG = a?.cfg as Cfg;
      if (CFG.ui && CFG.ui.theme) THEME = CFG.ui.theme;
      saveState();
      emit({ t: "model", name: CFG.provider.model, kind: CFG.provider.type });
      if (CFG.ui && CFG.ui.theme) emit({ t: "theme", name: CFG.ui.theme });
      return { ok: true };
    },
    async mcp_reconnect() {
      return ["mcp: no servers configured"];
    },
    async list_project_files() {
      const out: string[] = [];
      const walk = (n: Record<string, unknown>, prefix: string) => {
        for (const k of Object.keys(n).sort()) {
          if (typeof n[k] === "string") out.push(prefix + k);
          else walk(n[k] as Record<string, unknown>, prefix + k + "/");
        }
      };
      walk(FS.home as Record<string, unknown>, "");
      return out;
    },
    async list_dir(a) {
      const raw = a?.path ? String(a.path) : display(absDir());
      const target = resolve(raw);
      const n = node(target);
      if (n === null) throw "no such directory: " + raw;
      if (typeof n === "string") throw "not a directory";
      const rec = n as Record<string, unknown>;
      const names = Object.keys(rec).sort((x, y) => {
        const da = typeof rec[x] === "object";
        const db = typeof rec[y] === "object";
        return da === db ? x.toLowerCase().localeCompare(y.toLowerCase()) : da ? -1 : 1;
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
          dir: typeof rec[name] === "object",
          size: typeof rec[name] === "string" ? (rec[name] as string).length : 0,
        })),
      };
    },
    async pick_files() {
      return ["~/README.md", "~/src/main.rs"];
    },
    async pick_folder() {
      return "~/src";
    },
    async attach_path(a) {
      const target = resolve(String(a?.path || ""));
      const n = node(target);
      if (n === null) throw display(target) + ": no such file or directory";
      if (typeof n === "object") {
        const out = ["[folder: " + target + "]"];
        const counter = { n: 0 };
        tree(n as Record<string, unknown>, 0, counter, out);
        attachments.push([display(target) + "/ (" + counter.n + " entries)", out.join("\n")]);
        emitAttachments();
        return { ok: true, kind: "folder", entries: counter.n };
      }
      attachments.push([display(target), n as string]);
      emitAttachments();
      return { ok: true, kind: "file", size: (n as string).length };
    },
    async detach(a) {
      attachments.splice(Number(a?.index), 1);
      emitAttachments();
    },
  };

  commands.host_list = async () => ({ hosts: [], attached: null });
  commands.host_add = async (a) => ({
    id: "h-mock",
    name: a?.name || "host",
    label: `${a?.user || "root"}@${a?.host || "host"}:${a?.port || 22}`,
    state: "new",
    error: null,
    agent: null,
  });
  commands.host_del = async () => {};
  commands.host_check = async () => ({
    id: "h-mock",
    name: "host",
    label: "root@host:22",
    state: "failed",
    error: "mock: unreachable",
    agent: null,
  });
  commands.host_pubkey = async () => "ssh-ed25519 AAAA... mock";
  commands.host_attach = async () => "attached (mock)";
  commands.host_detach = async () => {};
  commands.host_terminal = async () => {};
  commands.skills_list = async () => ({
    skills: [
      { name: "review", description: "Review code like a senior", path: "/mock/review/SKILL.md", enabled: true },
    ],
  });
  commands.skills_toggle = async () => {};
  commands.skills_install = async () => 'installed "mock": 2 skills discovered';
  commands.crew_state = async () => ({ none: true, running: false });
  commands.crew_list = async () => ({ crews: [] });
  commands.crew_new = async () => ({ none: true });
  commands.crew_open = async () => ({ none: true });
  commands.crew_drop = async () => {};
  commands.crew_add = async () => ({ none: true });
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
  commands.crew_retry = async () => ({ none: true });
  commands.crew_preset_save = async () => ({ saved: "mock" });
  commands.crew_presets = async () => ({ presets: [] });
  commands.crew_preset_del = async () => {};
  commands.crew_spawn = async () => ({ none: true });
  commands.set_plan = async () => {};
  commands.answer = async () => {};
  commands.task_kill = async () => {};
  commands.list_agents = async () => ({ agents: [] });
  commands.mcp_auth = async () => ({});
  commands.mcp_resources = async () => [];
  commands.mcp_read_resource = async () => ({ text: "" });
  commands.mcp_subscribe = async () => {};
  commands.mcp_unsubscribe = async () => {};
  commands.mcp_prompts = async () => [];
  commands.mcp_templates = async () => [];
  commands.mcp_subscriptions = async () => [];
  commands.mcp_get_prompt = async () => [{ role: "user", text: "mock" }];
  commands.sandbox_detect = async () => ({ system_path: "/usr/bin/qemu", img_path: "/usr/bin/qemu-img", system_version: "QEMU emulator version 8.2.0", accel: "kvm" });
  commands.sandbox_list = async () => ({ dir: "/mock/sandboxes", sandboxes: [], attached: null });
  commands.sandbox_create = async () => ({});
  commands.sandbox_action = async () => {};
  commands.sandbox_attach = async () => {};
  commands.sandbox_detach = async () => {};
  commands.sandbox_agent_install = async () => {};
  commands.sandbox_ssh_terminal = async () => {};
  commands.sandbox_fwd_add = async () => {
    throw new Error("mock: create a sandbox first");
  };
  commands.sandbox_fwd_del = async () => {
    throw new Error("mock: create a sandbox first");
  };
  commands.sandbox_fetch_vm = async () => {
    throw new Error("mock: create a sandbox first");
  };

  window.__TAURI__ = {
    core: {
      invoke: (cmd, args) => {
        const fn = commands[cmd];
        if (!fn) return Promise.reject("unknown command: " + cmd);
        return fn(args || {}).catch((e) =>
          Promise.reject(typeof e === "string" ? e : String((e && e.message) || e))
        );
      },
    },
    event: {
      listen: async (_name, cb) => {
        listeners.push(cb);
        setTimeout(() => emit({ t: "note", s: "dev preview: backend is mocked (src/mock.ts)" }), 250);
        return () => {};
      },
    },
  };
}
