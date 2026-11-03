import { create } from "zustand";
import { invoke, listen } from "@/lib/tauri";
import type {
  AskQuestion,
  Cfg,
  ChangeEntry,
  CrewPreset,
  CrewSaved,
  CrewState,
  DiffRow,
  DirList,
  Ev,
  HostInfo,
  InitState,
  OpenSession,
  SandboxDetect,
  SandboxInfo,
  SessionMeta,
  SkillInfo,
} from "@/lib/types";
import { effectiveKeys, KEY_ACTIONS, type EffKeys } from "./keysStore";
import { comboFromEvent, parseCombo, sameCombo } from "@/lib/keys";
import { md } from "@/lib/md";

export type ChatItem =
  | { kind: "user"; id: number; text: string }
  | { kind: "bot"; id: number; text: string }
  | { kind: "note"; id: number; text: string }
  | { kind: "error"; id: number; text: string }
  | { kind: "tool"; id: number; name: string; detail: string; diff?: DiffRow[] }
  | { kind: "toolout"; id: number; text: string }
  | { kind: "todo"; id: number; text: string }
  | { kind: "bg"; bgId: string; id: number; output: string; ended: boolean }
  | { kind: "think"; id: number; text: string; streaming: boolean; open: boolean };

export type View = "chat" | "sandbox" | "hosts" | "skills" | "crew";

export interface MentionItem {
  v: string;
  label: string;
  agent: boolean;
}

let uid = 1;
let initStarted = false;
const nextId = () => uid++;

let streamRaw: string | null = null;
let renderTimer: ReturnType<typeof setTimeout> | null = null;
let crewLiveTimer: ReturnType<typeof setTimeout> | null = null;
let crewTitleTimer: ReturnType<typeof setInterval> | null = null;
let sbxTimer: ReturnType<typeof setInterval> | null = null;
let crewTimer: ReturnType<typeof setInterval> | null = null;

export const SBX_IMAGES = [
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
    hint: "the full generic cloud image: more packages on board than the minimal one, same cloud-init seed and ssh wiring",
  },
  {
    id: "ubuntu-24.04",
    label: "Ubuntu 24.04 LTS minimal",
    size: "~300 MiB download",
    hint: "Canonical's trimmed cloud image: smaller and quicker to boot than the standard server image; the cloud-init seed works out of the box",
  },
  {
    id: "ubuntu-24.04-std",
    label: "Ubuntu 24.04 LTS standard",
    size: "~650 MiB download",
    hint: "the standard server cloud image: ships the full server toolset; the cloud-init seed works out of the box",
  },
  {
    id: "custom",
    label: "own image",
    size: "no download",
    hint: "an .iso boots as install media; a qcow2 / raw disk image boots directly (no seed: you configure the login yourself)",
  },
];

const SBX_LEGACY_LABELS: Record<string, string> = { nixos: "NixOS minimal (legacy)" };

export interface AskForm {
  props: NonNullable<AskQuestion["schema"]>["properties"];
  names: string[];
  required: string[];
  vals: Record<string, unknown>;
}

interface SbxWizard {
  step: number;
  kind: string;
  iso: string;
  name: string;
  login: string;
  disk: number;
  ram: number;
  cpus: number;
  root: boolean;
  port: number;
}

interface Store {
  ready: boolean;
  cfg: Cfg | null;
  keys: EffKeys;
  model: string;
  kind: string;
  cwd: string;
  sid: string;
  sessions: SessionMeta[];
  theme: string;
  plan: boolean;
  waiting: boolean;
  // composer button state machine: idle -> sending (spinner) -> running (stop)
  phase: "idle" | "sending" | "running";
  queue: string[];
  tokens: { in: number; out: number; cost: number; cached: number };
  ctx: { used: number; limit: number };
  changes: Record<string, ChangeEntry>;
  transcript: ChatItem[];
  streamText: string | null;
  attachments: string[];
  models: string[];
  sidebarHidden: boolean;
  view: View;

  confirm: { open: boolean; name: string; detail: string; diff: DiffRow[] } | null;
  ask: {
    open: boolean;
    qs: AskQuestion[];
    forms: (AskForm | null)[];
    sel: Set<number>[];
    custom: string[];
  } | null;
  files: { open: boolean; data: DirList | null; parent: string | null };
  settingsOpen: boolean;
  quitAsk: boolean;
  reviewOpen: boolean;
  reviewExpanded: Set<string>;
  palette: { open: boolean; query: string; idx: number };

  sbx: { qemu: SandboxDetect | null; dir: string; list: SandboxInfo[]; attached: string | null; lastJson: string };
  sbxWizard: SbxWizard | null;
  hosts: { list: HostInfo[]; attached: string | null };
  skills: { list: SkillInfo[] };
  crew: {
    state: CrewState | null;
    lastJson: string | null;
    live: Record<string, { author: string; raw: string }> | null;
    savedOpen: boolean;
    saved: CrewSaved[];
    presetsOpen: boolean;
    presets: CrewPreset[];
    memoryOpen: boolean;
    graphOpen: boolean;
    usage: { rows: { name: string; role: string; requests: number; input: number; output: number; cost: number }[]; total: { input: number; output: number; cost: number }; budget?: number; spent?: number } | null;
  };

