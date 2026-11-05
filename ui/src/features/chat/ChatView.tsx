import { useEffect, useMemo, useRef, useState } from "react";
import { Icon } from "@/lib/icons";
import { Hint } from "@/lib/hint";
import { DiffLines, Md, mdClickHandler } from "@/lib/md";
import { maskUserText } from "@/lib/secrets";
import { openContextMenu, type MenuItem } from "@/lib/menu";
import { invoke } from "@/lib/tauri";
import { useStore, type ChatItem } from "@/store/app";
import { cn, copyText } from "@/lib/utils";
import { useT } from "@/lib/i18n";
import { InputBar } from "@/features/chat/InputBar";

// dsh tool row: per-family glyph, tool name as the title
const TOOL_ICONS: Record<string, string> = {
  bash: "terminal",
  read_file: "file",
  write_file: "pencil",
  edit: "pencil",
  apply_patch: "diff",
  webfetch: "globe",
  codesearch: "search",
  grep: "search",
  glob: "search",
  list: "list",
  todowrite: "list",
  todoread: "list",
  question: "crew",
  subagent: "crew",
  skill: "skill",
  code: "box",
  lsp: "search",
  task_kill: "x",
};

function toolIcon(name: string): string {
  if (name.startsWith("mcp__")) return "server";
  const base = name.replace(/ \(crew .*\)$/, "").trim();
  return TOOL_ICONS[base] || "box";
}

// dsh summarySuffix: mono duration that survives a narrow row
function fmtMs(ms?: number): string {
  if (typeof ms !== "number") return "";
  if (ms < 1000) return `${ms}ms`;
  const s = ms / 1000;
  if (s < 60) return `${s < 10 ? s.toFixed(1) : Math.round(s)}s`;
  const m = Math.floor(s / 60);
  return `${m}m ${Math.round(s % 60)}s`;
}

function ToolRow({ item }: { item: Extract<ChatItem, { kind: "tool" }> }) {
  const hasDiff = !!item.diff && item.diff.length > 0;
  const [open, setOpen] = useState(false);
  const adds = item.diff?.filter((r) => r.tag === 1).length ?? 0;
  const dels = item.diff?.filter((r) => r.tag === 2).length ?? 0;
  const failed = item.state === "error";
  const running = !!item.running;
  const dur = fmtMs(item.ms);
  return (
    <div className="msg-in my-1">
      <div
        data-state={failed ? "error" : running ? "running" : "ok"}
        onClick={() => hasDiff && setOpen(!open)}
        className={cn(
          "group/row -mx-2 flex h-7 items-center gap-2 rounded-[10px] px-2 transition-colors duration-100",
          hasDiff && "cursor-pointer hover:bg-accent"
        )}
      >
        <Icon
          name="chevron"
          className={cn(
            "flex-none text-caption transition-transform duration-100 [&>svg]:size-3.5",
            hasDiff ? (open ? "rotate-180 cursor-pointer" : "-rotate-90 cursor-pointer") : "-rotate-90 opacity-0"
          )}
        />
        <Icon name={toolIcon(item.name)} className="flex-none text-caption [&>svg]:size-3.5" />
        <span className="flex-none text-[13px] font-medium text-foreground">{item.name}</span>
        <span className="size-1 flex-none rounded-full bg-caption/70" />
        <span
          className={cn(
            "min-w-0 flex-1 truncate font-mono text-[11.5px] text-tertiary",
            running && "shimmer-text",
            failed && "text-destructive"
          )}
        >
          {item.detail}
        </span>
        {failed && item.exit != null && (
          <span className="flex flex-none items-center gap-1.5 rounded-full border-[0.5px] border-hairline bg-card px-2 py-px font-mono text-[10px] text-caption">
            <span className="size-1.5 rounded-full bg-destructive" />
            exit {item.exit}
          </span>
        )}
        {dur && !running && (
          <span className="flex-none font-mono text-[10px] text-caption">{dur}</span>
        )}
        {hasDiff && !failed && (
          <span className="flex-none font-mono text-[10.5px] opacity-0 transition-opacity duration-100 [@media(hover:hover)]:group-hover/row:opacity-100">
            <span className="text-success">+{adds}</span> <span className="text-destructive">−{dels}</span>
          </span>
        )}
      </div>
      {hasDiff && open && (
        <div className="animate-in fade-in slide-in-from-top-1 mt-1 mb-2 ml-3 overflow-x-auto rounded-[12px] border-[0.5px] border-hairline bg-code px-2.5 py-1.5 duration-150">
          <DiffLines rows={item.diff!} />
        </div>
      )}
    </div>
  );
}

