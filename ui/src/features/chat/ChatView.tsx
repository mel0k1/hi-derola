import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Icon } from "@/lib/icons";
import { DiffLines, Md } from "@/lib/md";
import { invoke } from "@/lib/tauri";
import { useStore, type ChatItem } from "@/store/app";
import { cn } from "@/lib/utils";

const SUGGESTIONS = [
  { label: "explain this codebase", prompt: "explain this codebase: what it does and how it is structured" },
  { label: "find and fix a bug", prompt: "find a bug in this project and fix it" },
  { label: "write tests for the core", prompt: "write tests for the core module" },
  { label: "show commands", prompt: "/help" },
];

function Welcome() {
  const doSend = useStore((s) => s.doSend);
  return (
    <div className="flex flex-col items-center pt-[14vh] text-center">
      <div className="msg-in" style={{ animationDelay: "0s" }}>
        <div className="logo-float">
          <img
            src="/logo.png"
            alt="Hi!Derola"
            width={72}
            height={72}
            draggable={false}
            className="logo-mark rounded-2xl border border-border"
          />
        </div>
      </div>
      <div className="msg-in mt-3 text-[26px] font-bold tracking-tight" style={{ animationDelay: "0.08s" }}>
        Hi!<span className="text-primary">Derola</span>
      </div>
      <div className="msg-in text-[13.5px] text-muted-foreground" style={{ animationDelay: "0.16s" }}>
        a fast coding assistant in pure rust
      </div>
      <div className="mt-6 flex flex-wrap items-center justify-center gap-2">
        {SUGGESTIONS.map((s, i) => (
          <Button
            key={s.label}
            variant="outline"
            size="sm"
            className="msg-in h-8 border-input bg-transparent font-normal text-muted-foreground shadow-none hover:border-primary/50 hover:text-foreground dark:bg-transparent"
            style={{ animationDelay: `${0.24 + i * 0.06}s` }}
            onClick={() => doSend(s.prompt)}
          >
            {s.label}
          </Button>
        ))}
      </div>
      <div className="msg-in mt-7 flex flex-wrap items-center justify-center gap-1.5 text-[12px] text-muted-foreground" style={{ animationDelay: "0.5s" }}>
        ask anything · attach files with <Icon name="paperclip" className="text-primary" /> · /help for commands
      </div>
    </div>
  );
}

function ToolRow({ item }: { item: Extract<ChatItem, { kind: "tool" }> }) {
  return (
    <div className="msg-in my-2.5 overflow-hidden rounded-lg border border-border bg-card/60">
      <div className="flex items-baseline gap-2 px-3 py-1.5 font-mono text-[11.5px] text-muted-foreground">
        <span className="text-primary">▸</span>
        <span className="font-semibold text-foreground">{item.name}</span>
        <span className="truncate">{item.detail}</span>
      </div>
      {item.diff && item.diff.length > 0 && (
        <div className="px-3 pb-2">
          <DiffLines rows={item.diff} />
        </div>
      )}
    </div>
  );
}

function ToolOut({ item }: { item: Extract<ChatItem, { kind: "toolout" }> }) {
  const [open, setOpen] = useState(false);
  return (
    <pre
      onClick={() => setOpen(!open)}
      className={cn(
        "msg-in my-1 ml-3.5 cursor-pointer overflow-y-auto border-l-2 border-border py-0.5 pl-2.5 font-mono text-[11px] leading-relaxed text-muted-foreground transition-[max-height,border-color,color] duration-200 ease-out hover:border-primary/50 hover:text-foreground",
        open ? "max-h-120" : "max-h-30"
      )}
    >
      {item.text}
    </pre>
  );
}

function ThinkBox({ item }: { item: Extract<ChatItem, { kind: "think" }> }) {
  const [open, setOpen] = useState(item.open);
  useEffect(() => setOpen(item.open), [item.open]);
  return (
    <div className="msg-in my-2.5 overflow-hidden rounded-lg border border-border border-l-2 border-l-primary/50 bg-card/40">
      <button
        onClick={() => setOpen(!open)}
        className="flex w-full items-center gap-1.5 px-3 py-1.5 font-mono text-[11.5px] font-semibold tracking-wide text-muted-foreground select-none hover:text-foreground"
      >
        thinking
        {item.streaming && <span className="ml-1 inline-block size-1.5 animate-pulse rounded-full bg-primary" />}
        <Icon name="chevron" className={cn("ml-auto transition-transform", open && "rotate-180")} />
      </button>
      {open && (
        <div className="max-h-64 animate-in fade-in slide-in-from-top-1 overflow-y-auto px-3.5 pb-2.5 text-[12.5px] break-words whitespace-pre-wrap text-muted-foreground duration-200">
          {item.text}
        </div>
      )}
    </div>
  );
}