  applyEvent: (ev: Ev) => void;
  toasts: { id: number; text: string }[];
  toast: (s: string) => void;
  setTheme: (t: string, persist: boolean) => void;
  toggleTheme: () => void;
  togglePlan: () => void;
  toggleSidebar: () => void;
  note: (s: string) => void;
  addMsg: (item: ChatItem) => void;
  removeItem: (id: number) => void;
  clearWelcomeItems: () => void;
  autoscrollFlag: number;

  refreshSessions: () => Promise<void>;
  openSession: (id: string) => Promise<void>;
  newChat: () => void;
  doSend: (text: string) => Promise<void>;
  runCmd: (text: string) => Promise<void>;
  stopGen: () => void;

  queueRemove: (index: number) => Promise<void>;
  queueMove: (index: number, to: number) => Promise<void>;
  queueEdit: (index: number, text: string) => Promise<void>;
  retryLast: () => Promise<void>;

  quitHide: () => void;
  quitNow: () => void;

  resolveConfirm: (ok: boolean, always?: boolean) => void;
  allowAll: () => void;
  setConfirmFeedback?: never;

  submitAsk: (text: string | null) => void;
  skipAsk: () => void;

  openFiles: () => void;
  closeFiles: () => void;
  browse: (path: string | null) => Promise<void>;
  attachPath: (path: string) => Promise<void>;
  detachChip: (index: number) => void;

  openReview: () => void;
  closeReview: () => void;

  setView: (v: View | null) => void;

  refreshSandbox: () => Promise<void>;
  openWizard: () => void;
  closeWizard: () => void;
  setWizard: (patch: Partial<SbxWizard>) => void;
  wizardNext: () => void;
  wizardBack: () => void;
  wizardCreate: () => Promise<void>;

  refreshHosts: () => Promise<void>;
  refreshSkills: () => Promise<void>;

  refreshCrew: () => Promise<void>;
  refreshCrewSaved: (force: boolean) => Promise<void>;
  refreshCrewPresets: (force: boolean) => Promise<void>;
  showCrewUsage: () => Promise<void>;
  crewApply: (d: { none?: boolean } & Partial<CrewState>) => void;

  saveSettings: (cfg: Cfg) => Promise<boolean>;
  setModels: (m: string[]) => void;

  init: () => Promise<void>;
  pollStart: (view: View) => void;
  pollStop: (view: View) => void;
}

function changesTotals(changes: Record<string, ChangeEntry>) {
  let adds = 0;
  let dels = 0;
  let files = 0;
  for (const [k, c] of Object.entries(changes)) {
    if (k.startsWith("__patch__")) continue;
    files++;
    adds += c.adds;
    dels += c.dels;
  }
  return { adds, dels, files };
}

function trackChanges(changes: Record<string, ChangeEntry>, ev: Extract<Ev, { t: "tool" }>) {
  const paths = ev.paths || [];
  if (!paths.length || !ev.diff || !ev.diff.length) return;
  const per: [string, number, number][] = [];
  let cur: string | null = null;
  let ca = 0;
  let cd = 0;
  const flush = () => {
    if (cur !== null) {
      per.push([cur, ca, cd]);
      cur = null;
      ca = 0;
      cd = 0;
    }
  };
  for (const r of ev.diff) {
    if (r.tag === 0 && typeof r.text === "string" && r.text.startsWith("--- ")) {
      flush();
      const label = r.text.slice(4).trim();
      if (paths.includes(label)) cur = label;
      continue;
    }
    if (r.tag === 1) ca++;
    else if (r.tag === 2) cd++;
  }
  flush();
  let adds = 0;
  let dels = 0;
  for (const r of ev.diff) {
    if (r.tag === 1) adds++;
    else if (r.tag === 2) dels++;
  }
  if (!adds && !dels) return;
  const next = { ...changes };
  if (per.length > 1) {
    for (const [p, a, d] of per) {
      if (!a && !d) continue;
      const old = next[p] || { adds: 0, dels: 0, rows: null };
      next[p] = { adds: old.adds + a, dels: old.dels + d, rows: null };
    }
    const key = "__patch__" + Date.now() + "_" + Math.random().toString(36).slice(2, 6);
    next[key] = { adds, dels, rows: ev.diff, label: `${ev.name}: ${ev.detail}` };
  } else {
    const p = per.length === 1 ? per[0][0] : paths[0];
    const a = per.length === 1 ? per[0][1] : adds;
    const d = per.length === 1 ? per[0][2] : dels;
    const old = next[p] || { adds: 0, dels: 0, rows: null };
    next[p] = { adds: old.adds + a, dels: old.dels + d, rows: ev.diff };
  }
  return next;
}

