import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Icon } from "@/lib/icons";
import { invoke } from "@/lib/tauri";
import { useStore, type MentionItem } from "@/store/app";
import { chipName, cn } from "@/lib/utils";

let mfilesCache: string[] | null = null;
let mentionSeq = 0;
let magentsCache: MentionItem[] | null = null;

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
    <div className="mx-auto w-full max-w-3xl px-5">
      <div className="mb-1.5 flex flex-wrap gap-1.5">
        {attachments.map((p, i) => (
          <span
            key={i}
            title={p}
            className="msg-in flex max-w-80 items-center gap-1.5 rounded-lg border border-border bg-secondary/70 py-0.5 pr-1 pl-2.5 text-[12px]"
          >
            <Icon name="file" className="text-primary" />
            <span className="truncate">{chipName(p)}</span>
            <button
              className="rounded p-0.5 text-muted-foreground hover:text-foreground"
              title="remove"
              onClick={() => detachChip(i)}
            >
              <Icon name="x" className="[&>svg]:size-3" />
            </button>
          </span>
        ))}
      </div>
    </div>
  );
}

export function InputBar() {
  const doSend = useStore((s) => s.doSend);
  const openFiles = useStore((s) => s.openFiles);
  const waiting = useStore((s) => s.waiting);
  const view = useStore((s) => s.view);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const [mention, setMention] = useState<MentionState | null>(null);
  const [hasText, setHasText] = useState(false);

  useEffect(() => {
    if (view === "chat") inputRef.current?.focus();
  }, [view]);

  const autosize = () => {
    const el = inputRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = Math.min(el.scrollHeight, 220) + "px";
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
    if (mention) {
      if (e.key === "ArrowDown" || e.key === "ArrowUp" || e.key === "Tab" || e.key === "Enter" || e.key === "Escape") {
        e.preventDefault();
        if (e.key === "ArrowDown") setMention({ ...mention, idx: (mention.idx + 1) % mention.items.length });
        else if (e.key === "ArrowUp") setMention({ ...mention, idx: (mention.idx - 1 + mention.items.length) % mention.items.length });
        else if (e.key === "Tab" || e.key === "Enter") completeMention(mention, mention.idx);
        else hideMention();
        return;
      }
    }
    const st = useStore.getState();
    if (st.waiting && st.keys.stop === "escape" && e.key === "Escape") {
      invoke("stop").catch(() => {});
      return;
    }
    if (st.keys.send === "enter" && e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      doSend(el.value);
      el.value = "";
      setHasText(false);
      autosize();
      hideMention();
      return;
    }
    if (st.keys.newline === "shift+enter" && e.key === "Enter" && e.shiftKey) {
      e.preventDefault();
      const s = el.selectionStart, epos = el.selectionEnd;
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
    autosize();
    hideMention();
    el.focus();
  };

  return (
    <div id="input-bar" className={cn("flex-none px-5 pt-1 pb-2", view !== "chat" && "hidden")}>
      <Chips />
      <div className="relative mx-auto w-full max-w-3xl">
        {mention && (
          <div className="absolute bottom-full left-0 z-40 mb-2.5 max-h-64 min-w-72 max-w-[26rem] animate-in fade-in slide-in-from-bottom-1 overflow-y-auto rounded-xl border border-border bg-popover p-1 shadow-lg duration-150">
            {mention.items.map((p, i) => (
              <div
                key={p.v + i}
                onMouseDown={(e) => {
                  e.preventDefault();
                  completeMention(mention, i);
                }}
                className={cn(
                  "flex cursor-pointer items-center gap-2 overflow-hidden rounded-md px-2.5 py-1.5 font-mono text-[12px] whitespace-nowrap",
                  i === mention.idx ? "bg-accent text-accent-foreground" : "text-foreground"
                )}
              >
                <Icon name={p.agent ? "spark" : "file"} className="flex-none text-muted-foreground" />
                <span className="truncate">{p.label}</span>
              </div>
            ))}
          </div>
        )}
        <div
          className={cn(
            "flex items-end gap-1.5 rounded-2xl border border-input bg-card px-2 py-1.5 transition-colors focus-within:border-ring focus-within:ring-3 focus-within:ring-ring/30"
          )}
        >
          <Button
            variant="ghost"
            size="icon-sm"
            className="mb-0.5 flex-none text-muted-foreground"
            title="attach file or folder"
            onClick={openFiles}
          >
            <Icon name="paperclip" />
          </Button>
          <textarea
            id="chat-input"
            ref={inputRef}
            rows={1}
            spellCheck={false}
            placeholder="message · /help for commands"
            className="max-h-56 min-h-9 flex-1 resize-none bg-transparent py-1.5 leading-relaxed outline-none placeholder:text-muted-foreground"
            onInput={() => {
              autosize();
              showMention();
              setHasText(!!inputRef.current?.value.trim());
            }}
            onClick={hideMention}
            onKeyDown={onKeyDown}
          />
          <Button
            size="icon-sm"
            className={cn(
              "mb-0.5 flex-none rounded-xl transition-transform active:scale-95",
              hasText && !waiting && "shadow-[0_0_0_3px_var(--ring)]"
            )}
            disabled={!hasText || waiting}
            title="send"
            onClick={send}
          >
            <Icon name="send" />
          </Button>
        </div>
      </div>
    </div>
  );
}
