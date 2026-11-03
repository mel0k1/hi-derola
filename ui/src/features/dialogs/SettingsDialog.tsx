import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Icon } from "@/lib/icons";
import { useStore } from "@/store/app";
import { invoke } from "@/lib/tauri";
import { parseKVLines } from "@/lib/utils";
import { comboFromEvent, comboToText } from "@/lib/keys";
import { KEY_ACTIONS } from "@/store/keysStore";
import type { Cfg, McpPrompt, McpResource, McpTemplate } from "@/lib/types";
import { useEffect, useState } from "react";

function HotkeyRow({ action, label, value, onChange }: { action: string; label: string; value: string; onChange: (v: string) => void }) {
  return (
    <div className="flex items-center gap-2">
      <span className="flex-1 text-[12.5px] text-muted-foreground">{label}</span>
      <input
        readOnly
        data-action={action}
        value={value || "none"}
        onKeyDown={(e) => {
          e.preventDefault();
          e.stopPropagation();
          if (e.key === "Escape") {
            onChange("none");
            return;
          }
          const c = comboFromEvent(e.nativeEvent);
          if (c) onChange(comboToText(c));
        }}
        className="w-36 rounded-md border border-input bg-background px-2 py-1.5 text-center font-mono text-[11.5px] text-primary outline-none focus:border-ring"
      />
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

export function SettingsDialog() {
  const open = useStore((s) => s.settingsOpen);
  const cfg = useStore((s) => s.cfg);
  const keys = useStore((s) => s.keys);
  const setModels = useStore((s) => s.setModels);
  const saveSettings = useStore((s) => s.saveSettings);
  const note = useStore((s) => s.note);
  const theme = useStore((s) => s.theme);

  const [form, setForm] = useState<Record<string, unknown>>({});
  const [keyOverrides, setKeyOverrides] = useState<Record<string, string>>({});
  const [msg, setMsg] = useState("");
  const [showKey, setShowKey] = useState(false);

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
      setMsg(p.active ? `profile "${p.active}" is active; its fields override the values below (switch with /profile in the chat)` : "");
    }
  }, [open, cfg]);

  if (!cfg) return null;
  const v = (k: string) => String(form[k] ?? "");
  const set = (k: string, val: unknown) => setForm((f) => ({ ...f, [k]: val }));

  const loadModels = async () => {
    setMsg("loading...");
    try {
      const models = await invoke<string[]>("list_models", {
        kind: v("type"),
        base_url: v("base").trim() || null,
        api_key: v("key").trim() || null,
      });
      setModels(models);
      setMsg(models.length + " models");
    } catch (e) {
      setMsg(String(e));
    }
  };

  const save = async () => {
    if (!v("model").trim()) {
      setMsg("model is required");
      return;
    }
    const num = (k: string) => (v(k).trim() === "" ? null : Number(v(k)));
    const keysOut: Record<string, string> = {};
    for (const [action] of KEY_ACTIONS) {
      keysOut[action] = keyOverrides[action] !== undefined ? keyOverrides[action] || "none" : keys[action] || "none";
    }
    const out: Cfg = {
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
      permissions: cfg.permissions,
      lsp: cfg.lsp,
      formatters: cfg.formatters,
    };
    const ok = await saveSettings(out);
    if (!ok) {
      setMsg("save failed");
      return;
    }
    useStore.setState({ settingsOpen: false });
    note("settings saved");
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
      permissions: cfg.permissions,
      lsp: cfg.lsp,
      formatters: cfg.formatters,
    };
  };

  const perm = cfg.permissions || {};
  const permLines: string[] = [];
  for (const k of ["edit", "write_file", "bash", "sandbox", "mcp"]) {
    if (perm[k]) permLines.push(k + " = " + String(perm[k]));
  }
  for (const r of perm.rules || []) {
    permLines.push(`${r.tool} ${r.pattern || "*"} = ${r.permission}`);
  }

  return (
    <Dialog open={open} onOpenChange={(o) => !o && useStore.setState({ settingsOpen: false })}>
      <DialogContent className="flex h-[88vh] w-[min(640px,94vw)] flex-col gap-0 p-0" showCloseButton={false}>
        <DialogHeader className="flex-row items-center justify-between border-b border-border px-4 py-3">
          <DialogTitle className="text-[14px]">settings</DialogTitle>
          <Button variant="ghost" size="icon-sm" className="text-muted-foreground" onClick={() => useStore.setState({ settingsOpen: false })}>
            <Icon name="x" />
          </Button>
        </DialogHeader>
        <div className="flex min-h-0 flex-1 flex-col gap-6 overflow-y-auto px-5 py-4">
          <section className="flex flex-col gap-2.5">
            <h3 className="font-mono text-[11.5px] font-semibold tracking-wider text-muted-foreground lowercase">provider</h3>
            <div className="grid grid-cols-2 gap-2.5">
              <div className="flex flex-col gap-1">
                <Label className="text-[12px] text-muted-foreground">type</Label>
                <select value={v("type")} onChange={(e) => set("type", e.target.value)} className="h-8.5 rounded-lg border border-input bg-background px-2 font-mono text-[12.5px] outline-none focus:border-ring">
                  <option value="openai">openai / compatible</option>
                  <option value="anthropic">anthropic</option>
                </select>
              </div>
              <div className="flex flex-col gap-1">
                <Label className="text-[12px] text-muted-foreground">base url</Label>
                <input value={v("base")} onChange={(e) => set("base", e.target.value)} placeholder="https://api.openai.com/v1" spellCheck={false} className="h-8.5 rounded-lg border border-input bg-background px-2.5 font-mono text-[12.5px] outline-none focus:border-ring" />
              </div>
            </div>
            <div className="flex flex-col gap-1">
              <Label className="text-[12px] text-muted-foreground">api key</Label>
              <div className="flex gap-2">
                <input type={showKey ? "text" : "password"} value={v("key")} onChange={(e) => set("key", e.target.value)} placeholder="sk-..." spellCheck={false} className="h-8.5 flex-1 rounded-lg border border-input bg-background px-2.5 font-mono text-[12.5px] outline-none focus:border-ring" />
                <Button variant="ghost" size="sm" onClick={() => setShowKey(!showKey)}>{showKey ? "hide" : "show"}</Button>
              </div>
            </div>
            <div className="flex flex-col gap-1">
              <Label className="text-[12px] text-muted-foreground">model</Label>
              <input value={v("model")} onChange={(e) => set("model", e.target.value)} list="model-list" spellCheck={false} className="h-8.5 rounded-lg border border-input bg-background px-2.5 font-mono text-[12.5px] outline-none focus:border-ring" />
              <datalist id="model-list">
                {useStore.getState().models.map((m) => (
                  <option key={m} value={m} />
                ))}
              </datalist>
            </div>
            <div className="flex items-center gap-2">
              <Button variant="ghost" size="sm" onClick={loadModels}>load models</Button>
              <span className="text-[11.5px] text-muted-foreground">{msg}</span>
            </div>
            <div className="grid grid-cols-3 gap-2.5">
              <div className="flex flex-col gap-1">
                <Label className="text-[12px] text-muted-foreground">max tokens</Label>
                <input type="number" min={128} step={128} value={v("maxTokens")} onChange={(e) => set("maxTokens", e.target.value)} className="h-8.5 rounded-lg border border-input bg-background px-2.5 font-mono text-[12.5px] outline-none focus:border-ring" />
              </div>
              <div className="flex flex-col gap-1">
                <Label className="text-[12px] text-muted-foreground">temperature</Label>
                <input type="number" step={0.1} min={0} max={2} value={v("temperature")} onChange={(e) => set("temperature", e.target.value)} className="h-8.5 rounded-lg border border-input bg-background px-2.5 font-mono text-[12.5px] outline-none focus:border-ring" />
              </div>
              <div className="flex flex-col gap-1">
                <Label className="text-[12px] text-muted-foreground">top p</Label>
                <input type="number" step={0.05} min={0} max={1} value={v("topP")} onChange={(e) => set("topP", e.target.value)} className="h-8.5 rounded-lg border border-input bg-background px-2.5 font-mono text-[12.5px] outline-none focus:border-ring" />
              </div>
            </div>
            <div className="flex items-center gap-2">
              <Switch id="s-stream" checked={!!form.stream} onCheckedChange={(c) => set("stream", c)} />
              <Label htmlFor="s-stream" className="text-[13px]">stream responses</Label>
            </div>
          </section>

          <section className="flex flex-col gap-2.5">
            <h3 className="font-mono text-[11.5px] font-semibold tracking-wider text-muted-foreground lowercase">hotkeys</h3>
            <div className="grid grid-cols-1 gap-1.5 sm:grid-cols-2">
              {KEY_ACTIONS.map(([action, label]) => (
                <HotkeyRow
                  key={action}
                  action={action}
                  label={label}
                  value={keyOverrides[action] !== undefined ? keyOverrides[action] || "none" : keys[action] || "none"}
                  onChange={(val) => setKeyOverrides((kv) => ({ ...kv, [action]: val }))}
                />
              ))}
            </div>
            <div className="text-[11.5px] text-muted-foreground">click a field and press the combination, escape sets none</div>
          </section>

          <McpSection mcp={cfg.mcp} onAdd={addMcp} onRemove={removeMcp} setMsg={setMsg} />

          <section className="flex flex-col gap-2.5">
            <h3 className="font-mono text-[11.5px] font-semibold tracking-wider text-muted-foreground lowercase">permissions</h3>
            <div className="text-[12px] whitespace-pre-wrap text-muted-foreground">
              {permLines.length ? permLines.join("\n") : "default: mutations (write/edit/bash/sandbox/mcp) ask, reads allowed"}
            </div>
            <div className="text-[11.5px] text-muted-foreground">edited in config.toml: allow | ask | deny per tool, [[permissions.rules]] adds wildcard patterns</div>
          </section>
        </div>
        <div className="flex items-center gap-2.5 border-t border-border px-5 py-3">
          <span className="flex-1 text-left text-[11.5px] text-muted-foreground">{msg}</span>
          <Button variant="ghost" size="sm" onClick={() => useStore.setState({ settingsOpen: false })}>cancel</Button>
          <Button size="sm" onClick={save}>save</Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