function BgTask({ item }: { item: Extract<ChatItem, { kind: "bg" }> }) {
  const preRef = useRef<HTMLPreElement>(null);
  useEffect(() => {
    if (preRef.current) preRef.current.scrollTop = preRef.current.scrollHeight;
  }, [item.output]);
  return (
    <div className={cn("msg-in my-2.5 border-l-2 py-0.5 pl-2.5", item.ended ? "border-border" : "border-primary/60")}>
      <div className="flex items-center justify-between gap-2 font-mono text-[11.5px] font-semibold tracking-wide">
        <span className={item.ended ? "text-muted-foreground" : "text-primary"}>{item.bgId} output</span>
        {!item.ended && (
          <button
            className="rounded-md border border-destructive/60 px-2 py-px font-mono text-[10px] font-semibold text-destructive/90 hover:bg-destructive hover:text-white"
            onClick={() => invoke("task_kill", { id: item.bgId }).catch(() => {})}
          >
            kill
          </button>
        )}
      </div>
      <pre ref={preRef} className="mt-1 max-h-44 overflow-y-auto font-mono text-[11px] leading-relaxed break-words whitespace-pre-wrap text-muted-foreground">
        {item.output}
      </pre>
    </div>
  );
}

function Item({ item }: { item: ChatItem }) {
  switch (item.kind) {
    case "user":
      return (
        <div className="msg-in my-4 flex flex-col items-end">
          <div className="mb-0.5 font-mono text-[11px] tracking-wide text-muted-foreground lowercase">you</div>
          <div className="max-w-[85%] rounded-xl rounded-br-sm border border-border bg-chat-you px-3.5 py-2 whitespace-pre-wrap break-words text-foreground">
            {item.text}
          </div>
        </div>
      );
    case "bot":
      return (
        <div className="msg-in my-4">
          <div className="mb-0.5 font-mono text-[11px] tracking-wide text-muted-foreground lowercase">bot</div>
          <Md text={item.text} />
        </div>
      );
    case "note":
      return <div className="my-1.5 text-[12.5px] break-words whitespace-pre-wrap text-muted-foreground">· {item.text}</div>;
    case "error":
      return <div className="shake my-1.5 text-[12.5px] break-words whitespace-pre-wrap text-destructive">· {item.text}</div>;
    case "tool":
      return <ToolRow item={item} />;
    case "toolout":
      return <ToolOut item={item} />;
    case "todo":
      return (
        <div className="msg-in my-2.5 border-l-2 border-primary/50 pl-3">
          <div className="mb-0.5 font-mono text-[11px] tracking-wide text-primary lowercase">todos</div>
          <pre className="font-mono text-[12px] leading-relaxed break-words whitespace-pre-wrap">{item.text}</pre>
        </div>
      );
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
  const scrollRef = useRef<HTMLDivElement>(null);
  const [away, setAway] = useState(false);

  const scrollBottom = () => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  };

  useEffect(() => {
    scrollBottom();
  }, [autoscrollFlag]);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onScroll = () => {
      setAway(el.scrollHeight - el.scrollTop - el.clientHeight > 320);
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
  }, []);

  return (
    <div ref={scrollRef} className="h-full overflow-y-auto" id="chat-scroll">
      {transcript.length === 0 && !streamText ? (
        <Welcome />
      ) : (
        <div className="mx-auto max-w-3xl px-5 pt-4 pb-6">
          {transcript.map((it) => (
            <Item key={it.id} item={it} />
          ))}
          {streamText !== null && (
            <div className="my-4">
              <div className="mb-0.5 font-mono text-[11px] tracking-wide text-muted-foreground lowercase">bot</div>
              <Md text={streamText} />
            </div>
          )}
        </div>
      )}
      <div className="pointer-events-none sticky bottom-2 h-0">
        <div className="pointer-events-auto mx-auto flex max-w-3xl justify-end px-5">
          <Button
            variant="outline"
            size="icon"
            className={cn(
              "size-8 rounded-full border-border bg-card shadow-md transition-all",
              away ? "translate-y-0 opacity-100" : "pointer-events-none translate-y-2 opacity-0"
            )}
            title="jump to latest"
            onClick={() => {
              const el = scrollRef.current;
              if (el) el.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
            }}
          >
            <Icon name="down" />
          </Button>
        </div>
      </div>
    </div>
  );
}