function crewGraphSvg(graph: { from: string; to: string; n: number }[], members: { name: string }[]) {
  const nodes = ["you", ...members.map((m) => m.name)];
  if (!nodes.length) return null;
  const W = 460, H = 260, R = 88, CX = W / 2, CY = H / 2;
  const pos = new Map<string, [number, number]>();
  nodes.forEach((n, i) => {
    const a = (i / nodes.length) * Math.PI * 2 - Math.PI / 2;
    pos.set(n, [CX + R * Math.cos(a), CY + R * Math.sin(a)]);
  });
  let svg = `<svg viewBox="0 0 ${W} ${H}" class="w-full h-auto block">`;
  svg += `<defs><marker id="crew-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0,0 L10,5 L0,10 z" class="fill-primary"/></marker></defs>`;
  const maxN = Math.max(1, ...(graph || []).map((g) => g.n));
  for (const g of graph || []) {
    const a = pos.get(g.from), b = pos.get(g.to);
    if (!a || !b) continue;
    const w = 1 + (g.n / maxN) * 4;
    svg += `<line x1="${a[0]}" y1="${a[1]}" x2="${b[0]}" y2="${b[1]}" stroke-width="${w.toFixed(1)}" marker-end="url(#crew-arrow)" class="stroke-primary" stroke-opacity="0.55" fill="none"/>`;
    const mx = (a[0] + b[0]) / 2, my = (a[1] + b[1]) / 2;
    svg += `<text x="${mx}" y="${my - 4}" text-anchor="middle" class="fill-muted-foreground" font-size="9">${g.n}</text>`;
  }
  for (const n of nodes) {
    const [x, y] = pos.get(n)!;
    svg += `<circle cx="${x}" cy="${y}" r="17" class="fill-card stroke-primary/60" stroke-width="1.5"${n === "you" ? ' stroke-dasharray="3 2"' : ""}/>`;
    svg += `<text x="${x}" y="${y + 4}" text-anchor="middle" class="fill-foreground" font-size="10" font-weight="600">${n.slice(0, 8)}</text>`;
  }
  svg += `</svg>`;
  return svg;
}

