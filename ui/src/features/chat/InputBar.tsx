import { useEffect, useRef, useState } from "react";
import { Icon } from "@/lib/icons";
import { Hint, keyChips } from "@/lib/hint";
import { invoke } from "@/lib/tauri";
import { useStore, type MentionItem, type TodoEntry } from "@/store/app";
import { chipName, cn, fmtCost, fmtTokens } from "@/lib/utils";
import { ModelMenu } from "./ModelMenu";

let mfilesCache: string[] | null = null;
let mentionSeq = 0;
let magentsCache: MentionItem[] | null = null;

// dsh claim ghost hints: while the draft is a bare slash command with blank
// args, the expected arguments render after the caret in caption color
const SLASH_HINTS: Record<string, string> = {
  "/file": "path — attach a file to the next message",
  "/model": "name — switch model (bare = show current)",
  "/models": "list models available for the api key",
  "/profile": "name — switch provider profile (bare = list, none = base)",
  "/plan": "toggle plan mode (read-only research)",
  "/undo": "revert file changes of the last turn",
  "/redo": "reapply undone changes",
  "/init": "create or improve AGENTS.md for this project",
  "/compact": "summarize and shrink the conversation context",
  "/export": "path — save the session as markdown",
  "/sessions": "list saved sessions",
  "/resume": "id — switch to a saved session (latest by default)",
  "/mcpauth": "name — mcp oauth: status, start a flow or paste a code",
  "/mcpres": "server — list mcp resources and uri templates",
  "/mcpstatus": "per-server status: connected/failed/needs auth",
  "/mcpread": "server uri — read an mcp resource into the chat",
  "/mcpadd": "name url|command — add a server and connect it",
  "/mcpconnect": "name — (re)connect a configured server",
  "/mcpdisconnect": "name — drop the live connection",
  "/mcplog": "server — recent mcp log messages",
  "/jstools": "list user js tools (.hi-derola/tools/)",
  "/doctor": "environment self-check",
  "/update": "check github releases and update the binary",
  "/usage": "token/cost report for this session",
  "/sandbox": "start|stop|attach|detach <id|name> · bare = list",
  "/host": "add|check|attach|detach|del <name> · bare = list",
  "/skills": "list · toggle <name> · install <git-url>",
  "/crew": "new <goal> · add <name|role> · step · auto · stop",
  "/clear": "start a new session",
  "/quit": "exit",
  "/help": "show commands and tools",
};

// dsh slash menu icons: a quiet glyph per command family
const SLASH_ICONS: Record<string, string> = {
  file: "file",
  model: "spark",
  models: "list",
  profile: "sliders",
  plan: "eye",
  undo: "undo",
  redo: "redo",
  init: "pencil",
  compact: "list",
  export: "download",
  sessions: "list",
  resume: "play",
  mcpauth: "server",
  mcpres: "server",
  mcpstatus: "server",
  mcpread: "server",
  mcpadd: "server",
  mcpconnect: "server",
  mcpdisconnect: "server",
  mcplog: "server",
  jstools: "terminal",
  doctor: "check",
  update: "download",
  usage: "list",
  sandbox: "box",
  host: "server",
  skills: "skill",
  crew: "crew",
  clear: "trash",
  quit: "x",
  help: "command",
};

const SLASH_LIST = Object.entries(SLASH_HINTS).map(([name, hint]) => ({
  name,
  hint,
  icon: SLASH_ICONS[name.slice(1)] ?? "command",
}));

interface MentionState {
  items: MentionItem[];
  idx: number;
  from: number;
  to: number;
}