// dsh quiet welcome: just the mark and the name — no tagline, no filler
function Welcome() {
  return (
    <div className="flex flex-1 flex-col items-center justify-center px-5 pb-10 text-center">
      <div className="msg-in" style={{ animationDelay: "0s" }}>
        <div className="logo-float">
          <img src="/logo.png" alt="Hi!Derola" width={68} height={68} draggable={false} className="logo-mark rounded-2xl border-[0.5px] border-hairline shadow-soft" />
        </div>
      </div>
      <div className="msg-in mt-4 text-[26px] leading-8 font-semibold tracking-tight" style={{ animationDelay: "0.06s" }}>
        Hi!Derola
      </div>
    </div>
  );
}

function ToolOut({ item }: { item: Extract<ChatItem, { kind: "toolout" }> }) {
  const [open, setOpen] = useState(false);
  const t = useT();
  return (
    <pre
      onClick={() => setOpen(!open)}
      title={open ? t("ch.collapse") : t("ch.expand")}
      className={cn(
        "msg-in my-1 ml-2 cursor-pointer overflow-y-auto overflow-x-hidden rounded-[12px] border-[0.5px] border-hairline bg-code px-3 py-1.5 font-mono text-[11px] leading-[19px] break-words whitespace-pre-wrap text-tertiary transition-colors duration-100 hover:text-muted-foreground",
        open ? "max-h-80" : "max-h-9"
      )}
    >
      {item.text}
    </pre>
  );
}

function ThinkBox({ item }: { item: Extract<ChatItem, { kind: "think" }> }) {
  const [open, setOpen] = useState(item.open);
  const t = useT();
  useEffect(() => setOpen(item.open), [item.open]);
  return (
    <div className="msg-in my-1">
      <button
        onClick={() => setOpen(!open)}
        className="-mx-2 flex h-7 items-center gap-2 rounded-[10px] px-2 text-left transition-colors duration-100 hover:bg-accent"
      >
        <Icon name="chevron" className={cn("flex-none text-caption transition-transform duration-100 [&>svg]:size-3.5", open ? "rotate-180" : "-rotate-90")} />
        <span className={cn("text-[13px]", item.streaming ? "shimmer-text font-medium" : "text-tertiary")}>{t("ch.thinking")}</span>
        {item.streaming && <span className="pulse-dot size-1.5 flex-none rounded-full bg-info" />}
      </button>
      {open && (
        <div className="animate-in fade-in slide-in-from-top-1 mb-2 ml-4 max-h-64 overflow-y-auto pr-2 text-[13px] leading-[21px] break-words whitespace-pre-wrap text-tertiary duration-150">
          {item.text}
        </div>
      )}
    </div>
  );
}

function BgTask({ item }: { item: Extract<ChatItem, { kind: "bg" }> }) {
  const preRef = useRef<HTMLPreElement>(null);
  const t = useT();
  useEffect(() => {
    if (preRef.current) preRef.current.scrollTop = preRef.current.scrollHeight;
  }, [item.output]);
  return (
    <div className="msg-in my-2">
      <div className="-mx-2 flex h-7 items-center gap-2 px-2">
        <span className={cn("size-1.5 flex-none rounded-full", item.ended ? "bg-caption/70" : "pulse-dot bg-info")} />
        <span className={cn("font-mono text-[11.5px] font-semibold tracking-wide", item.ended ? "text-caption" : "text-info")}>{item.bgId} {t("ch.output")}</span>
        {!item.ended && (
          <button
            className="h-5 rounded-[8px] border-[0.5px] border-destructive/40 px-2 font-mono text-[10px] font-semibold text-destructive transition-colors duration-100 hover:bg-destructive/10"
            onClick={() => invoke("task_kill", { id: item.bgId }).catch(() => {})}
          >
            {t("ch.kill")}
          </button>
        )}
      </div>
      <pre
        ref={preRef}
        className="mt-0.5 max-h-44 overflow-y-auto rounded-[12px] border-[0.5px] border-hairline bg-code px-3 py-2 font-mono text-[11px] leading-[19px] break-words whitespace-pre-wrap text-tertiary"
      >
        {item.output}
      </pre>
    </div>
  );
}

// dsh MessageIconActions: hover-revealed chrome under each message,
// reserved space (no layout shift), kept visible while focused
function MsgActions({ side, children }: { side: "left" | "right"; children: React.ReactNode }) {
  return (
    <div
      className={cn(
        "absolute -bottom-5 flex items-center gap-0.5 opacity-0 transition-opacity duration-100 group-focus-within/msg:opacity-100 [@media(hover:hover)]:group-hover/msg:opacity-100",
        side === "right" ? "right-1" : "left-1",
      )}
    >
      {children}
    </div>
  );
}