export const useStore = create<Store>((set, get) => ({
  ready: false,
  cfg: null,
  keys: effectiveKeys({}),
  model: "",
  kind: "openai",
  cwd: "",
  sid: "",
  sessions: [],
  theme: "dark",
  plan: false,
  waiting: false,
  phase: "idle",
  queue: [],
  tokens: { in: 0, out: 0, cost: 0, cached: 0 },
  ctx: { used: 0, limit: 0 },
  changes: {},
  transcript: [],
  streamText: null,
  attachments: [],
  models: [],
  sidebarHidden: localStorage.getItem("hiderola.sidebar") === "0",
  view: "chat",

  confirm: null,
  ask: null,
  files: { open: false, data: null, parent: null },
  settingsOpen: false,
  quitAsk: false,
  reviewOpen: false,
  reviewExpanded: new Set(),
  palette: { open: false, query: "", idx: 0 },

  sbx: { qemu: null, dir: "", list: [], attached: null, lastJson: "" },
  sbxWizard: null,
  hosts: { list: [], attached: null },
  skills: { list: [] },
  crew: {
    state: null,
    lastJson: null,
    live: null,
    savedOpen: true,
    saved: [],
    presetsOpen: true,
    presets: [],
    memoryOpen: false,
    graphOpen: false,
    usage: null,
  },

  autoscrollFlag: 0,
  toasts: [],
  toast: (s) => {
    const id = nextId();
    set((st) => ({ toasts: [...st.toasts.slice(-3), { id, text: s }] }));
    setTimeout(() => set((st) => ({ toasts: st.toasts.filter((t) => t.id !== id) })), 3200);
  },
  note: (s) => {
    get().clearWelcomeItems();
    set((st) => ({
      transcript: [...st.transcript, { kind: "note", id: nextId(), text: s }],
      autoscrollFlag: st.autoscrollFlag + 1,
    }));
  },
  addMsg: (item) => {
    get().clearWelcomeItems();
    set((st) => ({ transcript: [...st.transcript, item], autoscrollFlag: st.autoscrollFlag + 1 }));
  },
  removeItem: (id) => set((st) => ({ transcript: st.transcript.filter((t) => t.id !== id) })),
  clearWelcomeItems: () => {},
  setTheme: (t, persist) => {
    const name = t === "light" ? "light" : "dark";
    document.documentElement.classList.toggle("dark", name === "dark");
    set({ theme: name });
    if (persist) invoke("set_theme", { theme: name }).catch(() => {});
  },
  toggleTheme: () => get().setTheme(get().theme === "light" ? "dark" : "light", true),
  togglePlan: () => {
    const plan = !get().plan;
    set({ plan });
    invoke("set_plan", { on: plan }).catch(() => {});
    get().note(plan ? "plan mode on: read-only research" : "plan mode off");
  },
  toggleSidebar: () =>
    set((st) => {
      const v = !st.sidebarHidden;
      localStorage.setItem("hiderola.sidebar", v ? "0" : "1");
      return { sidebarHidden: v };
    }),

  refreshSessions: async () => {
    const list = await invoke<SessionMeta[]>("list_sessions").catch(() => []);
    set({ sessions: list });
  },

  openSession: async (id) => {
    if (get().waiting || get().confirm?.open) return;
    let st: OpenSession;
    try {
      st = await invoke<OpenSession>("open_session", { id });
    } catch (e) {
      get().note(String(e));
      return;
    }
    const items: ChatItem[] = [];
    for (const it of st.transcript) {
      if (it.k === "user") items.push({ kind: "user", id: nextId(), text: it.s });
      else if (it.k === "bot") items.push({ kind: "bot", id: nextId(), text: it.s });
      else if (it.k === "tool") {
        const i = it.s.indexOf(" ");
        items.push({
          kind: "tool",
          id: nextId(),
          name: i < 0 ? it.s : it.s.slice(0, i),
          detail: i < 0 ? "" : it.s.slice(i + 1, i + 81),
        });
      } else if (it.k === "toolout") items.push({ kind: "toolout", id: nextId(), text: it.s });
    }
    const changes: Record<string, ChangeEntry> = {};
    for (const c of st.changes || []) changes[c.path] = { adds: c.adds, dels: c.dels, rows: null };
    streamRaw = null;
    set({
      sid: st.id,
      phase: "idle",
      queue: st.queue || [],
      tokens: { in: st.tokens_in || 0, out: st.tokens_out || 0, cost: st.cost || 0, cached: 0 },
      ctx: { used: 0, limit: get().ctx.limit },
      changes,
      reviewExpanded: new Set(),
      transcript: items,
      streamText: null,
      autoscrollFlag: get().autoscrollFlag + 1,
    });
    if ((st.queue || []).length) {
      get().note(`${(st.queue || []).length} queued message(s) restored from the previous run; they will steer the next run`);
    }
    get().refreshSessions();
  },

  newChat: () => {
    if (get().waiting || get().confirm?.open) return;
    // always land on the chat view first, so any error note from the backend is visible
    set({ view: "chat" });
    // if the chat is already empty the backend may not emit "cleared" — reset locally
    if (!get().sid && get().transcript.length === 0 && streamRaw === null) {
      set({
        sid: "",
        ctx: { used: 0, limit: get().ctx.limit },
        changes: {},
        reviewExpanded: new Set(),
        transcript: [],
        streamText: null,
        autoscrollFlag: get().autoscrollFlag + 1,
      });
    }
    invoke("new_session").catch((e) => get().note(String(e)));
  },

  doSend: async (text) => {
    const st = get();
    const t = text.trim();
    if (!t || st.confirm?.open || st.ask?.open || st.settingsOpen || st.files.open || st.view !== "chat") return;
    const myId = nextId();
    set((s) => ({
      transcript: [...s.transcript, { kind: "user", id: myId, text: t }],
      autoscrollFlag: s.autoscrollFlag + 1,
    }));
    set({ waiting: true, phase: "sending" });
    let res: { cmd?: boolean; note?: string };
    try {
      res = await invoke<{ cmd?: boolean; note?: string }>("send", { text: t });
    } catch (e) {
      set((s) => ({
        transcript: s.transcript.filter((x) => x.id !== myId),
        waiting: false,
        phase: "idle",
      }));
      get().note(String(e));
      if (String(e).includes("api key")) set({ settingsOpen: true });
      return;
    }
    if (res.cmd) {
      set((s) => ({
        transcript: s.transcript.filter((x) => x.id !== myId),
        waiting: false,
        phase: "idle",
      }));
      if (res.note) get().note(res.note);
    }
  },

  runCmd: async (text) => {
    try {
      const res = await invoke<{ note?: string }>("send", { text });
      if (res && res.note) get().note(res.note);
    } catch (e) {
      get().note(String(e));
    }
  },

  stopGen: () => {
    invoke("stop").catch(() => {});
  },

  queueRemove: async (index) => {
    try {
      await invoke("queue_remove", { index });
    } catch (e) {
      get().note(String(e));
    }
  },
  queueMove: async (index, to) => {
    try {
      await invoke("queue_move", { index, to });
    } catch (e) {
      get().note(String(e));
    }
  },
  queueEdit: async (index, text) => {
    try {
      await invoke("queue_edit", { index, text });
    } catch (e) {
      get().note(String(e));
    }
  },
  retryLast: async () => {
    const st = get();
    if (st.waiting || st.confirm?.open || st.ask?.open) return;
    set({ waiting: true, phase: "sending" });
    try {
      await invoke("retry");
    } catch (e) {
      set({ waiting: false, phase: "idle" });
      get().note(String(e));
    }
  },

  quitHide: () => {
    set({ quitAsk: false });
    invoke("hide_window").catch(() => {});
  },
  quitNow: () => {
    set({ quitAsk: false });
    invoke("quit_now").catch(() => {});
  },

  resolveConfirm: (ok, always = false) => {
    set({ confirm: null, waiting: true });
    const fbEl = document.getElementById("confirm-feedback") as HTMLInputElement | null;
    const feedback = ok ? "" : (fbEl?.value || "").trim();
    invoke("confirm", { ok, feedback, always }).catch(() => {});
    if (!ok) get().note(feedback ? "rejected with feedback" : "denied");
    if (ok && always) get().note("always allowed: rule saved to config");
    document.getElementById("chat-input")?.focus();
  },
  allowAll: () => {
    set({ confirm: null, waiting: true });
    invoke("allow_all").catch(() => {});
    document.getElementById("chat-input")?.focus();
  },

  submitAsk: (text) => {
    set({ ask: null });
    invoke("answer", { text: text || "" }).catch(() => {});
    get().note(text ? "answered" : "no answer given");
  },
  skipAsk: () => {
    set({ ask: null });
    invoke("answer", { text: "" }).catch(() => {});
    get().note("skipped");
  },

  openFiles: () => {
    set({ files: { open: true, data: null, parent: null } });
    get().browse(get().cwd || null);
  },
  closeFiles: () => set({ files: { open: false, data: null, parent: null } }),
  browse: async (path) => {
    try {
      const d = await invoke<DirList>("list_dir", { path: path || null });
      set({ files: { open: true, data: d, parent: d.parent } });
    } catch (e) {
      get().note(String(e));
    }
  },
  attachPath: async (path) => {
    try {
      await invoke("attach_path", { path });
    } catch (e) {
      get().note(String(e));
    }
  },
  detachChip: (index) => {
    invoke("detach", { index }).catch(() => {});
  },

  openReview: () => set({ reviewOpen: true }),
  closeReview: () => set({ reviewOpen: false }),

  setView: (v) => {
    const cur = get().view;
    const next = v === null ? (cur === "chat" ? null : "chat") : cur === v && v !== "chat" ? "chat" : v;
    if (!next) return;
    set({ view: next });
    if (next === "sandbox") {
      set({ sbxWizard: null });
      get().refreshSandbox();
    } else if (next === "hosts") get().refreshHosts();
    else if (next === "skills") get().refreshSkills();
    else if (next === "crew") {
      get().refreshCrew();
      get().refreshCrewSaved(true);
      get().refreshCrewPresets(true);
    }
  },

  refreshSandbox: async () => {
    try {
      const cur = get().sbx;
      const [qemu, list] = await Promise.all([
        cur.qemu ? Promise.resolve(cur.qemu) : invoke<SandboxDetect>("sandbox_detect"),
        invoke<{ dir?: string; sandboxes: SandboxInfo[]; attached: string | null }>("sandbox_list"),
      ]);
      set({
        sbx: {
          qemu,
          dir: list.dir || "",
          list: list.sandboxes || [],
          attached: list.attached || null,
          lastJson: JSON.stringify(list.sandboxes || []),
        },
      });
    } catch (e) {
      get().note(String(e));
    }
  },

  openWizard: () => {
    const used = new Set(get().sbx.list.map((s) => s.spec.ssh_port));
    let port = 2222;
    while (used.has(port)) port++;
    set({
      sbxWizard: { step: 1, kind: "debian-trixie", iso: "", name: "", login: "derola", disk: 20, ram: 2048, cpus: 2, root: false, port },
    });
  },
  closeWizard: () => set({ sbxWizard: null }),
  setWizard: (patch) => set((st) => ({ sbxWizard: st.sbxWizard ? { ...st.sbxWizard, ...patch } : null })),
  wizardNext: () => {
    const w = get().sbxWizard;
    if (!w) return;
    if (w.step === 1 && w.kind === "custom" && !w.iso.trim()) {
      get().note("pick an image file path first");
      return;
    }
    const login = (w.login || "").trim();
    if (w.step === 2 && (!/^[a-z][a-z0-9_-]{0,31}$/.test(login) || login === "root")) {
      get().note('login must be a-z, digits, "-" or "_", starting with a letter, not "root"');
      return;
    }
    set({ sbxWizard: { ...w, step: w.step + 1 } });
  },
  wizardBack: () => set((st) => ({ sbxWizard: st.sbxWizard ? { ...st.sbxWizard, step: Math.max(1, st.sbxWizard.step - 1) } : null })),
  wizardCreate: async () => {
    const w = get().sbxWizard;
    if (!w) return;
    if (!w.name.trim()) {
      get().note("name is required");
      set({ sbxWizard: { ...w, step: 2 } });
      return;
    }
    const login = w.login.trim();
    if (!/^[a-z][a-z0-9_-]{0,31}$/.test(login) || login === "root") {
      get().note('login must be a-z, digits, "-" or "_", starting with a letter, not "root"');
      set({ sbxWizard: { ...w, step: 2 } });
      return;
    }
    try {
      await invoke("sandbox_create", {
        spec: {
          name: w.name.trim(),
          kind: w.kind,
          login: login || "derola",
          iso_path: w.kind === "custom" ? w.iso.trim() : null,
          disk_gib: w.disk,
          ram_mib: w.ram,
          cpus: w.cpus,
          root: w.root,
          ssh_port: w.port,
        },
      });
      set({ sbxWizard: null });
      await get().refreshSandbox();
    } catch (e) {
      get().note(String(e));
    }
  },

  refreshHosts: async () => {
    try {
      const d = await invoke<{ hosts: HostInfo[]; attached: string | null }>("host_list");
      set({ hosts: { list: d.hosts || [], attached: d.attached || null } });
    } catch (e) {
      get().note(String(e));
    }
  },

  refreshSkills: async () => {
    try {
      const d = await invoke<{ skills: SkillInfo[] }>("skills_list");
      set({ skills: { list: d.skills || [] } });
    } catch (e) {
      get().note(String(e));
    }
  },

  refreshCrew: async () => {
    try {
      const d = await invoke<CrewState>("crew_state");
      get().crewApply(d);
    } catch (e) {
      get().note(String(e));
    }
  },
  crewApply: (d) => {
    const state = d.none ? null : (d as CrewState);
    set((st) => ({
      crew: { ...st.crew, state, lastJson: JSON.stringify(d), live: null },
    }));
  },
  refreshCrewSaved: async (force) => {
    if (!force && !get().crew.savedOpen) return;
    try {
      const d = await invoke<{ crews: CrewSaved[] }>("crew_list");
      set((st) => ({ crew: { ...st.crew, saved: d.crews || [] } }));
    } catch (e) {
      get().note(String(e));
    }
  },
  refreshCrewPresets: async (force) => {
    if (!force && !get().crew.presetsOpen) return;
    try {
      const d = await invoke<{ presets: CrewPreset[] }>("crew_presets");
      set((st) => ({ crew: { ...st.crew, presets: d.presets || [] } }));
    } catch (e) {
      get().note(String(e));
    }
  },
  showCrewUsage: async () => {
    try {
      const u = await invoke<Store["crew"]["usage"]>("crew_usage");
      set((st) => ({ crew: { ...st.crew, usage: u } }));
    } catch (e) {
      get().note(String(e));
    }
  },

  saveSettings: async (cfg) => {
    try {
      await invoke("save", { cfg });
      set({ cfg, keys: effectiveKeys(cfg.keys), models: [] });
      return true;
    } catch {
      return false;
    }
  },
  setModels: (m) => set({ models: m }),

  pollStart: (view) => {
    if (view === "sandbox" && !sbxTimer) {
      sbxTimer = setInterval(async () => {
        if (get().view !== "sandbox") return;
        try {
          const list = await invoke<{ sandboxes: SandboxInfo[]; attached: string | null }>("sandbox_list");
          const j = JSON.stringify([list.sandboxes || [], list.attached || null]);
          if (j !== get().sbx.lastJson) {
            set((st) => ({
              sbx: { ...st.sbx, list: list.sandboxes || [], attached: list.attached || null, lastJson: j },
            }));
          }
        } catch {}
      }, 900);
    }
    if (view === "crew" && !crewTimer) {
      crewTimer = setInterval(async () => {
        if (get().view !== "crew") return;
        try {
          const d = await invoke<CrewState>("crew_state");
          const j = JSON.stringify(d);
          if (j !== get().crew.lastJson) {
            get().crewApply(d);
          }
        } catch {}
      }, 1500);
    }
  },
  pollStop: (view) => {
    if (view === "sandbox" && sbxTimer) {
      clearInterval(sbxTimer);
      sbxTimer = null;
    }
    if (view === "crew" && crewTimer) {
      clearInterval(crewTimer);
      crewTimer = null;
    }
  },

  applyEvent: (ev) => {
    switch (ev.t) {
      case "chunk": {
        if (streamRaw === null) {
          streamRaw = "";
          get().clearWelcomeItems();
          set((s) => ({
            transcript: [...s.transcript, { kind: "bot", id: nextId(), text: "" }],
            autoscrollFlag: s.autoscrollFlag + 1,
          }));
        }
        streamRaw += ev.s;
        if (!renderTimer) {
          renderTimer = setTimeout(() => {
            renderTimer = null;
            set((s) => {
              const items = [...s.transcript];
              for (let i = items.length - 1; i >= 0; i--) {
                if (items[i].kind === "bot") {
                  items[i] = { ...(items[i] as Extract<ChatItem, { kind: "bot" }>), text: streamRaw || "" };
                  break;
                }
              }
              return { transcript: items, streamText: streamRaw, phase: "running", autoscrollFlag: s.autoscrollFlag + 1 };
            });
          }, 90);
        }
        break;
      }
      case "reasoning": {
        set((s) => {
          const items = [...s.transcript];
          let last: Extract<ChatItem, { kind: "think" }> | undefined;
          for (let i = items.length - 1; i >= 0; i--) {
            if (items[i].kind === "think") {
              last = items[i] as Extract<ChatItem, { kind: "think" }>;
              break;
            }
            if (items[i].kind !== "bot") break;
          }
          if (last && last.streaming) {
            const idx = items.indexOf(last);
            items[idx] = { ...last, text: last.text + ev.s };
          } else {
            items.push({ kind: "think", id: nextId(), text: ev.s, streaming: true, open: true });
          }
          return { transcript: items, phase: "running", autoscrollFlag: s.autoscrollFlag + 1 };
        });
        break;
      }
      case "note": {
        if (ev.s) {
          get().note(ev.s);
          const m = ev.s.match(/task (bg-\d+) (finished|failed|was killed|killed)/);
          if (m) {
            const bgId = m[1];
            set((s) => ({
              transcript: s.transcript.map((it) =>
                it.kind === "bg" && it.bgId === bgId ? { ...it, ended: true } : it
              ),
            }));
          }
        }
        break;
      }
      case "tool": {
        set((s) => ({
          transcript: s.transcript.map((it) =>
            it.kind === "think" ? { ...it, streaming: false, open: false } : it
          ),
        }));
        const changes = trackChanges(get().changes, ev);
        get().clearWelcomeItems();
        set((s) => ({
          transcript: [
            ...s.transcript,
            { kind: "tool", id: nextId(), name: ev.name, detail: ev.detail, diff: ev.diff },
          ],
          ...(changes ? { changes } : {}),
          phase: "running",
          autoscrollFlag: s.autoscrollFlag + 1,
        }));
        break;
      }
      case "confirm": {
        set((s) => ({
          transcript: s.transcript.map((it) =>
            it.kind === "think" ? { ...it, streaming: false, open: false } : it
          ),
          confirm: { open: true, name: ev.name, detail: ev.detail, diff: ev.diff || [] },
        }));
        break;
      }
      case "ask": {
        let qs: AskQuestion[] = [];
        try {
          qs = JSON.parse(ev.args).questions || [];
        } catch {}
        if (!qs.length) {
          invoke("answer", { text: "" }).catch(() => {});
          break;
        }
        const forms = qs.map((q) => {
          const props = q.schema && q.schema.properties;
          if (!props || typeof props !== "object") return null;
          const names = Object.keys(props);
          if (!names.length) return null;
          const vals: Record<string, unknown> = {};
          const required = Array.isArray(q.schema!.required) ? q.schema!.required! : [];
          names.forEach((n) => {
            const d = props[n] || {};
            if (d.default !== undefined) vals[n] = d.type === "boolean" ? !!d.default : d.default;
            else if (d.type === "boolean") vals[n] = false;
          });
          return { props, names, required, vals };
        });
        set({
          ask: {
            open: true,
            qs,
            forms,
            sel: qs.map(() => new Set<number>()),
            custom: qs.map(() => ""),
          },
        });
        break;
      }
      case "todo": {
        set((s) => {
          const rest = s.transcript.filter((it) => it.kind !== "todo");
          if (!ev.s || ev.s === "(todo list is empty)") return { transcript: rest };
          return {
            transcript: [...rest, { kind: "todo", id: nextId(), text: ev.s }],
            autoscrollFlag: s.autoscrollFlag + 1,
          };
        });
        break;
      }
      case "bgout": {
        set((s) => {
          const items = [...s.transcript];
          let idx = items.findIndex((it) => it.kind === "bg" && it.bgId === ev.id);
          if (idx < 0) {
            items.push({ kind: "bg", bgId: ev.id, id: nextId(), output: "", ended: false });
            idx = items.length - 1;
          }
          const bg = items[idx] as Extract<ChatItem, { kind: "bg" }>;
          let output = bg.output + ev.s;
          if (output.length > 24000) output = output.slice(-18000);
          items[idx] = { ...bg, output };
          return { transcript: items, phase: "running", autoscrollFlag: s.autoscrollFlag + 1 };
        });
        break;
      }
      case "plan":
        set({ plan: !!ev.on });
        break;
      case "usage":
        set((s) => ({
          tokens: {
            in: s.tokens.in + ev.input,
            out: s.tokens.out + ev.output,
            cached: s.tokens.cached + (ev.cached || 0),
            cost: typeof ev.cost === "number" ? ev.cost : s.tokens.cost,
          },
          ctx: {
            used: typeof ev.ctx_used === "number" ? ev.ctx_used : s.ctx.used,
            limit: typeof ev.ctx_limit === "number" && ev.ctx_limit > 0 ? ev.ctx_limit : s.ctx.limit,
          },
        }));
        break;
      case "attachments":
        set({ attachments: ev.list || [] });
        break;
      case "sessions":
        set((s) => ({ sessions: ev.list || [], sid: ev.sid || s.sid }));
        break;
      case "cleared":
        streamRaw = null;
        set({
          sid: "",
          ctx: { used: 0, limit: get().ctx.limit },
          changes: {},
          reviewExpanded: new Set(),
          transcript: [],
          streamText: null,
          queue: [],
          phase: "idle",
          view: "chat",
        });
        break;
      case "done": {
        if (renderTimer) {
          clearTimeout(renderTimer);
          renderTimer = null;
        }
        set((s) => {
          let items = s.transcript.map((it) =>
            it.kind === "think" ? { ...it, streaming: false, open: false } : it
          );
          const hadStream = streamRaw !== null;
          streamRaw = null;
          if (hadStream) {
            for (let i = items.length - 1; i >= 0; i--) {
              if (items[i].kind === "bot") {
                items[i] = { ...(items[i] as Extract<ChatItem, { kind: "bot" }>), text: ev.text };
                break;
              }
            }
          } else if (ev.text) {
            items = [...items, { kind: "bot", id: nextId(), text: ev.text }];
          }
          return { transcript: items, streamText: null, waiting: false, phase: "idle", autoscrollFlag: s.autoscrollFlag + 1 };
        });
        break;
      }
      case "failed":
        set((s) => ({
          transcript: [
            ...s.transcript.map((it) => (it.kind === "think" ? { ...it, streaming: false, open: false } : it)),
            { kind: "error", id: nextId(), text: ev.s },
          ],
          waiting: false,
          phase: "idle",
          autoscrollFlag: s.autoscrollFlag + 1,
        }));
        break;
      case "idle":
        set({ waiting: false, phase: "idle" });
        break;
      case "queued":
        // either a fresh run is starting, or the user queued behind an active
        // run (streamText still live) — the next stream event flips it back
        set((s) => ({ waiting: true, phase: s.streamText !== null ? "running" : "sending" }));
        break;
      case "queue":
        set({ queue: ev.list || [] });
        break;
      case "quit_ask":
        set({ quitAsk: true });
        break;
      case "model":
        set({ model: ev.name, kind: ev.kind || get().kind });
        break;
      case "theme":
        get().setTheme(ev.name, false);
        break;
      case "crew": {
        const stCrew = get().crew;
        if (get().view === "crew" && stCrew.live) {
          const key = `${ev.id || ""}|${ev.author}`;
          if (stCrew.live[key]) {
            set((s) => {
              const live = { ...(s.crew.live || {}) };
              delete live[key];
              return { crew: { ...s.crew, live } };
            });
          }
        }
        break;
      }
      case "crewchunk": {
        if (get().view !== "crew") break;
        set((s) => {
          const live = { ...(s.crew.live || {}) };
          const key = `${ev.id || ""}|${ev.author}`;
          live[key] = { author: ev.author, raw: (live[key]?.raw || "") + ev.s };
          return { crew: { ...s.crew, live }, autoscrollFlag: s.autoscrollFlag + 1 };
        });
        if (!crewLiveTimer) {
          crewLiveTimer = setTimeout(() => {
            crewLiveTimer = null;
          }, 90);
        }
        break;
      }
      case "bell":
        crewNotifyDone();
        break;
    }
  },

  init: async () => {
    if (initStarted) return;
    initStarted = true;
    await listen((payload) => get().applyEvent(payload as Ev));
    try {
      const st = await invoke<InitState>("init");
      const theme = st.theme === "light" ? "light" : "dark";
      document.documentElement.classList.toggle("dark", theme === "dark");
      const keys = effectiveKeys(st.keys || {});
      set({
        ready: true,
        cfg: st.cfg,
        keys,
        model: st.cfg?.provider?.model || "",
        kind: st.cfg?.provider?.type || "openai",
        cwd: st.cwd,
        sid: st.sid || "",
        sessions: st.sessions || [],
        theme,
        plan: !!st.plan,
      });
      if (localStorage.getItem("hiderola.sidebar") === "0" || window.innerWidth < 900) {
        set({ sidebarHidden: true });
      }
      if (st.todos) {
        set((s) => ({ transcript: [...s.transcript, { kind: "todo", id: nextId(), text: st.todos! }] }));
      }
      if (!st.has_provider) {
        get().note("no api key yet: press ctrl+comma or click the sliders icon to add one");
      }
    } catch (e) {
      get().note("init failed: " + e);
    }
  },
}));