function Chips() {
  const attachments = useStore((s) => s.attachments);
  const detachChip = useStore((s) => s.detachChip);
  if (!attachments.length) return null;
  return (
    <div className="mb-1.5 flex flex-wrap gap-1.5">
      {attachments.map((p, i) => (
        <span
          key={i}
          title={p}
          className="msg-in flex max-w-72 items-center gap-1.5 rounded-[10px] border-[0.5px] border-hairline bg-secondary py-1 pr-1 pl-2.5 text-[12px] text-foreground"
        >
          <Icon name="file" className="flex-none text-tertiary [&>svg]:size-3.5" />
          <span className="truncate">{chipName(p)}</span>
          <button
            className="rounded-[6px] p-0.5 text-caption transition-colors duration-100 hover:text-foreground"
            title="remove"
            onClick={() => detachChip(i)}
          >
            <Icon name="x" className="[&>svg]:size-3" />
          </button>
        </span>
      ))}
    </div>
  );
}

function PlusMenu() {
  const openFilesDlg = useStore((s) => s.openFiles);
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [open]);

  const pick = async (kind: "files" | "folder") => {
    setOpen(false);
    const st = useStore.getState();
    try {
      if (kind === "files") {
        const paths = await invoke<string[]>("pick_files");
        for (const p of paths) await st.attachPath(p);
        if (paths.length) st.toast(`attached ${paths.length} file(s)`);
      } else {
        const p = await invoke<string | null>("pick_folder");
        if (p) {
          await st.attachPath(p);
          st.toast("folder attached");
        }
      }
    } catch (e) {
      st.note(String(e));
    }
  };

  const item =
    "flex w-full items-center gap-2.5 rounded-[10px] px-2.5 py-1.5 text-left text-[12.5px] text-foreground/90 transition-colors duration-100 hover:bg-accent";

  return (
    <div ref={ref} className="relative">
      <Hint label="attach files or folders" side="top">
        <button
          aria-haspopup="menu"
          aria-expanded={open}
          onClick={() => setOpen(!open)}
          className="flex size-7 flex-none items-center justify-center rounded-full bg-secondary text-muted-foreground transition-all duration-150 hover:bg-muted hover:text-foreground active:scale-95"
        >
          <Icon name="plus" className={cn("[&>svg]:size-4 transition-transform duration-200", open && "rotate-45")} />
        </button>
      </Hint>
      {open && (
        <div className="absolute bottom-full left-0 z-50 mb-2 w-60 animate-in fade-in slide-in-from-bottom-1 rounded-[14px] border-[0.5px] border-hairline bg-glass p-1 shadow-panel backdrop-blur-[40px] backdrop-saturate-150 duration-150">
          <button className={item} onMouseDown={(e) => { e.preventDefault(); pick("files"); }}>
            <Icon name="file" className="flex-none text-tertiary" />
            choose files…
          </button>
          <button className={item} onMouseDown={(e) => { e.preventDefault(); pick("folder"); }}>
            <Icon name="folder" className="flex-none text-tertiary" />
            choose folder…
          </button>
          <button
            className={item}
            onMouseDown={(e) => {
              e.preventDefault();
              setOpen(false);
              openFilesDlg();
            }}
          >
            <Icon name="search" className="flex-none text-tertiary" />
            browse project…
          </button>
          <div className="mt-0.5 border-t-[0.5px] border-hairline px-2.5 pt-1.5 pb-1 text-[11px] text-caption">or drop files anywhere</div>
        </div>
      )}
    </div>
  );
}

// dsh TodoPanel: collapsed plan dock above the composer — header with
// per-status counts, expandable list with state dots (done/ongoing/idle)
function TodoStateDot({ status }: { status: TodoEntry["status"] }) {
  if (status === "in_progress") {
    return (
      <svg className="todo-spin size-3.5 flex-none text-caption" viewBox="0 0 24 24" aria-hidden="true">
        <circle className="todo-spin-track" cx="12" cy="12" r="9.5" />
        <circle className="todo-spin-arc" cx="12" cy="12" r="9.5" />
      </svg>
    );
  }
  return (
    <span
      aria-hidden="true"
      className={cn(
        "flex size-2.5 flex-none items-center justify-center rounded-full",
        status === "completed" ? "bg-success" : status === "cancelled" ? "bg-caption/50" : "bg-caption/40"
      )}
    />
  );
}

