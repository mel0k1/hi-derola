import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { AlertDialog, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Icon } from "@/lib/icons";
import { useStore } from "@/store/app";
import { invoke } from "@/lib/tauri";
import { parseKVLines, cn, copyText } from "@/lib/utils";
import { comboFromEvent, comboToText } from "@/lib/keys";
import { KEY_ACTIONS } from "@/store/keysStore";
import type { Cfg, McpPrompt, McpResource, McpTemplate } from "@/lib/types";
import { useEffect, useRef, useState } from "react";

function HotkeyRow({ action, label, value, base, onChange }: { action: string; label: string; value: string; base: string; onChange: (v: string) => void }) {
  const [rec, setRec] = useState(false);
  const caps = value && value !== "none" ? value.split("+") : [];
  return (
    <div className="flex items-center gap-2 border-b-[0.5px] border-hairline py-2.5 last:border-b-0">
      <span className="flex-1 text-[13px]">{label}</span>
      {rec ? (
        <button
          autoFocus
          data-action={action}
          tabIndex={-1}
          onKeyDown={(e) => {
            e.preventDefault();
            e.stopPropagation();
            if (e.key === "Escape") {
              setRec(false);
              return;
            }
            if (["Control", "Shift", "Alt", "Meta"].includes(e.key)) return;
            const c = comboFromEvent(e.nativeEvent);
            if (c) {
              onChange(comboToText(c));
              setRec(false);
            }
          }}
          onBlur={() => setRec(false)}
          className="flex h-7 min-w-28 animate-pulse items-center justify-center rounded-[8px] border border-info/60 bg-info/10 px-2 text-center font-mono text-[11.5px] text-info outline-none"
        >
          press keys…
        </button>
      ) : (
        <button
          title="click to record a shortcut"
          onClick={() => setRec(true)}
          className="flex h-7 min-w-28 items-center justify-center gap-1 rounded-[8px] border-[0.5px] border-hairline bg-secondary px-2 transition-colors duration-100 hover:border-info/40 hover:bg-muted"
        >
          {caps.length ? (
            caps.map((c) => (
              <kbd key={c} className="rounded-[5px] border-[0.5px] border-hairline bg-background px-1.5 py-px font-mono text-[10.5px] text-foreground shadow-[0_1px_0_rgba(0,0,0,0.15)]">
                {c}
              </kbd>
            ))
          ) : (
            <span className="text-[11.5px] text-caption">none</span>
          )}
        </button>
      )}
      <button
        title="reset to default"
        onClick={() => {
          setRec(false);
          onChange(base);
        }}
        className="rounded-[7px] p-1 text-caption transition-colors duration-100 hover:bg-accent hover:text-foreground"
      >
        <Icon name="undo" className="[&>svg]:size-3.5" />
      </button>
      <button
        title="clear"
        onClick={() => {
          setRec(false);
          onChange("none");
        }}
        className="rounded-[7px] p-1 text-caption transition-colors duration-100 hover:bg-accent hover:text-destructive"
      >
        <Icon name="x" className="[&>svg]:size-3.5" />
      </button>
    </div>
  );
}

function ThemeCubes() {
  const theme = useStore((s) => s.theme);
  const setTheme = useStore((s) => s.setTheme);
  const cubes = [
    { id: "light", label: "light", icon: "sun", sw: "bg-[#f4f5f7] text-[#3c414b]" },
    { id: "dark", label: "dark", icon: "moon", sw: "bg-[#1b1b1d] text-[#dfe2e7]" },
  ];
  return (
    <div className="flex gap-2.5">
      {cubes.map((c) => (
        <button
          key={c.id}
          onClick={() => setTheme(c.id, true)}
          className={cn(
            "flex flex-1 flex-col items-center gap-2 rounded-[14px] border-[0.5px] p-2.5 transition-all duration-150",
            theme === c.id ? "border-info bg-info/8 ring-1 ring-info/30" : "border-hairline hover:border-foreground/25"
          )}
        >
          <span className={cn("flex h-14 w-full items-center justify-center rounded-[10px]", c.sw)}>
            <Icon name={c.icon} className="[&>svg]:size-5" />
          </span>
          <span className={cn("text-[12px]", theme === c.id ? "font-medium text-foreground" : "text-muted-foreground")}>{c.label}</span>
        </button>
      ))}
    </div>
  );
}