function CopyBtn({ getText, side }: { getText: () => string; side: "left" | "right" }) {
  const [copied, setCopied] = useState(false);
  const t = useT();
  return (
    <Hint label={copied ? t("a.copied") : t("a.copy")} side={side}>
      <button
        aria-label={t("ch.copy_msg")}
        onClick={() => {
          void copyText(getText()).then((ok) => {
            if (!ok) return;
            setCopied(true);
            setTimeout(() => setCopied(false), 1400);
          });
        }}
        className="rounded-[7px] p-1 text-caption transition-colors duration-100 hover:bg-accent hover:text-foreground"
      >
        <Icon name={copied ? "check" : "copy"} className={cn("[&>svg]:size-3.5", copied && "text-info")} />
      </button>
    </Hint>
  );
}

// dsh url/message menu: copy selection when present, else the whole message;
// user bubbles also carry the chat-fork entry
function msgMenuTr(e: React.MouseEvent, text: string, fork: (() => void) | undefined, tr: (k: string) => string) {
  const sel = window.getSelection()?.toString() || "";
  const items: MenuItem[] = [];
  if (sel.trim()) {
    items.push({
      id: "sel",
      label: tr("ch.copy_sel"),
      icon: "copy",
      onSelect: () => {
        void copyText(sel).then((ok) => ok && useStore.getState().toast("a.copied"));
      },
    });
  }
  items.push({
    id: "msg",
    label: tr("ch.copy_msg"),
    icon: "copy",
    sep: !!sel.trim(),
    onSelect: () => {
      void copyText(text).then((ok) => ok && useStore.getState().toast("a.copied"));
    },
  });
  if (fork) {
    items.push({
      id: "fork",
      label: tr("ch.fork_here"),
      icon: "branch",
      sep: true,
      onSelect: fork,
    });
  }
  openContextMenu(e, items);
}

function UserItem({ item, userIndex, canFork }: { item: Extract<ChatItem, { kind: "user" }>; userIndex: number; canFork: boolean }) {
  const forkFrom = useStore((s) => s.forkFrom);
  const t = useT();
  const fork = () => void forkFrom(userIndex);
  return (
    <div className="group/msg relative my-3 flex flex-col items-end" onContextMenu={(e) => msgMenuTr(e, item.text, canFork ? fork : undefined, t)}>
      <div
        className="msg-in max-w-[85%] rounded-[20px] bg-bubble px-4 py-2.5 leading-[1.57] whitespace-pre-wrap break-words text-foreground"
        onClick={mdClickHandler}
        dangerouslySetInnerHTML={{ __html: maskUserText(item.text) }}
      />
      <MsgActions side="right">
        {canFork && (
          <Hint label={t("ch.fork_here")} side="left">
            <button
              aria-label={t("ch.fork_aria")}
              onClick={fork}
              className="rounded-[7px] p-1 text-caption transition-colors duration-100 hover:bg-accent hover:text-foreground"
            >
              <Icon name="branch" className="[&>svg]:size-3.5" />
            </button>
          </Hint>
        )}
        <CopyBtn getText={() => item.text} side="left" />
      </MsgActions>
    </div>
  );
}

function BotItem({ item, canRetry }: { item: Extract<ChatItem, { kind: "bot" }>; canRetry: boolean }) {
  const retryLast = useStore((s) => s.retryLast);
  const t = useT();
  return (
    <div className="group/msg relative my-3" onContextMenu={(e) => msgMenuTr(e, item.text, undefined, t)}>
      <div className="msg-in">
        <Md text={item.text} />
      </div>
      <MsgActions side="left">
        <CopyBtn getText={() => item.text} side="right" />
        {canRetry && (
          <Hint label={t("ch.regenerate")} side="right">
            <button
              aria-label={t("ch.regenerate")}
              onClick={() => void retryLast()}
              className="rounded-[7px] p-1 text-caption transition-colors duration-100 hover:bg-accent hover:text-foreground"
            >
              <Icon name="retry" className="[&>svg]:size-3.5" />
            </button>
          </Hint>
        )}
      </MsgActions>
    </div>
  );
}