function TodoPanel() {
  const todos = useStore((s) => s.todos);
  const [collapsed, setCollapsed] = useState(true);
  if (!todos.length) return null;
  const done = todos.filter((t) => t.status === "completed").length;
  const active = todos.filter((t) => t.status === "in_progress").length;
  const dropped = todos.filter((t) => t.status === "cancelled").length;
  const pending = todos.length - done - active - dropped;
  const progress = [
    ...done > 0 ? [`${done} done`] : [],
    ...active > 0 ? [`${active} active`] : [],
    ...pending > 0 ? [`${pending} pending`] : [],
    ...dropped > 0 ? [`${dropped} dropped`] : [],
  ].join("\u2002·\u2002");
  return (
    <section className="msg-in mx-2 mb-1 rounded-[12px] border-[0.5px] border-hairline bg-secondary/50" aria-label="todos">
      <button
        type="button"
        aria-expanded={!collapsed}
        onClick={() => setCollapsed((v) => !v)}
        className="flex h-8 w-full items-center gap-2 rounded-[12px] px-2.5 text-left transition-colors duration-100 [@media(hover:hover)]:hover:bg-accent/60"
      >
        <Icon name="list" className="flex-none text-caption [&>svg]:size-3.5" />
        <span className="flex-none text-[12.5px] font-medium text-foreground">todos</span>
        <span className="min-w-0 flex-1 truncate text-[11.5px] text-tertiary">{progress}</span>
        <Icon
          name="chevron"
          className={cn("flex-none text-caption transition-transform duration-100 [&>svg]:size-3.5", collapsed ? "-rotate-90" : "rotate-180")}
        />
      </button>
      {!collapsed && (
        <ul className="animate-in fade-in slide-in-from-top-1 max-h-52 overflow-y-auto px-2.5 pb-2 duration-150">
          {todos.map((t, i) => (
            <li key={i} className="flex items-start gap-2 py-1 text-[12.5px] leading-[18px]">
              <span className="mt-[3px] flex-none">
                <TodoStateDot status={t.status} />
              </span>
              <span
                className={cn(
                  "min-w-0 break-words",
                  t.status === "completed" && "text-tertiary",
                  t.status === "cancelled" && "text-caption line-through",
                  t.status === "in_progress" && "font-medium text-foreground",
                  t.status === "pending" && "text-muted-foreground"
                )}
              >
                {t.content}
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

// dsh QueueDock: visible strip of messages waiting behind the current run,
// each with inline edit / run-next / remove
function QueueRow({ text, index }: { text: string; index: number }) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(text);
  const taRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    if (!editing) return;
    const el = taRef.current;
    if (el) {
      el.focus();
      el.setSelectionRange(el.value.length, el.value.length);
    }
  }, [editing]);

  const stopEdit = () => {
    setEditing(false);
    setDraft(text);
  };

  const save = () => {
    const v = draft.trim();
    if (v && v !== text) void useStore.getState().queueEdit(index, v);
    setEditing(false);
  };

  if (editing) {
    return (
      <div className="msg-in mx-2 mb-1 rounded-[12px] border-[0.5px] border-info/40 bg-secondary/60 p-1.5">
        <textarea
          ref={taRef}
          value={draft}
          rows={2}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              save();
            } else if (e.key === "Escape") {
              e.preventDefault();
              stopEdit();
            }
          }}
          className="w-full resize-none bg-transparent px-1.5 py-1 text-[12.5px] leading-[19px] outline-none"
        />
        <div className="flex justify-end gap-1 pt-0.5">
          <button
            onClick={stopEdit}
            className="h-6 rounded-[8px] px-2 text-[11px] text-caption transition-colors duration-100 hover:bg-accent hover:text-foreground"
          >
            cancel
          </button>
          <button
            onClick={save}
            className="h-6 rounded-[8px] bg-info px-2.5 text-[11px] font-medium text-white transition-colors duration-100 hover:bg-info-hover active:scale-[0.97]"
          >
            save
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="group/qrow mx-2 mb-0.5 flex items-start gap-2 rounded-[12px] px-1.5 py-1 transition-colors duration-100 hover:bg-accent/60">
      <span className="mt-[2px] flex-none font-mono text-[10px] leading-[19px] text-caption">{index + 1}</span>
      <span className="line-clamp-2 min-w-0 flex-1 text-[12.5px] leading-[19px] break-words text-muted-foreground">{text}</span>
      <div className="flex flex-none items-center gap-0.5 opacity-0 transition-opacity duration-100 group-focus-within/qrow:opacity-100 [@media(hover:hover)]:group-hover/qrow:opacity-100">
        {index > 0 && (
          <Hint label="run next" side="left">
            <button
              onClick={() => void useStore.getState().queueMove(index, 0)}
              className="rounded-[7px] p-1 text-caption transition-colors duration-100 hover:bg-accent hover:text-foreground"
            >
              <Icon name="up" className="[&>svg]:size-3.5" />
            </button>
          </Hint>
        )}
        <Hint label="edit" side="left">
          <button
            onClick={() => setEditing(true)}
            className="rounded-[7px] p-1 text-caption transition-colors duration-100 hover:bg-accent hover:text-foreground"
          >
            <Icon name="pencil" className="[&>svg]:size-3.5" />
          </button>
        </Hint>
        <Hint label="remove" side="left">
          <button
            onClick={() => void useStore.getState().queueRemove(index)}
            className="rounded-[7px] p-1 text-caption transition-colors duration-100 hover:bg-accent hover:text-destructive"
          >
            <Icon name="x" className="[&>svg]:size-3.5" />
          </button>
        </Hint>
      </div>
    </div>
  );
}

function QueueDock() {
  const queue = useStore((s) => s.queue);
  const waiting = useStore((s) => s.waiting);
  const [open, setOpen] = useState(true);
  if (!queue.length) return null;
  return (
    <div className="msg-in mb-1.5 overflow-hidden rounded-[18px] border-[0.5px] border-hairline bg-card/70 shadow-[0_1px_8px_rgba(0,0,0,0.03)]">
      <button
        aria-expanded={open}
        onClick={() => setOpen(!open)}
        className="flex h-8 w-full items-center gap-2 px-3.5 text-left transition-colors duration-100 hover:bg-accent/40"
      >
        <Icon name="list" className="flex-none text-tertiary [&>svg]:size-3.5" />
        <span className="text-[12px] font-medium text-foreground">queued</span>
        <span className="rounded-full bg-secondary px-1.5 py-px font-mono text-[10px] font-semibold text-muted-foreground">{queue.length}</span>
        <span className="min-w-0 flex-1 truncate text-[11px] text-caption">{waiting ? "runs after the current task" : ""}</span>
        <Icon name="chevron" className={cn("flex-none text-caption transition-transform duration-150 [&>svg]:size-3.5", open && "rotate-180")} />
      </button>
      {open && (
        <div className="animate-in fade-in slide-in-from-top-1 px-1 pb-1.5 duration-150">
          {queue.map((text, i) => (
            <QueueRow key={i} text={text} index={i} />
          ))}
        </div>
      )}
    </div>
  );
}

// dsh RunningStatus: isolated live clock so transcript re-renders never
// reset it; shimmer label with elapsed time in 1m 23s form
function RunClock() {
  const [t0] = useState(() => Date.now());
  const [, setTick] = useState(0);
  useEffect(() => {
    const id = setInterval(() => setTick((n) => n + 1), 1000);
    return () => clearInterval(id);
  }, []);
  const s = Math.max(0, Math.floor((Date.now() - t0) / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor(s / 60) % 60;
  const dur = h > 0 ? `${h}h ${m}m ${s % 60}s` : s >= 60 ? `${m}m ${s % 60}s` : `${s}s`;
  return (
    <span className="flex shrink-0 items-center gap-1.5">
      <span className="pulse-dot size-1.5 flex-none rounded-full bg-info" />
      <span className="shimmer-text font-sans text-[12px] font-medium">working · {dur}</span>
    </span>
  );
}

function Dock() {
  const waiting = useStore((s) => s.waiting);
  const tokens = useStore((s) => s.tokens);
  const ctx = useStore((s) => s.ctx);
  const runCmd = useStore((s) => s.runCmd);
  const pct = ctx.limit > 0 && ctx.used > 0 ? Math.min(100, Math.round((ctx.used / ctx.limit) * 100)) : null;

  let stats = "";
  if (!waiting && (tokens.in || tokens.out)) {
    stats = `${fmtTokens(tokens.in)} in · ${fmtTokens(tokens.out)} out`;
    if (tokens.cached > 0) stats += ` (${fmtTokens(tokens.cached)} cached)`;
    if (tokens.cost > 0) stats += " · " + fmtCost(tokens.cost);
  }

  return (
    <div className="flex h-6 items-center gap-2 px-2.5 pt-0.5 font-mono text-[11px] text-caption">
      {pct !== null && (
        <button
          title={`context: ${fmtTokens(ctx.used)} of ${fmtTokens(ctx.limit)} tokens · click to /compact`}
          onClick={() => runCmd("/compact")}
          className={cn(
            "rounded-full border-[0.5px] px-2 py-px font-mono text-[10.5px] font-semibold transition-colors duration-100",
            pct >= 92
              ? "border-destructive/50 text-destructive hover:bg-destructive/10"
              : pct >= 80
                ? "border-warning/50 text-warning hover:bg-warning/10"
                : "border-hairline text-caption hover:text-muted-foreground"
          )}
        >
          {pct}% ctx
        </button>
      )}
      <span className="truncate">{stats}</span>
      <span className="flex-1" />
      {waiting && <RunClock />}
    </div>
  );
}

export function InputBar() {
  const doSend = useStore((s) => s.doSend);
  const phase = useStore((s) => s.phase);
  const keys = useStore((s) => s.keys);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const slashRef = useRef<HTMLDivElement>(null);
  const [mention, setMention] = useState<MentionState | null>(null);
  const [hasText, setHasText] = useState(false);
  const [ghost, setGhost] = useState<{ token: string; hint: string } | null>(null);
  const [slash, setSlash] = useState<{ items: typeof SLASH_LIST; idx: number } | null>(null);

  const updateGhost = () => {
    const el = inputRef.current;
    if (!el) {
      setGhost(null);
      setSlash(null);
      return;
    }
    const v = el.value;
    // dsh slash menu: while the draft is a slash token still being typed
    // (no space yet), matching commands list above the composer
    const mm = v.match(/^\/([a-z0-9_-]*)$/i);
    if (mm) {
      const q = mm[1].toLowerCase();
      const items = SLASH_LIST.filter((c) => c.name.slice(1).startsWith(q));
      setSlash(items.length ? { items, idx: 0 } : null);
    } else {
      setSlash(null);
    }
    // ghost hint once the token is complete and a space follows
    const m = v.match(/^\/([a-z0-9_-]*)[ \t]+$/i);
    const hint = m ? SLASH_HINTS["/" + m[1].toLowerCase()] : undefined;
    setGhost(m && hint ? { token: m[0], hint } : null);
  };

  // dsh menu dismissal: pointer outside the card and the composer closes it
  useEffect(() => {
    if (!slash) return;
    const onDown = (e: PointerEvent) => {
      const t = e.target as Node;
      if (slashRef.current?.contains(t)) return;
      if (slashRef.current?.closest("[data-composer-card]")?.contains(t)) return;
      setSlash(null);
    };
    document.addEventListener("pointerdown", onDown, true);
    return () => document.removeEventListener("pointerdown", onDown, true);
  }, [slash]);

  // focus stays in the textarea (combobox), so keyboard moves never scroll
  // the highlighted row into view — do it here, dsh MenuView
  useEffect(() => {
    if (!slash) return;
    slashRef.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
  }, [slash]);

  // accept a highlighted command: token settles into the draft with a
  // trailing space, which hands over to the ghost args hint
  const acceptSlash = (sl: { items: typeof SLASH_LIST; idx: number }, i: number) => {
    const el = inputRef.current;
    const c = sl.items[i];
    if (!el || !c) return;
    el.value = c.name + " ";
    const caret = el.value.length;
    el.setSelectionRange(caret, caret);
    setSlash(null);
    autosize();
    updateGhost();
    setHasText(true);
    el.focus();
  };

  const autosize = () => {
    const el = inputRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = Math.min(el.scrollHeight, 240) + "px";
  };

  const hideMention = () => setMention(null);

  const completeMention = (m: MentionState, idx: number) => {
    const el = inputRef.current;
    const it = m.items[idx];
    if (!el || !it) return;
    const v = it.v;
    el.value = el.value.slice(0, m.from) + "@" + v + " " + el.value.slice(m.to);
    const caret = m.from + v.length + 2;
    el.setSelectionRange(caret, caret);
    hideMention();
    autosize();
    el.focus();
  };

  const showMention = async () => {
    const el = inputRef.current;
    if (!el) return;
    const pos = el.selectionStart;
    const before = el.value.slice(0, pos);
    const m = before.match(/(?:^|\s)@([^\s@]*)$/);
    const seq = ++mentionSeq;
    if (!m) {
      hideMention();
      return;
    }
    if (!mfilesCache) {
      mfilesCache = await invoke<string[]>("list_project_files").catch(() => []);
      if (mfilesCache.length > 800) mfilesCache.length = 800;
    }
    if (!magentsCache) {
      const r = await invoke<{ agents: { name: string; description?: string }[] }>("list_agents").catch(() => ({ agents: [] }));
      magentsCache = (r.agents || []).map((a) => ({
        v: a.name,
        label: a.description ? a.name + ": " + a.description : a.name,
        agent: true,
      }));
      magentsCache.push({ v: "general", label: "general: full tool access", agent: true });
      magentsCache.push({ v: "explore", label: "explore: read-only exploration", agent: true });
    }
    if (seq !== mentionSeq) return;
    const q = m[1].toLowerCase();
    const agentItems = magentsCache.filter((a) => !q || a.v.toLowerCase().startsWith(q)).slice(0, 4);
    const fileItems = (q ? mfilesCache.filter((f) => f.toLowerCase().includes(q)) : mfilesCache)
      .slice(0, 7)
      .map((f) => ({ v: f, label: f, agent: false }));
    const items = [...agentItems, ...fileItems].slice(0, 7);
    if (!items.length) {
      hideMention();
      return;
    }
    setMention({ items, idx: 0, from: pos - m[1].length - 1, to: pos });
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    const el = inputRef.current;
    if (!el) return;
    // dsh slash menu: combobox pattern — the textarea keeps focus, arrows
    // walk the shared highlight, enter/tab settle, escape dismisses
    if (slash) {
      if (e.key === "ArrowDown" || e.key === "ArrowUp" || e.key === "Tab" || e.key === "Enter" || e.key === "Escape") {
        e.preventDefault();
        // stop the window handler: enter must not send the draft, escape
        // must not count as a stop-sequence press
        e.stopPropagation();
        if (e.key === "ArrowDown") setSlash({ ...slash, idx: (slash.idx + 1) % slash.items.length });
        else if (e.key === "ArrowUp") setSlash({ ...slash, idx: (slash.idx - 1 + slash.items.length) % slash.items.length });
        else if (e.key === "Tab" || e.key === "Enter") acceptSlash(slash, slash.idx);
        else setSlash(null);
        return;
      }
    }
    if (mention) {
      if (e.key === "ArrowDown" || e.key === "ArrowUp" || e.key === "Tab" || e.key === "Enter" || e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        if (e.key === "ArrowDown") setMention({ ...mention, idx: (mention.idx + 1) % mention.items.length });
        else if (e.key === "ArrowUp") setMention({ ...mention, idx: (mention.idx - 1 + mention.items.length) % mention.items.length });
        else if (e.key === "Tab" || e.key === "Enter") completeMention(mention, mention.idx);
        else hideMention();
        return;
      }
    }
    const st = useStore.getState();
    // escape-to-stop is handled globally (double-press sequence in App.tsx)
    if (st.waiting && st.keys.stop === "escape" && e.key === "Escape") {
      return;
    }
    if (st.keys.send === "enter" && e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      doSend(el.value);
      el.value = "";
      setHasText(false);
      setGhost(null);
      setSlash(null);
      autosize();
      hideMention();
      return;
    }
    if (st.keys.newline === "shift+enter" && e.key === "Enter" && e.shiftKey) {
      e.preventDefault();
      const s = el.selectionStart,
        epos = el.selectionEnd;
      el.value = el.value.slice(0, s) + "\n" + el.value.slice(epos);
      el.selectionStart = el.selectionEnd = s + 1;
      autosize();
    }
  };

  const send = () => {
    const el = inputRef.current;
    if (!el) return;
    doSend(el.value);
    el.value = "";
    setHasText(false);
    setGhost(null);
    setSlash(null);
    autosize();
    hideMention();
    el.focus();
  };

  return (
    <div id="input-bar" className="mx-auto w-full max-w-[800px]">
      <TodoPanel />
      <QueueDock />
      <Chips />
      <div className="relative">
        {/* dsh slash menu: candidate list anchored above the composer card,
            edge to edge; name left, description right-aligned; the textarea
            keeps focus (combobox) so rows are mousedown-handled */}
        {slash && (
          <div
            ref={slashRef}
            data-slash-menu=""
            className="absolute bottom-full left-0 right-0 z-40 mb-1 flex max-h-[400px] animate-in fade-in slide-in-from-bottom-1 flex-col overflow-hidden rounded-[16px] border-[0.5px] border-hairline bg-glass p-1 shadow-panel backdrop-blur-[40px] backdrop-saturate-150 duration-150"
          >
            <div className="flex-none px-2 pt-1.5 pb-1 text-[11px] leading-[15px] text-caption">commands</div>
            <div role="listbox" aria-label="slash commands" className="min-h-0 overflow-y-auto">
              {slash.items.map((c, i) => (
                <button
                  key={c.name}
                  type="button"
                  role="option"
                  aria-selected={i === slash.idx}
                  onMouseDown={(e) => {
                    e.preventDefault();
                    acceptSlash(slash, i);
                  }}
                  onMouseMove={() => {
                    if (i !== slash.idx) setSlash({ ...slash, idx: i });
                  }}
                  className={cn(
                    "flex min-h-[34px] w-full items-center gap-1.5 rounded-[10px] px-2 py-1.5 text-left text-[13px] leading-[20px] text-foreground transition-colors duration-100",
                    i === slash.idx && "bg-accent"
                  )}
                >
                  <Icon name={c.icon} className="flex-none text-tertiary [&>svg]:size-3.5" />
                  <span className="max-w-[40%] flex-none truncate">{c.name}</span>
                  <span className="min-w-0 flex-1 truncate text-right text-[12px] leading-[18px] text-caption">{c.hint}</span>
                </button>
              ))}
            </div>
          </div>
        )}
        {mention && (
          <div className="absolute bottom-full left-0 z-40 mb-2.5 max-h-64 min-w-72 max-w-[26rem] animate-in fade-in slide-in-from-bottom-1 overflow-y-auto rounded-[16px] border-[0.5px] border-hairline bg-glass p-1.5 shadow-panel backdrop-blur-[40px] backdrop-saturate-150 duration-150">
            {mention.items.map((p, i) => (
              <div
                key={p.v + i}
                onMouseDown={(e) => {
                  e.preventDefault();
                  completeMention(mention, i);
                }}
                className={cn(
                  "flex cursor-pointer items-center gap-2 overflow-hidden rounded-[10px] px-2.5 py-1.5 font-mono text-[12px] whitespace-nowrap transition-colors duration-100",
                  i === mention.idx ? "bg-accent text-foreground" : "text-foreground/90"
                )}
              >
                <Icon name={p.agent ? "spark" : "file"} className="flex-none text-tertiary" />
                <span className="truncate">{p.label}</span>
              </div>
            ))}
          </div>
        )}
        {/* dsh claim ghost hint: an invisible mirror of the token keeps the
            exact caret width, the expected arguments follow in caption color */}
        {ghost && (
          <div className="pointer-events-none absolute top-[14px] left-[18px] z-10 flex whitespace-pre text-[14px] leading-[22px]">
            <span className="invisible">{ghost.token}</span>
            <span className="text-caption">{ghost.hint}</span>
          </div>
        )}
        {/* composer card, dsh InputBar: 28px radius, soft elevation, blue caret */}
        <div data-composer-card="" className="flex flex-col gap-0.5 rounded-[28px] bg-card px-2 pt-2 pb-1.5 shadow-[0_0_0_0.5px_var(--border),0_4px_16px_rgba(0,0,0,0.04),0_0_24px_rgba(0,0,0,0.03)]">
          <textarea
            id="chat-input"
            ref={inputRef}
            rows={1}
            autoFocus
            spellCheck={false}
            placeholder="message · /help for commands · @ files"
            className="max-h-60 min-h-9 flex-1 resize-none bg-transparent px-2.5 py-1.5 text-[14px] leading-[22px] outline-none placeholder:text-caption"
            onInput={() => {
              autosize();
              showMention();
              updateGhost();
              setHasText(!!inputRef.current?.value.trim());
            }}
            onClick={hideMention}
            onKeyDown={onKeyDown}
          />
          <div className="flex items-center gap-1.5 px-0.5 pt-0.5 pb-0.5">
            <PlusMenu />
            <span className="flex-1" />
            <ModelMenu />
            {phase === "sending" ? (
              <span
                aria-label="starting"
                className="flex size-[34px] flex-none items-center justify-center rounded-full bg-info/90 text-white"
              >
                <span key="spin" className="animate-in fade-in zoom-in-50 size-4 animate-spin rounded-full border-2 border-white/30 border-t-white duration-100" />
              </span>
            ) : phase === "running" && !hasText ? (
              <Hint label="stop" keys={["esc", "esc"]}>
                <button
                  onClick={() => invoke("stop").catch(() => {})}
                  className="flex size-[34px] flex-none items-center justify-center rounded-full bg-info text-white transition-all duration-100 hover:bg-info-hover active:scale-95"
                >
                  <span key="stop" className="animate-in fade-in zoom-in-50 flex duration-100">
                    <Icon name="stop" className="[&>svg]:size-3.5" />
                  </span>
                </button>
              </Hint>
            ) : (
              <Hint label={phase === "running" ? "queue after the current task" : "send"} keys={keyChips(keys.send)}>
                <button
                  disabled={!hasText}
                  onClick={send}
                  className="flex size-[34px] flex-none items-center justify-center rounded-full bg-info text-white transition-all duration-100 hover:bg-info-hover active:scale-95 disabled:opacity-40 disabled:hover:bg-info"
                >
                  <span key={phase === "running" ? "queue" : "send"} className="animate-in fade-in zoom-in-50 flex duration-100">
                    <Icon name="send" className="[&>svg]:size-4" />
                  </span>
                </button>
              </Hint>
            )}
          </div>
        </div>
      </div>
      <Dock />
    </div>
  );
}
