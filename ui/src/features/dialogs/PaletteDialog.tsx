import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Icon } from "@/lib/icons";
import { fmtRel } from "@/lib/utils";
import { useStore } from "@/store/app";
import { cn } from "@/lib/utils";
import { useEffect, useRef } from "react";

interface PaletteItem {
  icon: string;
  label: string;
  hint: string;
  run: () => void;
}

const CMDS: [string, string, string][] = [
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

export function PaletteDialog() {
  const palette = useStore((s) => s.palette);
  const sessions = useStore((s) => s.sessions);
  const models = useStore((s) => s.models);
  const model = useStore((s) => s.model);
  const openSession = useStore((s) => s.openSession);
  const runCmd = useStore((s) => s.runCmd);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const items: PaletteItem[] = [];
  for (const [cmd, label, hint] of CMDS) {
    items.push({ icon: "command", label, hint, run: () => runCmd(cmd) });
  }
  items.push({ icon: "sun", label: "toggle theme", hint: "dark / light", run: () => useStore.getState().toggleTheme() });
  items.push({ icon: "box", label: "open sandbox", hint: "local VMs, isolated agent workspace", run: () => useStore.getState().setView("sandbox") });
  items.push({ icon: "sliders", label: "open settings", hint: "provider, hotkeys, mcp, permissions", run: () => useStore.setState({ settingsOpen: true }) });
  items.push({ icon: "command", label: "keyboard shortcuts", hint: "quick reference", run: () => useStore.setState({ hotkeysOpen: true }) });
  items.push({ icon: "diff", label: "review session changes", hint: "changed files and diffs", run: () => useStore.getState().openReview() });
  items.push({ icon: "menu", label: "toggle sidebar", hint: "history panel", run: () => useStore.getState().toggleSidebar() });
  for (const s of sessions.slice(0, 15)) {
    items.push({
      icon: "spark",
      label: s.title || "new chat",
      hint: `${s.count} msgs · ${fmtRel(s.updated)}`,
      run: () => openSession(s.id),
    });
  }
  for (const m of models) {
    if (m === model) continue;
    items.push({ icon: "file", label: "model: " + m, hint: "switch model", run: () => runCmd("/model " + m) });
  }

  const query = palette.query.trim().toLowerCase();
  let filtered: PaletteItem[];
  if (!query) {
    filtered = items.slice(0, 30);
  } else {
    const scored: [number, PaletteItem][] = [];
    for (const it of items) {
      const l = it.label.toLowerCase();
      const idx = l.indexOf(query);
      if (idx === 0) scored.push([0, it]);
      else if (idx > 0) scored.push([1, it]);
      else if (it.hint && it.hint.toLowerCase().includes(query)) scored.push([2, it]);
    }
    scored.sort((a, b) => a[0] - b[0]);
    filtered = scored.slice(0, 30).map(([, it]) => it);
  }
  const idx = Math.min(palette.idx, Math.max(0, filtered.length - 1));

  const run = () => {
    const it = filtered[idx];
    if (!it) return;
    useStore.setState({ palette: { open: false, query: "", idx: 0 } });
    it.run();
  };

  useEffect(() => {
    if (palette.open) {
      inputRef.current?.focus();
    }
  }, [palette.open]);

  useEffect(() => {
    const act = listRef.current?.children[idx] as HTMLElement | undefined;
    if (act) act.scrollIntoView({ block: "nearest" });
  }, [idx, filtered.length]);

  return (
    <Dialog open={palette.open} onOpenChange={(o) => !o && useStore.setState({ palette: { open: false, query: "", idx: 0 } })}>
      <DialogContent className="translate-y-[-18vh] flex-col gap-0 overflow-hidden p-0" showCloseButton={false}>
        <input
          ref={inputRef}
          value={palette.query}
          placeholder="type a command, session or model..."
          spellCheck={false}
          className="w-full border-b border-border bg-transparent px-4.5 py-3.5 text-[14px] outline-none placeholder:text-muted-foreground"
          onChange={(e) => useStore.setState((s) => ({ palette: { ...s.palette, query: e.target.value, idx: 0 } }))}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") {
              e.preventDefault();
              useStore.setState((s) => ({ palette: { ...s.palette, idx: Math.min(s.palette.idx + 1, filtered.length - 1) } }));
            } else if (e.key === "ArrowUp") {
              e.preventDefault();
              useStore.setState((s) => ({ palette: { ...s.palette, idx: Math.max(s.palette.idx - 1, 0) } }));
            } else if (e.key === "Enter") {
              e.preventDefault();
              run();
            }
          }}
        />
        <div ref={listRef} className="max-h-[46vh] overflow-y-auto p-1.5">
          {!filtered.length ? (
            <div className="p-3 text-[12.5px] text-muted-foreground">nothing matches</div>
          ) : (
            filtered.map((it, i) => (
              <div
                key={it.label + i}
                onMouseDown={(e) => {
                  e.preventDefault();
                  useStore.setState({ palette: { ...palette, idx: i } });
                  setTimeout(() => {
                    useStore.setState({ palette: { open: false, query: "", idx: 0 } });
                    it.run();
                  }, 0);
                }}
                className={cn(
                  "flex cursor-pointer items-center gap-2.5 rounded-md px-3 py-2 text-[13px]",
                  i === idx ? "bg-accent" : ""
                )}
              >
                <Icon name={it.icon} className={cn("flex-none", i === idx ? "text-primary" : "text-muted-foreground")} />
                <span className="min-w-0 flex-1 truncate">{it.label}</span>
                {it.hint && <span className="max-w-[45%] flex-none truncate font-mono text-[10.5px] text-muted-foreground">{it.hint}</span>}
              </div>
            ))
          )}
        </div>
        <div className="border-t border-border px-4 py-2 text-[11px] text-muted-foreground">up/down navigate · enter run · esc close</div>
      </DialogContent>
    </Dialog>
  );
}