function Item({ item, canRetry, userIndex, canFork }: { item: ChatItem; canRetry?: boolean; userIndex?: number; canFork?: boolean }) {
  switch (item.kind) {
    case "user":
      return <UserItem item={item} userIndex={userIndex ?? -1} canFork={!!canFork} />;
    case "bot":
      return <BotItem item={item} canRetry={!!canRetry} />;
    case "note":
      return (
        <div className="my-1.5 flex items-start gap-2 text-[12.5px] leading-[18px] break-words whitespace-pre-wrap text-tertiary">
          <span className="mt-[7px] size-1 flex-none rounded-full bg-caption/70" />
          {item.text}
        </div>
      );
    case "error":
      return (
        <div className="shake my-1.5 flex items-start gap-2 text-[12.5px] leading-[18px] break-words whitespace-pre-wrap text-destructive">
          <span className="mt-[7px] size-1 flex-none rounded-full bg-destructive/70" />
          {item.text}
        </div>
      );
    case "tool":
      return <ToolRow item={item} />;
    case "toolout":
      return <ToolOut item={item} />;
    case "think":
      return <ThinkBox item={item} />;
    case "bg":
      return <BgTask item={item} />;
  }
}

export function ChatView() {
  const transcript = useStore((s) => s.transcript);
  const streamText = useStore((s) => s.streamText);
  const autoscrollFlag = useStore((s) => s.autoscrollFlag);
  const scrollJump = useStore((s) => s.scrollJump);
  const phase = useStore((s) => s.phase);
  const t = useT();
  const scrollRef = useRef<HTMLDivElement>(null);
  const [away, setAway] = useState(false);
  // dsh use-scroll-follow: refs keep the streaming ticks honest — while the
  // reader is away (>320px from the tail) new content never yanks the view;
  // a smooth jump suppresses autoscroll until it lands
  const awayRef = useRef(false);
  const jumpingRef = useRef(false);

  const lastBotId = useMemo(() => {
    for (let i = transcript.length - 1; i >= 0; i--) {
      if (transcript[i].kind === "bot") return transcript[i].id;
    }
    return -1;
  }, [transcript]);

  const scrollBottom = () => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  };

  useEffect(() => {
    if (!awayRef.current) scrollBottom();
  }, [autoscrollFlag]);

  // hard jumps: session opened, new chat, user sent a message
  useEffect(() => {
    jumpingRef.current = false;
    awayRef.current = false;
    setAway(false);
    scrollBottom();
  }, [scrollJump]);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onScroll = () => {
      const d = el.scrollHeight - el.scrollTop - el.clientHeight;
      if (jumpingRef.current && d < 40) jumpingRef.current = false;
      awayRef.current = d > 320;
      setAway(d > 320);
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
  }, []);

  const empty = transcript.length === 0 && !streamText;

  return (
    <div ref={scrollRef} id="chat-scroll" className="flex h-full flex-col overflow-y-auto">
      {empty ? (
        <Welcome />
      ) : (
        <div className="mx-auto w-full max-w-[768px] flex-1 px-5 pt-4 pb-2" style={{ fontSize: "var(--chat-font, 14px)" }}>
          {/* userIndex = ordinal of the user message in the saved history — the fork cut point */}
          {(() => {
            let userOrdinal = 0;
            return transcript.map((it) => {
              const u = it.kind === "user" ? userOrdinal++ : -1;
              return <Item key={it.id} item={it} canRetry={it.kind === "bot" && it.id === lastBotId && phase === "idle"} userIndex={u} canFork={it.kind === "user" && phase === "idle"} />;
            });
          })()}
          {streamText !== null && (
            <div className="my-3">
              <Md text={streamText} />
            </div>
          )}
        </div>
      )}
      {/* sticky composer seat with dsh gradient mask */}
      <div
        className="sticky bottom-0 z-10 px-5 pt-9 pb-1.5"
        style={{ background: "linear-gradient(180deg, transparent 0px, var(--background) 40px)" }}
      >
        <div className="relative mx-auto w-full max-w-[800px]">
          <div className={cn("absolute -top-11 right-0 transition-all duration-150 ease-[cubic-bezier(0.4,0,0.2,1)]", away ? "translate-y-0 opacity-100" : "pointer-events-none translate-y-2 opacity-0")}>
            <Hint label={t("ch.jump_latest")} side="top">
              <button
                onClick={() => {
                  const el = scrollRef.current;
                  if (!el) return;
                  jumpingRef.current = true;
                  el.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
                }}
                className="flex size-8 items-center justify-center rounded-full bg-card text-muted-foreground shadow-panel transition-colors duration-150 hover:text-foreground active:scale-95"
              >
                <Icon name="down" className="[&>svg]:size-4" />
              </button>
            </Hint>
          </div>
          <InputBar />
        </div>
      </div>
    </div>
  );
}