function McpSection({ mcp, onAdd, onRemove, setMsg }: { mcp: Cfg["mcp"]; onAdd: (entry: Cfg["mcp"][number]) => Promise<void>; onRemove: (m: Cfg["mcp"][number]) => void; setMsg: (s: string) => void }) {
  const note = useStore((s) => s.note);
  const [showForm, setShowForm] = useState(false);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [res, setRes] = useState<{ resources: McpResource[]; prompts: McpPrompt[]; templates: McpTemplate[]; subs: Set<string> } | null>(null);
  const [name, setName] = useState("");
  const [type, setType] = useState("stdio");
  const [command, setCommand] = useState("");
  const [args, setArgs] = useState("");
  const [env, setEnv] = useState("");
  const [cwd, setCwd] = useState("");
  const [url, setUrl] = useState("");
  const [headers, setHeaders] = useState("");
  const [timeout, setTimeoutV] = useState("");

  const list = mcp;

  const loadRes = async () => {
    const [r, p, t, s] = await Promise.all([
      invoke<McpResource[]>("mcp_resources").catch(() => [] as McpResource[]),
      invoke<McpPrompt[]>("mcp_prompts").catch(() => [] as McpPrompt[]),
      invoke<McpTemplate[]>("mcp_templates").catch(() => [] as McpTemplate[]),
      invoke<{ server: string; uri: string }[]>("mcp_subscriptions").catch(() => []),
    ]);
    setRes({ resources: r, prompts: p, templates: t, subs: new Set(s.map((x) => x.server + "|" + x.uri)) });
  };

  const addServer = async () => {
    if (!name.trim()) {
      setMsg("name is required");
      return;
    }
    if (list.some((m) => m.name === name.trim())) {
      setMsg("a server with this name already exists");
      return;
    }
    const entry: Record<string, unknown> = { name: name.trim(), type };
    if (type === "remote") {
      if (!url.trim()) {
        setMsg("url is required for remote servers");
        return;
      }
      entry.url = url.trim();
      const h = parseKVLines(headers, ":");
      if (Object.keys(h).length) entry.headers = h;
    } else {
      if (!command.trim()) {
        setMsg("command is required for stdio servers");
        return;
      }
      entry.command = command.trim();
      entry.args = args.split("\n").map((x) => x.trim()).filter(Boolean);
      const e = parseKVLines(env, "=");
      if (Object.keys(e).length) entry.env = e;
      if (cwd.trim()) entry.cwd = cwd.trim();
    }
    const t = Number(timeout);
    if (timeout.trim() && Number.isFinite(t) && t > 0) entry.timeout = t;
    setMsg("");
    await onAdd(entry as unknown as Cfg["mcp"][number]);
    setShowForm(false);
    setMsg("server added");
  };

  const removeServer = (m: Cfg["mcp"][number]) => onRemove(m);

  const intoInput = (text: string) => {
    const el = document.getElementById("chat-input") as HTMLTextAreaElement | null;
    if (el) {
      el.value = text;
      el.focus();
      el.dispatchEvent(new Event("input", { bubbles: true }));
    }
  };

  return (
    <section className="flex flex-col gap-2.5">
      <h3 className="font-mono text-[11.5px] font-semibold tracking-wider text-muted-foreground lowercase">mcp servers</h3>
      {list.length === 0 ? (
        <div className="text-[12px] whitespace-pre-wrap text-muted-foreground">no servers yet: add one below or edit config.toml [[mcp]]</div>
      ) : (
        <div className="flex flex-col gap-1.5">
          {list.map((m) => {
            const remote = m.type === "remote" || m.url;
            const isOpen = expanded === m.name;
            return (
              <div key={m.name}>
                <div className="flex flex-wrap items-center gap-1.5 text-[12.5px]">
                  <span className="min-w-0 flex-1 truncate">
                    {m.name}: {remote ? "remote " + m.url : (m.command || "") + " " + (m.args || []).join(" ")}
                  </span>
                  {remote && (
                    <Button variant="ghost" size="xs" onClick={async () => {
                      try {
                        const logs = await invoke<string[]>("mcp_auth", { name: m.name });
                        for (const l of logs) note(l);
                      } catch (e) {
                        const msg2 = String(e);
                        note(msg2);
                        if (msg2.includes("mcpauth") && msg2.includes("<code")) {
                          const u = msg2.split("\n").find((l) => l.startsWith("http"));
                          if (u) window.open(u, "_blank");
                          const code = prompt("paste the code from the redirect url:");
                          if (code && code.trim()) {
                            try {
                              const logs2 = await invoke<string[]>("mcp_auth", { name: m.name, code: code.trim() });
                              for (const l of logs2) note(l);
                            } catch (e2) {
                              note(String(e2));
                            }
                          }
                        }
                      }
                    }}>
                      auth
                    </Button>
                  )}
                  <Button
                    variant="ghost"
                    size="xs"
                    title="browse resources & prompts"
                    onClick={async () => {
                      if (isOpen) {
                        setExpanded(null);
                        return;
                      }
                      if (!res) await loadRes();
                      setExpanded(m.name);
                    }}
                  >
                    res
                  </Button>
                  <Button variant="ghost" size="icon-xs" className="text-muted-foreground hover:text-destructive" title="remove from config" onClick={() => removeServer(m)}>
                    <Icon name="trash" />
                  </Button>
                </div>
                {isOpen && res && (
                  <div className="mt-1 ml-4 flex flex-col items-start gap-1 font-mono text-[11.5px] text-muted-foreground">
                    <span>resources ({res.resources.filter((r) => r.server === m.name).length})</span>
                    {res.resources.filter((r) => r.server === m.name).map((r) => (
                      <div key={r.uri} className="flex w-full max-w-full items-center gap-1.5">
                        <Button
                          variant="ghost"
                          size="xs"
                          className="min-w-0 flex-1 justify-start font-mono"
                          title={(r.description ? r.description + ": " : "") + r.uri + (r.mime ? " (" + r.mime + ")" : "")}
                          onClick={async () => {
                            try {
                              const out = await invoke<{ text: string }>("mcp_read_resource", { server: r.server, uri: r.uri });
                              if (out.text.length > 100000) {
                                note(`resource too big for the input (${out.text.length} chars); ask the agent to read ${r.uri} via mcp_resource`);
                              } else {
                                intoInput(out.text);
                              }
                            } catch (e) {
                              note(String(e));
                            }
                          }}
                        >
                          <span className="truncate">{r.name || r.uri}</span>
                        </Button>
                        <Button
                          variant="ghost"
                          size="xs"
                          title="toggle subscription: resource updates land in the chat"
                          onClick={async () => {
                            try {
                              if (res.subs.has(r.server + "|" + r.uri)) {
                                await invoke("mcp_unsubscribe", { server: r.server, uri: r.uri });
                                res.subs.delete(r.server + "|" + r.uri);
                              } else {
                                await invoke("mcp_subscribe", { server: r.server, uri: r.uri });
                                res.subs.add(r.server + "|" + r.uri);
                              }
                              setRes({ ...res, subs: new Set(res.subs) });
                            } catch (e) {
                              note(String(e));
                            }
                          }}
                        >
                          {res.subs.has(r.server + "|" + r.uri) ? "unsub" : "sub"}
                        </Button>
                      </div>
                    ))}
                    {!res.resources.some((r) => r.server === m.name) && <span className="ml-2">none</span>}
                    <span>uri templates ({res.templates.filter((t) => t.server === m.name).length})</span>
                    {res.templates.filter((t) => t.server === m.name).map((t) => (
                      <Button
                        key={t.uri_template}
                        variant="ghost"
                        size="xs"
                        className="w-full max-w-full justify-start font-mono"
                        title={(t.description ? t.description + ": " : "") + t.uri_template + (t.mime ? " (" + t.mime + ")" : "")}
                        onClick={() => intoInput("/mcpread " + t.server + " " + t.uri_template)}
                      >
                        <span className="truncate">{t.name || t.uri_template}</span>
                      </Button>
                    ))}
                    {!res.templates.some((t) => t.server === m.name) && <span className="ml-2">none</span>}
                    <span>prompts ({res.prompts.filter((p) => p.server === m.name).length})</span>
                    {res.prompts.filter((p) => p.server === m.name).map((p) => (
                      <PromptRow key={p.name} p={p} intoInput={intoInput} note={note} />
                    ))}
                    {!res.prompts.some((p) => p.server === m.name) && <span className="ml-2">none</span>}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
      {showForm ? (
        <div className="flex flex-col gap-2 rounded-xl border border-border bg-secondary/40 p-3">
          <div className="grid grid-cols-2 gap-2.5">
            <div className="flex flex-col gap-1">
              <Label className="text-[12px] text-muted-foreground">name</Label>
              <input value={name} onChange={(e) => setName(e.target.value)} placeholder="my-server" spellCheck={false} className="h-8.5 rounded-lg border border-input bg-background px-2.5 font-mono text-[12.5px] outline-none focus:border-ring" />
            </div>
            <div className="flex flex-col gap-1">
              <Label className="text-[12px] text-muted-foreground">type</Label>
              <select value={type} onChange={(e) => setType(e.target.value)} className="h-8.5 rounded-lg border border-input bg-background px-2 font-mono text-[12.5px] outline-none focus:border-ring">
                <option value="stdio">stdio - local process</option>
                <option value="remote">remote - http / sse</option>
              </select>
            </div>
          </div>
          {type === "stdio" ? (
            <>
              <div className="flex flex-col gap-1">
                <Label className="text-[12px] text-muted-foreground">command</Label>
                <input value={command} onChange={(e) => setCommand(e.target.value)} placeholder="npx" spellCheck={false} className="h-8.5 rounded-lg border border-input bg-background px-2.5 font-mono text-[12.5px] outline-none focus:border-ring" />
              </div>
              <div className="flex flex-col gap-1">
                <Label className="text-[12px] text-muted-foreground">args</Label>
                <textarea value={args} onChange={(e) => setArgs(e.target.value)} rows={2} placeholder="one argument per line" className="resize-y rounded-lg border border-input bg-background px-2.5 py-1.5 font-mono text-[12.5px] outline-none focus:border-ring" />
              </div>
              <div className="flex flex-col gap-1">
                <Label className="text-[12px] text-muted-foreground">env</Label>
                <textarea value={env} onChange={(e) => setEnv(e.target.value)} rows={2} placeholder="KEY=value per line" className="resize-y rounded-lg border border-input bg-background px-2.5 py-1.5 font-mono text-[12.5px] outline-none focus:border-ring" />
              </div>
              <div className="flex flex-col gap-1">
                <Label className="text-[12px] text-muted-foreground">working dir</Label>
                <input value={cwd} onChange={(e) => setCwd(e.target.value)} placeholder="optional" spellCheck={false} className="h-8.5 rounded-lg border border-input bg-background px-2.5 font-mono text-[12.5px] outline-none focus:border-ring" />
              </div>
            </>
          ) : (
            <>
              <div className="flex flex-col gap-1">
                <Label className="text-[12px] text-muted-foreground">url</Label>
                <input value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://example.com/mcp" spellCheck={false} className="h-8.5 rounded-lg border border-input bg-background px-2.5 font-mono text-[12.5px] outline-none focus:border-ring" />
              </div>
              <div className="flex flex-col gap-1">
                <Label className="text-[12px] text-muted-foreground">headers</Label>
                <textarea value={headers} onChange={(e) => setHeaders(e.target.value)} rows={2} placeholder="Name: value per line" className="resize-y rounded-lg border border-input bg-background px-2.5 py-1.5 font-mono text-[12.5px] outline-none focus:border-ring" />
              </div>
            </>
          )}
          <div className="flex flex-col gap-1">
            <Label className="text-[12px] text-muted-foreground">timeout, seconds</Label>
            <input value={timeout} onChange={(e) => setTimeoutV(e.target.value)} type="number" min={1} placeholder="default" className="h-8.5 w-28 rounded-lg border border-input bg-background px-2.5 font-mono text-[12.5px] outline-none focus:border-ring" />
          </div>
          <div className="flex items-center gap-2">
            <Button size="sm" onClick={addServer}>add</Button>
            <Button variant="ghost" size="sm" onClick={() => setShowForm(false)}>cancel</Button>
            <span className="text-[11.5px] text-muted-foreground">saved to config.toml and connected</span>
          </div>
        </div>
      ) : (
        <div className="flex items-center gap-2">
          <Button variant="ghost" size="sm" onClick={() => setShowForm(true)}>
            <Icon name="plus" /> add server
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={async (e) => {
              const btn = e.currentTarget;
              btn.disabled = true;
              try {
                const logs = await invoke<string[]>("mcp_reconnect");
                for (const l of logs) note(l);
              } finally {
                btn.disabled = false;
              }
            }}
          >
            reconnect
          </Button>
        </div>
      )}
    </section>
  );
}

function PromptRow({ p, intoInput, note }: { p: McpPrompt; intoInput: (t: string) => void; note: (s: string) => void }) {
  const [showArgs, setShowArgs] = useState(false);
  const [vals, setVals] = useState<Record<string, string>>({});

  const run = async () => {
    try {
      const msgs = await invoke<{ role: string; text: string }[]>("mcp_get_prompt", { server: p.server, name: p.name, args: vals });
      const text = msgs.map((x) => (x.role !== "user" ? "[" + x.role + "]\n" : "") + x.text).join("\n\n");
      intoInput(text);
      setShowArgs(false);
    } catch (e) {
      note(String(e));
    }
  };

  if (!p.arguments || !p.arguments.length) {
    return (
      <Button
        variant="ghost"
        size="xs"
        className="w-full max-w-full justify-start font-mono"
        title={p.description || p.name}
        onClick={async () => {
          try {
            const msgs = await invoke<{ role: string; text: string }[]>("mcp_get_prompt", { server: p.server, name: p.name, args: {} });
            const text = msgs.map((x) => (x.role !== "user" ? "[" + x.role + "]\n" : "") + x.text).join("\n\n");
            intoInput(text);
          } catch (e) {
            note(String(e));
          }
        }}
      >
        <span className="truncate">{p.name}</span>
      </Button>
    );
  }

  return (
    <div className="ml-2 flex flex-col items-start gap-1">
      <Button variant="ghost" size="xs" className="font-mono" title={p.description || p.name} onClick={() => setShowArgs(!showArgs)}>
        {p.name}
      </Button>
      {showArgs && (
        <div className="flex flex-col gap-1.5 rounded-lg border border-border bg-background/60 p-2">
          {p.arguments.map((a) => (
            <div key={a.name} className="flex items-center gap-2">
              <label className="flex-none font-mono text-[11px]" title={a.description || a.name}>
                {a.required ? "*" : ""}
                {a.name}
              </label>
              <input
                placeholder={a.description || a.name}
                className="h-7 min-w-0 flex-1 rounded-md border border-input bg-background px-2 font-mono text-[11px] outline-none focus:border-ring"
                onChange={(e) => setVals((v) => ({ ...v, [a.name]: e.target.value }))}
                onKeyDown={(e) => e.key === "Enter" && run()}
              />
            </div>
          ))}
          <div className="flex gap-1.5">
            <Button variant="ghost" size="xs" onClick={run}>run</Button>
            <Button variant="ghost" size="xs" onClick={() => setShowArgs(false)}>cancel</Button>
          </div>
        </div>
      )}
    </div>
  );
}


const SECTIONS = [
  { id: "general", label: "general", icon: "sliders" },
  { id: "provider", label: "provider", icon: "server" },
  { id: "hotkeys", label: "hotkeys", icon: "command" },
  { id: "mcp", label: "mcp servers", icon: "box" },
  { id: "permissions", label: "permissions", icon: "warn" },
  { id: "about", label: "about", icon: "spark" },
];

// dsh permission presets: three coarse modes over the per-tool fields in
// config.toml; rules ([[permissions.rules]]) keep applying on top
type PermMode = "full" | "ask" | "custom";

const PERM_TOOLS: [string, string][] = [
  ["edit", "edits to project files"],
  ["write_file", "creating and overwriting files"],
  ["bash", "running shell commands"],
  ["sandbox", "local vm sandbox tools"],
  ["mcp", "mcp server tools"],
  ["webfetch", "fetching web pages"],
  ["websearch", "web search"],
  ["codesearch", "code search"],
  ["subagent", "spawning subagents"],
];

const PERM_MODES: Record<PermMode, { label: string; desc: string }> = {
  full: { label: "full access", desc: "the agent runs every tool without asking — only for tasks you trust" },
  ask: { label: "ask every time", desc: "every write, command and network call asks before running; reads stay free" },
  custom: { label: "custom per tool", desc: "pick allow, ask or deny for each tool below" },
};

const APP_VERSION = "0.3.9";
// real version comes from the tauri shell when available (global app api)
let VERSION = APP_VERSION;
try {
  const t = window.__TAURI__ as unknown as { app?: { getVersion?: () => Promise<string> } } | undefined;
  if (t?.app?.getVersion) {
    void t.app.getVersion().then((v) => {
      if (v) VERSION = v;
    });
  }
} catch {}

export function SettingsDialog() {
  const open = useStore((s) => s.settingsOpen);
  const cfg = useStore((s) => s.cfg);
  const keys = useStore((s) => s.keys);
  const theme = useStore((s) => s.theme);
  const models = useStore((s) => s.models);
  const setModels = useStore((s) => s.setModels);
  const saveSettings = useStore((s) => s.saveSettings);
  const note = useStore((s) => s.note);
  const chatFont = useStore((s) => s.chatFont);
  const setChatFont = useStore((s) => s.setChatFont);

  const [sec, setSec] = useState("general");
  const [form, setForm] = useState<Record<string, unknown>>({});
  const [keyOverrides, setKeyOverrides] = useState<Record<string, string>>({});
  const [msg, setMsg] = useState("");
  const [showKey, setShowKey] = useState(false);
  const [permMode, setPermMode] = useState<PermMode>("custom");
  const [permFields, setPermFields] = useState<Record<string, string>>({});
  const [permMenu, setPermMenu] = useState(false);
  const permMenuRef = useRef<HTMLDivElement>(null);
  const [riskOpen, setRiskOpen] = useState(false);
  const [riskAck, setRiskAck] = useState(false);

  useEffect(() => {
    if (!permMenu) return;
    const onDown = (e: MouseEvent) => {
      if (!permMenuRef.current?.contains(e.target as Node)) setPermMenu(false);
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [permMenu]);

  useEffect(() => {
    if (open && cfg) {
      const p = cfg.provider;
      setForm({
        type: p.type || "openai",
        base: p.base_url || "",
        key: p.api_key || "",
        model: p.model || "",
        maxTokens: p.max_tokens ?? "",
        temperature: p.temperature ?? "",
        topP: p.top_p ?? "",
        stream: p.stream !== false,
      });
      setShowKey(false);
      setKeyOverrides({ ...cfg.keys });
      // derive the permission mode from the per-tool fields
      const pm = (cfg.permissions || {}) as Record<string, unknown>;
      const pv = PERM_TOOLS.map(([k]) => (typeof pm[k] === "string" ? (pm[k] as string).toLowerCase() : ""));
      setPermMode(pv.every((t) => t === "allow") ? "full" : pv.every((t) => t === "ask") ? "ask" : "custom");
      setPermFields(Object.fromEntries(PERM_TOOLS.map(([k], i) => [k, pv[i]])));
      setMsg(p.active ? `profile "${p.active}" is active; its fields override the values below (switch with /profile in the chat)` : "");
      setSec("general");
    }
  }, [open, cfg]);

  if (!cfg) return null;
  const v = (k: string) => String(form[k] ?? "");
  const set = (k: string, val: unknown) => setForm((f) => ({ ...f, [k]: val }));

  const loadModels = async () => {
    setMsg("loading models...");
    try {
      const ms = await invoke<string[]>("list_models", {
        kind: v("type"),
        base_url: v("base").trim() || null,
        api_key: v("key").trim() || null,
      });
      setModels(ms);
      setMsg(ms.length + " models loaded");
    } catch (e) {
      setMsg(String(e));
    }
  };

  const buildCfg = (): Cfg => {
    const num = (k: string) => (v(k).trim() === "" ? null : Number(v(k)));
    const keysOut: Record<string, string> = {};
    for (const [action] of KEY_ACTIONS) {
      keysOut[action] = keyOverrides[action] !== undefined ? keyOverrides[action] || "none" : keys[action] || "none";
    }
    return {
      provider: {
        type: v("type"),
        model: v("model").trim(),
        base_url: v("base").trim() || null,
        api_key: v("key"),
        max_tokens: num("maxTokens"),
        temperature: num("temperature"),
        top_p: num("topP"),
        stream: !!form.stream,
        active: cfg.provider.active || null,
      },
      profiles: cfg.profiles,
      mcp: cfg.mcp,
      keys: keysOut,
      ui: { theme },
      agent: cfg.agent,
      permissions: (() => {
        const permOut: Record<string, unknown> = { ...(cfg.permissions || {}) };
        for (const [k] of PERM_TOOLS) {
          const val = permMode === "full" ? "allow" : permMode === "ask" ? "ask" : permFields[k] || null;
          if (val) permOut[k] = val;
          else delete permOut[k];
        }
        return permOut as Cfg["permissions"];
      })(),
      lsp: cfg.lsp,
      formatters: cfg.formatters,
    };
  };

  const save = async () => {
    if (!v("model").trim()) {
      setSec("provider");
      setMsg("model is required");
      return;
    }
    const ok = await saveSettings(buildCfg());
    if (!ok) {
      setMsg("save failed");
      return;
    }
    useStore.setState({ settingsOpen: false });
    useStore.getState().toast("settings saved");
  };

  const addMcp = async (entry: Cfg["mcp"][number]) => {
    const next: Cfg = { ...cfg, mcp: [...cfg.mcp, entry] };
    try {
      await invoke("save", { cfg: { ...buildCfg(), mcp: next.mcp } });
      useStore.setState({ cfg: next });
      const logs = await invoke<string[]>("mcp_reconnect");
      for (const l of logs) note(l);
    } catch (e) {
      note(String(e));
    }
  };

  const removeMcp = (m: Cfg["mcp"][number]) => {
    const next: Cfg = { ...cfg, mcp: cfg.mcp.filter((x) => x.name !== m.name) };
    useStore.setState({ cfg: next });
    invoke("save", { cfg: { ...buildCfg(), mcp: next.mcp } })
      .then(async () => {
        const logs = await invoke<string[]>("mcp_reconnect");
        for (const l of logs) note(l);
        note(`server ${m.name} removed from config`);
      })
      .catch((e) => note(String(e)));
  };

  const permRules = cfg.permissions?.rules || [];

  const field =
    "h-9 w-full rounded-[10px] border border-input bg-background px-2.5 font-mono text-[12.5px] outline-none transition-colors duration-100 focus-visible:border-info focus-visible:ring-1 focus-visible:ring-info/30";
  const fieldLabel = "text-[12px] text-muted-foreground";
  const secTitle = "font-mono text-[11.5px] font-semibold tracking-wider text-muted-foreground lowercase";

  return (
    <Dialog open={open} onOpenChange={(o) => !o && useStore.setState({ settingsOpen: false })}>
      <DialogContent
        className="grid h-[min(760px,92vh)] w-[min(840px,94vw)] grid-cols-[188px_minmax(0,1fr)] grid-rows-[auto_minmax(0,1fr)_auto] gap-0 overflow-hidden p-0"
        showCloseButton={false}
      >
        <DialogHeader className="col-span-2 flex-row items-center justify-between border-b border-border px-4 py-3">
          <DialogTitle className="text-[14px]">settings</DialogTitle>
          <Button variant="ghost" size="icon-sm" className="text-muted-foreground" onClick={() => useStore.setState({ settingsOpen: false })}>
            <Icon name="x" />
          </Button>
        </DialogHeader>

        <nav className="flex min-h-0 flex-col gap-0.5 overflow-y-auto border-r-[0.5px] border-hairline p-2">
          {SECTIONS.map((s) => (
            <button
              key={s.id}
              onClick={() => setSec(s.id)}
              className={cn(
                "flex h-9 flex-none items-center gap-2.5 rounded-[10px] px-3 text-left text-[13px] transition-colors duration-100",
                sec === s.id ? "bg-accent font-medium text-foreground" : "text-muted-foreground hover:bg-accent/60 hover:text-foreground"
              )}
            >
              <Icon name={s.icon} className="flex-none [&>svg]:size-4" />
              {s.label}
            </button>
          ))}
        </nav>

        <div key={sec} className="view-in min-h-0 overflow-y-auto px-6 py-5">
          {sec === "general" && (
            <div className="flex max-w-xl flex-col gap-6">
              <section className="flex flex-col gap-2.5">
                <h3 className={secTitle}>appearance</h3>
                <ThemeCubes />
                <div className="text-[11.5px] text-muted-foreground">applied instantly and saved to config.toml</div>
              </section>
              <section className="flex flex-col gap-2.5">
                <h3 className={secTitle}>chat</h3>
                <div className="flex items-center gap-2.5">
                  <Switch id="s-stream" checked={!!form.stream} onCheckedChange={(c) => set("stream", c)} />
                  <Label htmlFor="s-stream" className="text-[13px]">stream responses</Label>
                </div>
                <div className="text-[11.5px] text-muted-foreground">part of the saved form — press save to apply</div>
                {/* dsh FontSizeRow: stepper over --chat-font, applied live */}
                <div className="flex items-center gap-2.5 pt-1">
                  <span className="text-[13px]">chat font size</span>
                  <div className="ml-auto flex items-center gap-1">
                    <button
                      aria-label="decrease chat font size"
                      disabled={chatFont <= 12}
                      onClick={() => setChatFont(chatFont - 1)}
                      className="flex h-7 w-7 items-center justify-center rounded-[8px] text-[15px] text-muted-foreground transition-colors duration-100 hover:bg-accent hover:text-foreground active:scale-95 disabled:cursor-not-allowed disabled:opacity-40"
                    >
                      −
                    </button>
                    <span className="w-10 text-center font-mono text-[12px] text-foreground">{chatFont}px</span>
                    <button
                      aria-label="increase chat font size"
                      disabled={chatFont >= 18}
                      onClick={() => setChatFont(chatFont + 1)}
                      className="flex h-7 w-7 items-center justify-center rounded-[8px] text-[15px] text-muted-foreground transition-colors duration-100 hover:bg-accent hover:text-foreground active:scale-95 disabled:cursor-not-allowed disabled:opacity-40"
                    >
                      +
                    </button>
                    {chatFont !== 14 && (
                      <button
                        onClick={() => setChatFont(14)}
                        className="ml-1 text-[11.5px] text-caption transition-colors duration-100 hover:text-foreground"
                      >
                        reset
                      </button>
                    )}
                  </div>
                </div>
                <div className="text-[11.5px] text-muted-foreground">scales messages and markdown, stored locally, applied instantly</div>
              </section>
            </div>
          )}

          {sec === "provider" && (
            <div className="flex max-w-xl flex-col gap-4">
              <h3 className={secTitle}>provider</h3>
              <div className="grid grid-cols-2 gap-3">
                <div className="flex flex-col gap-1.5">
                  <Label className={fieldLabel}>type</Label>
                  <select value={v("type")} onChange={(e) => set("type", e.target.value)} className={field}>
                    <option value="openai">openai / compatible</option>
                    <option value="anthropic">anthropic</option>
                  </select>
                </div>
                <div className="flex flex-col gap-1.5">
                  <Label className={fieldLabel}>base url</Label>
                  <input value={v("base")} onChange={(e) => set("base", e.target.value)} placeholder="https://api.openai.com/v1" spellCheck={false} className={field} />
                </div>
              </div>
              <div className="flex flex-col gap-1.5">
                <Label className={fieldLabel}>api key</Label>
                <div className="relative">
                  <input type={showKey ? "text" : "password"} value={v("key")} onChange={(e) => set("key", e.target.value)} placeholder="sk-..." spellCheck={false} autoComplete="off" className={field + " pr-10"} />
                  <button
                    title={showKey ? "hide key" : "show key"}
                    onClick={() => setShowKey(!showKey)}
                    className="absolute top-1/2 right-1.5 -translate-y-1/2 rounded-[7px] p-1 text-caption transition-colors duration-100 hover:bg-accent hover:text-foreground"
                  >
                    <Icon name={showKey ? "eyeoff" : "eye"} className="[&>svg]:size-3.5" />
                  </button>
                </div>
              </div>
              <div className="flex flex-col gap-1.5">
                <Label className={fieldLabel}>model</Label>
                <input value={v("model")} onChange={(e) => set("model", e.target.value)} list="model-list" spellCheck={false} className={field} />
                <datalist id="model-list">
                  {models.map((m) => (
                    <option key={m} value={m} />
                  ))}
                </datalist>
              </div>
              <div className="flex items-center gap-2">
                <Button variant="ghost" size="sm" onClick={loadModels}>
                  <Icon name="download" className="[&>svg]:size-3.5" /> load models
                </Button>
                <span className="text-[11.5px] text-muted-foreground">fetches the list from the provider, then pick models right in the composer</span>
              </div>
              <div className="grid grid-cols-3 gap-3 pt-1">
                <div className="flex flex-col gap-1.5">
                  <Label className={fieldLabel}>max tokens</Label>
                  <input type="number" min={128} step={128} value={v("maxTokens")} onChange={(e) => set("maxTokens", e.target.value)} className={field} />
                </div>
                <div className="flex flex-col gap-1.5">
                  <Label className={fieldLabel}>temperature</Label>
                  <input type="number" step={0.1} min={0} max={2} value={v("temperature")} onChange={(e) => set("temperature", e.target.value)} className={field} />
                </div>
                <div className="flex flex-col gap-1.5">
                  <Label className={fieldLabel}>top p</Label>
                  <input type="number" step={0.05} min={0} max={1} value={v("topP")} onChange={(e) => set("topP", e.target.value)} className={field} />
                </div>
              </div>
            </div>
          )}

          {sec === "hotkeys" && (
            <div className="flex max-w-xl flex-col gap-3">
              <h3 className={secTitle}>hotkeys</h3>
              <div className="rounded-[12px] border-[0.5px] border-hairline px-3 py-1">
                {KEY_ACTIONS.map(([action, label]) => (
                  <HotkeyRow
                    key={action}
                    action={action}
                    label={label}
                    value={keyOverrides[action] !== undefined ? keyOverrides[action] || "none" : keys[action] || "none"}
                    base={keys[action] || "none"}
                    onChange={(val) => setKeyOverrides((kv) => ({ ...kv, [action]: val }))}
                  />
                ))}
              </div>
              {/* dsh ShortcutsRow: the settings entry to the full reference dialog */}
              <div className="flex items-center gap-2">
                <button
                  onClick={() => useStore.setState({ hotkeysOpen: true })}
                  className="h-8 rounded-[10px] bg-secondary px-3 text-[12.5px] text-foreground transition-colors duration-100 hover:bg-muted"
                >
                  open quick reference
                </button>
                <span className="text-[11.5px] text-muted-foreground">{keys.shortcuts || "none"}</span>
              </div>
              <div className="text-[11.5px] text-muted-foreground">click a shortcut to record a new combination · escape cancels recording · undo restores the default, x clears it</div>
            </div>
          )}

          {sec === "mcp" && <McpSection mcp={cfg.mcp} onAdd={addMcp} onRemove={removeMcp} setMsg={setMsg} />}

          {sec === "permissions" && (
            <div className="flex max-w-xl flex-col gap-4">
              <h3 className={secTitle}>permissions</h3>
              {/* dsh PermissionRow: title/desc left, preset selector pill right */}
              <div className="flex items-center gap-3 border-b-[0.5px] border-hairline pb-4">
                <div className="flex min-w-0 flex-1 flex-col gap-1 pr-6">
                  <span className="whitespace-nowrap text-[14px] text-foreground">{PERM_MODES[permMode].label}</span>
                  <span className="text-[12px] leading-[18px] text-muted-foreground">{PERM_MODES[permMode].desc}</span>
                </div>
                <div ref={permMenuRef} className="relative flex-none">
                  <button
                    aria-haspopup="menu"
                    aria-expanded={permMenu}
                    onClick={() => setPermMenu(!permMenu)}
                    className="flex h-9 items-center gap-2.5 rounded-[10px] bg-secondary px-3.5 text-[13px] text-foreground transition-colors duration-100 hover:bg-muted [@media(hover:hover)]:hover:bg-muted"
                  >
                    {PERM_MODES[permMode].label}
                    <Icon name="chevron" className={cn("flex-none text-tertiary transition-transform duration-150 [&>svg]:size-3.5", permMenu && "rotate-180")} />
                  </button>
                  {permMenu && (
                    <div className="absolute top-full right-0 z-50 mt-1.5 w-72 animate-in fade-in slide-in-from-top-1 rounded-[14px] border-[0.5px] border-hairline bg-glass p-1 shadow-panel backdrop-blur-[40px] backdrop-saturate-150 duration-150">
                      {(Object.keys(PERM_MODES) as PermMode[]).map((m) => (
                        <button
                          key={m}
                          onClick={() => {
                            setPermMenu(false);
                            if (m === "full") {
                              if (permMode === "full") return;
                              setRiskAck(false);
                              setRiskOpen(true);
                            } else {
                              setPermMode(m);
                            }
                          }}
                          className="flex w-full items-start gap-2.5 rounded-[10px] px-2.5 py-1.5 text-left transition-colors duration-100 hover:bg-accent"
                        >
                          <span className="min-w-0 flex-1">
                            <span className={cn("block text-[12.5px]", permMode === m ? "font-medium text-foreground" : "text-foreground/90")}>{PERM_MODES[m].label}</span>
                            <span className="block text-[11px] leading-[16px] text-caption">{PERM_MODES[m].desc}</span>
                          </span>
                          {permMode === m && <Icon name="check" className="mt-0.5 flex-none text-info [&>svg]:size-3.5" />}
                        </button>
                      ))}
                    </div>
                  )}
                </div>
              </div>
              {permMode === "custom" && (
                <div className="flex flex-col">
                  {PERM_TOOLS.map(([k, label]) => (
                    <div key={k} className="flex min-h-10 items-center gap-3 border-b-[0.5px] border-hairline py-1.5 last:border-b-0">
                      <div className="min-w-0 flex-1">
                        <div className="font-mono text-[12.5px] text-foreground">{k}</div>
                        <div className="text-[11px] text-caption">{label}</div>
                      </div>
                      <div className="flex flex-none rounded-[9px] border-[0.5px] border-hairline p-0.5">
                        {(["allow", "ask", "deny"] as const).map((p) => {
                          const on = (permFields[k] || "") === p;
                          return (
                            <button
                              key={p}
                              onClick={() => setPermFields((f) => ({ ...f, [k]: p }))}
                              className={cn(
                                "h-6 rounded-[6px] px-2.5 text-[11px] font-medium capitalize transition-colors duration-100",
                                on
                                  ? p === "allow"
                                    ? "bg-info/15 text-info"
                                    : p === "ask"
                                      ? "bg-warning/15 text-warning"
                                      : "bg-destructive/15 text-destructive"
                                  : "text-caption hover:text-foreground",
                              )}
                            >
                              {p}
                            </button>
                          );
                        })}
                      </div>
                    </div>
                  ))}
                </div>
              )}
              <div className="text-[11.5px] text-muted-foreground">
                {permMode === "custom"
                  ? permRules.length
                    ? `${permRules.length} wildcard rule(s) from config.toml still apply on top`
                    : "wildcard rules ([[permissions.rules]] in config.toml) apply on top of these"
                  : "reads and searches stay available in every mode · wildcard rules from config.toml apply on top"}
              </div>
              <div className="text-[11.5px] text-muted-foreground">part of the saved form — press save to apply</div>
            </div>
          )}

          {sec === "about" && (
            <div className="flex max-w-xl flex-col gap-4">
              <h3 className={secTitle}>about</h3>
              <div className="flex items-center gap-3">
                <img src="/logo.png" alt="Hi!Derola logo" width={44} height={44} draggable={false} className="size-11 flex-none rounded-[14px] border-[0.5px] border-hairline object-cover" />
                <div className="flex flex-col gap-0.5">
                  <span className="text-[14px] font-medium">Hi!Derola</span>
                  <button
                    title="click to copy the version"
                    onClick={() => {
                      void copyText("Hi!Derola " + VERSION).then((ok) => {
                        if (ok) useStore.getState().toast("version copied: " + VERSION);
                      });
                    }}
                    className="w-fit rounded-[6px] text-[12px] text-muted-foreground transition-colors duration-100 hover:bg-accent hover:text-foreground"
                  >
                    desktop assistant · v{VERSION}
                  </button>
                </div>
              </div>
              <Button variant="ghost" size="sm" className="w-fit" onClick={() => window.open("https://github.com/mel0k1/hi-derola", "_blank")}>
                <Icon name="crew" className="[&>svg]:size-3.5" /> github repository
              </Button>
              <div className="text-[11.5px] text-muted-foreground">settings live in config.toml in the platform config directory; the same file is shared with the cli and tui</div>
            </div>
          )}
        </div>

        <div className="col-span-2 flex items-center gap-2.5 border-t border-border px-5 py-3">
          <span className="flex-1 truncate text-left text-[11.5px] text-muted-foreground">{msg}</span>
          <Button variant="ghost" size="sm" onClick={() => useStore.setState({ settingsOpen: false })}>cancel</Button>
          <Button size="sm" onClick={save}>save</Button>
        </div>
      </DialogContent>

      {/* dsh RiskConfirmation: enabling full access requires an explicit acknowledge */}
      <AlertDialog
        open={riskOpen}
        onOpenChange={(o) => {
          if (!o) {
            setRiskOpen(false);
            setRiskAck(false);
          }
        }}
      >
        <AlertDialogContent className="max-w-[440px]">
          <AlertDialogHeader>
            <AlertDialogTitle className="text-[15px]">enable full access?</AlertDialogTitle>
            <AlertDialogDescription className="text-[13.5px] leading-[21px]">
              full access lets the agent skip confirmation steps and perform more actions directly, including sensitive operations, file changes, or external commands. only use it when you trust the subsequent tasks.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <label className="flex items-start gap-2.5 text-[13px] leading-[20px] text-muted-foreground">
            <Checkbox checked={riskAck} onCheckedChange={(c) => setRiskAck(c === true)} className="mt-0.5" />
            <span>I understand the risks and want to continue</span>
          </label>
          <AlertDialogFooter>
            <AlertDialogCancel onClick={() => { setRiskOpen(false); setRiskAck(false); }}>cancel</AlertDialogCancel>
            <Button
              disabled={!riskAck}
              onClick={() => {
                setRiskOpen(false);
                setRiskAck(false);
                setPermMode("full");
              }}
              className="min-w-[136px]"
            >
              enable full access
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Dialog>
  );
}