function crewNotifyDone() {
  const get = () => useStore.getState();
  get().note("crew finished, goal marked done");
  if (get().view === "crew") {
    invoke<CrewState>("crew_state")
      .then((s) => get().crewApply(s))
      .catch(() => {});
  }
  const prev = document.title;
  if (crewTitleTimer) clearInterval(crewTitleTimer);
  let n = 0;
  crewTitleTimer = setInterval(() => {
    document.title = n % 2 === 0 ? "✔ crew DONE: Hi!Derola" : prev;
    if (++n >= 6) {
      clearInterval(crewTitleTimer!);
      crewTitleTimer = null;
      document.title = prev;
    }
  }, 700);
  (async () => {
    try {
      const np = window.__TAURI__ && (window.__TAURI__ as unknown as Record<string, { isPermissionGranted?: () => Promise<boolean>; requestPermission?: () => Promise<string>; sendNotification?: (o: { title: string; body: string }) => Promise<void> }>).notification;
      if (np) {
        let granted = np.isPermissionGranted ? await np.isPermissionGranted() : false;
        if (!granted && np.requestPermission) {
          granted = (await np.requestPermission()) === "granted";
        }
        if (granted && np.sendNotification) {
          await np.sendNotification({ title: "Hi!Derola crew", body: "Crew goal marked DONE." });
          return;
        }
      }
      if (window.Notification && Notification.permission === "granted") {
        new Notification("Hi!Derola crew", { body: "Crew goal marked DONE." });
      }
    } catch {}
  })();
}

export { changesTotals, crewGraphSvg, sbxKindLabel };
function sbxKindLabel(kind: string) {
  const img = SBX_IMAGES.find((i) => i.id === kind);
  if (img) return img.label;
  return SBX_LEGACY_LABELS[kind] || kind;
}

export { KEY_ACTIONS, comboFromEvent, parseCombo, sameCombo, md };
