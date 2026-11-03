import { useEffect, useMemo, useRef, useState } from "react";
import { Icon } from "@/lib/icons";
import { Hint } from "@/lib/hint";
import { invoke } from "@/lib/tauri";
import { useStore } from "@/store/app";
import { cn } from "@/lib/utils";

// dsh ModelSelect: a quiet chip in the composer control row that opens an
// upward menu with search, keyboard navigation and a check mark on the active model
export function ModelMenu() {
  const model = useStore((s) => s.model);
  const models = useStore((s) => s.models);
  const setModels = useStore((s) => s.setModels);
  const runCmd = useStore((s) => s.runCmd);
  const waiting = useStore((s) => s.waiting);
  const cfg = useStore((s) => s.cfg);

  const [open, setOpen] = useState(false);
  const [q, setQ] = useState("");
  const [idx, setIdx] = useState(0);
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState("");
  const rootRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);

  const list = useMemo(() => {
    const all = [model, ...models.filter((m) => m !== model)].filter(Boolean);
    const t = q.trim().toLowerCase();
    return t ? all.filter((m) => m.toLowerCase().includes(t)) : all;
  }, [model, models, q]);

  useEffect(() => {
    if (!open) return;
    setIdx(0);
    const t = setTimeout(() => searchRef.current?.focus(), 0);
    const onDown = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    return () => {
      clearTimeout(t);
      document.removeEventListener("mousedown", onDown);
    };
  }, [open]);

  const load = async () => {
    setLoading(true);
    setErr("");
    try {
      const p = cfg?.provider;
      const ms = await invoke<string[]>("list_models", {
        kind: p?.type || "openai",
        base_url: p?.base_url?.trim() || null,
        api_key: p?.api_key?.trim() || null,
      });
      setModels(ms);
      if (!ms.length) setErr("provider returned no models");
    } catch (e) {
      setErr(String(e));
    }
    setLoading(false);
  };

  const pick = (m: string) => {
    setOpen(false);
    setQ("");
    if (m && m !== model) {
      runCmd("/model " + m);
      useStore.getState().toast("model → " + m);
    }
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (!open) return;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setIdx((i) => (list.length ? (i + 1) % list.length : 0));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setIdx((i) => (list.length ? (i - 1 + list.length) % list.length : 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (list[idx]) pick(list[idx]);
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      setOpen(false);
    }
  };

  if (!model && !models.length) return null;

  return (
    <div ref={rootRef} className="relative" onKeyDown={onKey}>
      <Hint label="switch model" side="top">
        <button
          aria-haspopup="listbox"
          aria-expanded={open}
          disabled={waiting}
          onClick={() => {
            setErr("");
            setOpen(!open);
          }}
          className="flex h-7 max-w-[300px] items-center gap-1.5 rounded-[8px] px-2 text-[12px] text-muted-foreground transition-colors duration-100 outline-none hover:bg-accent hover:text-foreground focus-visible:ring-1 focus-visible:ring-info/50 disabled:opacity-50"
        >
          <Icon name="spark" className="flex-none text-info [&>svg]:size-3.5" />
          <span className="truncate font-mono">{model || "model"}</span>
          <Icon name="chevron" className={cn("flex-none transition-transform duration-150 [&>svg]:size-3", open && "rotate-180")} />
        </button>
      </Hint>
      {open && (
        <div
          role="listbox"
          className="absolute right-0 bottom-full z-50 mb-2 w-[min(320px,84vw)] animate-in fade-in slide-in-from-bottom-1 rounded-[14px] border-[0.5px] border-hairline bg-glass p-1 shadow-panel backdrop-blur-[40px] backdrop-saturate-150 duration-150"
        >
          {models.length > 6 && (
            <div className="relative px-0.5 pb-1">
              <Icon name="search" className="absolute top-1/2 left-3 -translate-y-1/2 text-caption [&>svg]:size-3.5" />
              <input
                ref={searchRef}
                value={q}
                onChange={(e) => {
                  setQ(e.target.value);
                  setIdx(0);
                }}
                placeholder="search models"
                spellCheck={false}
                className="h-8 w-full rounded-[10px] bg-secondary pr-2.5 pl-8 font-mono text-[12px] outline-none placeholder:text-caption focus-visible:ring-1 focus-visible:ring-info/40"
              />
            </div>
          )}
          <div className="max-h-72 overflow-y-auto p-0.5">
            {list.map((m, i) => (
              <button
                key={m}
                role="option"
                aria-selected={m === model}
                onMouseDown={(e) => {
                  e.preventDefault();
                  pick(m);
                }}
                onMouseEnter={() => setIdx(i)}
                className={cn(
                  "flex w-full items-center gap-2 rounded-[10px] px-2.5 py-1.5 text-left font-mono text-[12.5px] transition-colors duration-100",
                  i === idx ? "bg-accent text-foreground" : "text-foreground/90"
                )}
              >
                <span className="min-w-0 flex-1 truncate">{m}</span>
                {m === model && <Icon name="check" className="flex-none text-info [&>svg]:size-3.5" />}
              </button>
            ))}
            {!list.length && <div className="px-2.5 py-2 text-[12px] text-caption">no match</div>}
          </div>
          <div className="mt-0.5 border-t-[0.5px] border-hairline px-0.5 pt-1 pb-0.5">
            {err && <div className="line-clamp-2 px-2 pt-0.5 pb-1 text-[11px] break-all text-destructive">{err}</div>}
            <button
              onMouseDown={(e) => {
                e.preventDefault();
                load();
              }}
              disabled={loading}
              className="flex w-full items-center gap-2 rounded-[10px] px-2.5 py-1.5 text-[12px] text-muted-foreground transition-colors duration-100 hover:bg-accent hover:text-foreground"
            >
              <Icon name="download" className={cn("flex-none [&>svg]:size-3.5", loading && "animate-bounce")} />
              {loading ? "loading…" : models.length ? "refresh model list" : "load models from provider"}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
