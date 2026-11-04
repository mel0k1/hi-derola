import { Icon } from "@/lib/icons";
import { Hint, keyChips } from "@/lib/hint";
import { fmtCost, fmtRel, cn } from "@/lib/utils";
import { useStore } from "@/store/app";
import { invoke } from "@/lib/tauri";
import { useState, type ReactNode } from "react";

const TOOLS: { id: "sandbox" | "hosts" | "skills" | "crew"; icon: string; label: string; title: string }[] = [
  { id: "sandbox", icon: "box", label: "sandbox", title: "sandbox — local QEMU VMs for isolated agent work" },
  { id: "hosts", icon: "server", label: "remote hosts", title: "remote hosts — run the agent on your servers over ssh" },
  { id: "skills", icon: "skill", label: "skills", title: "skills — plugin folders with SKILL.md, toggles, git install" },
  { id: "crew", icon: "crew", label: "crew", title: "crew — several AI agents in one session" },
];

const PIN_KEY = "hiderola.pinned";

function loadPins(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(PIN_KEY) || "[]");
    return Array.isArray(v) ? v.filter((x) => typeof x === "string") : [];
  } catch {
    return [];
  }
}

function SessionRow({ id, title, count, updated, cost, active, pinned, attention, forked, onPin }: { id: string; title: string; count: number; updated: number; cost?: number; active: boolean; pinned: boolean; attention: boolean; forked?: boolean; onPin: (id: string) => void }) {
  const openSession = useStore((s) => s.openSession);
  const waiting = useStore((s) => s.waiting);
  const note = useStore((s) => s.note);
  const refreshSessions = useStore((s) => s.refreshSessions);
  const [confirming, setConfirming] = useState(false);

  return (
    <div
      role="button"
      tabIndex={0}
      title={`${count} msgs · ${fmtRel(updated)}${cost && cost > 0 ? ` · ${fmtCost(cost)}` : ""}`}
      onClick={() => openSession(id)}
      onKeyDown={(e) => e.key === "Enter" && openSession(id)}
      className={cn(
        "group flex h-8 cursor-pointer items-center gap-1 rounded-[12px] pr-1 pl-3 transition-colors duration-100 outline-none",
        "hover:bg-accent",
        active ? "bg-accent" : "bg-transparent"
      )}
    >
      {forked && <Icon name="branch" className="flex-none text-caption [&>svg]:size-3" />}
      <span className={cn("min-w-0 flex-1 truncate text-[13px] leading-5", active ? "font-medium text-foreground" : "text-foreground/90")}>
        {title || "new chat"}
      </span>
      {attention && <span className="pulse-dot size-1.5 flex-none rounded-full bg-warning" title="the agent is working here" />}
      <span className={cn("flex-none font-mono text-[10.5px] text-caption transition-opacity duration-100 group-hover:opacity-0", confirming && "opacity-0")}>
        {fmtRel(updated)}
      </span>
      <button
        title={pinned ? "unpin" : "pin"}
        className={cn(
          "size-6 flex-none items-center justify-center rounded-[8px] text-caption transition-colors duration-100 hover:bg-accent hover:text-foreground",
          pinned ? "flex text-tertiary" : "hidden group-hover:flex",
          confirming && "hidden"
        )}
        onClick={(e) => {
          e.stopPropagation();
          onPin(id);
        }}
      >
        <Icon name="pin" className={cn("[&>svg]:size-3.5", pinned && "text-info")} />
      </button>
      <button
        title="delete"
        className={cn(
          "hidden size-6 flex-none items-center justify-center rounded-[8px] text-caption transition-colors duration-100 group-hover:flex",
          confirming ? "text-destructive" : "hover:bg-accent hover:text-destructive"
        )}
        onClick={(e) => {
          e.stopPropagation();
          if (confirming) {
            if (waiting) {
              note("stop the current run before deleting this chat");
              return;
            }
            invoke("delete_session", { id })
              .catch((err) => note(String(err)))
              .finally(() => refreshSessions());
          } else {
            setConfirming(true);
            setTimeout(() => setConfirming(false), 2500);
          }
        }}
      >
        <Icon name={confirming ? "check" : "trash"} className="[&>svg]:size-3.5" />
      </button>
    </div>
  );
}

function SectionLabel({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={cn("px-3 pt-4 pb-1 text-[11px] font-medium tracking-[0.08em] text-caption uppercase select-none", className)}>{children}</div>;
}

export function Sidebar() {
  const collapsed = useStore((s) => s.sidebarHidden);
  const view = useStore((s) => s.view);
  const setView = useStore((s) => s.setView);
  const sessions = useStore((s) => s.sessions);
  const sid = useStore((s) => s.sid);
  const cwd = useStore((s) => s.cwd);
  const newChat = useStore((s) => s.newChat);
  const toggleSidebar = useStore((s) => s.toggleSidebar);
  const theme = useStore((s) => s.theme);
  const toggleTheme = useStore((s) => s.toggleTheme);
  const keys = useStore((s) => s.keys);
  const waiting = useStore((s) => s.waiting);
  const openSettings = () => useStore.setState({ settingsOpen: true });

  const [query, setQuery] = useState("");
  const [pins, setPins] = useState<string[]>(loadPins);
  const togglePin = (id: string) =>
    setPins((p) => {
      const next = p.includes(id) ? p.filter((x) => x !== id) : [...p, id];
      localStorage.setItem(PIN_KEY, JSON.stringify(next));
      return next;
    });
  const q = query.trim().toLowerCase();
  const filtered = q ? sessions.filter((s) => (s.title || "new chat").toLowerCase().includes(q)) : sessions;
  const ordered = [...filtered.filter((s) => pins.includes(s.id)), ...filtered.filter((s) => !pins.includes(s.id))];

  return (
    <aside
      className={cn(
        "relative z-10 flex-none overflow-visible bg-sidebar text-sidebar-foreground transition-[width] duration-300 ease-[cubic-bezier(0.4,0,0.2,1)]",
        collapsed ? "w-14" : "w-[280px]"
      )}
    >
      {/* expanded content */}
      <div
        aria-hidden={collapsed}
        className={cn(
          "absolute inset-y-0 left-0 flex w-[280px] flex-col transition-opacity duration-150 ease-[cubic-bezier(0.4,0,0.2,1)]",
          collapsed ? "pointer-events-none opacity-0" : "opacity-100"
        )}
      >
        {/* logo row */}
        <div className="flex h-[52px] flex-none items-center gap-2.5 pr-2 pl-5">
          <img src="/logo.png" alt="" draggable={false} className="h-6 w-6 flex-none rounded-[6px]" />
          <div className="text-[16px] font-semibold tracking-[0.02em] whitespace-nowrap">
            Hi!<span className="font-medium">Derola</span>
          </div>
        </div>

        {/* new chat bar */}
        <div className="flex-none px-3 pt-1 pb-1.5">
          <button
            onClick={newChat}
            className="flex h-9 w-full items-center gap-2 rounded-[12px] border-[0.5px] border-border bg-card px-3 text-[13.5px] font-medium text-foreground shadow-xs transition-colors duration-100 hover:bg-muted"
          >
            <Icon name="plus" className="text-tertiary" />
            new chat
          </button>
        </div>

        {/* tools */}
        <SectionLabel>tools</SectionLabel>
        <nav className="flex flex-col gap-0.5 px-3">
          {TOOLS.map((t) => (
            <button
              key={t.id}
              title={t.title}
              onClick={() => setView(t.id)}
              className={cn(
                "flex h-8 items-center gap-2.5 rounded-[12px] px-2.5 text-left text-[13px] leading-5 transition-colors duration-100 outline-none",
                view === t.id ? "bg-accent font-medium text-foreground" : "text-foreground/85 hover:bg-accent/60"
              )}
            >
              <Icon name={t.icon} className={view === t.id ? "text-foreground" : "text-tertiary"} />
              {t.label}
            </button>
          ))}
        </nav>

        {/* sessions */}
        <SectionLabel>sessions</SectionLabel>
        <div className="flex-none px-3 pb-1.5">
          <div className="relative">
            <Icon name="search" className="absolute top-1/2 left-2.5 -translate-y-1/2 text-caption [&>svg]:size-3.5" />
            <input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="search sessions"
              spellCheck={false}
              className="h-7.5 w-full rounded-[10px] bg-secondary py-1 pr-2.5 pl-8 text-[12px] outline-none placeholder:text-caption focus-visible:ring-1 focus-visible:ring-info/40"
            />
          </div>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto px-3 pb-2">
          {sessions.length === 0 ? (
            <div className="px-2.5 py-1 text-[12.5px] text-caption">no history yet</div>
          ) : ordered.length === 0 ? (
            <div className="px-2.5 py-1 text-[12.5px] text-caption">no match</div>
          ) : (
            <div className="flex flex-col gap-0.5">
              {ordered.map((s) => (
                <SessionRow
                  key={s.id}
                  id={s.id}
                  title={s.title}
                  count={s.count}
                  updated={s.updated}
                  cost={s.cost}
                  active={s.id === sid}
                  pinned={pins.includes(s.id)}
                  attention={s.id === sid && waiting}
                  forked={!!s.parent}
                  onPin={togglePin}
                />
              ))}
            </div>
          )}
        </div>

        {/* foot */}
        <div className="flex flex-none items-center gap-0.5 border-t-[0.5px] border-hairline px-3 py-2">
          <Hint label="toggle theme" keys={keyChips(keys.toggle_theme)} side="top">
            <button
              onClick={toggleTheme}
              className="flex size-7 items-center justify-center rounded-[8px] text-tertiary transition-colors duration-100 hover:bg-accent hover:text-foreground"
            >
              <Icon name={theme === "light" ? "moon" : "sun"} className="[&>svg]:size-4" />
            </button>
          </Hint>
          <Hint label="settings" keys={keyChips(keys.open_settings)} side="top">
            <button
              onClick={openSettings}
              className="flex size-7 items-center justify-center rounded-[8px] text-tertiary transition-colors duration-100 hover:bg-accent hover:text-foreground"
            >
              <Icon name="sliders" className="[&>svg]:size-4" />
            </button>
          </Hint>
          <span className="max-w-36 truncate pl-1.5 font-mono text-[10.5px] text-caption" title={cwd}>
            {cwd}
          </span>
        </div>
      </div>

      {/* collapsed rail (56px) */}
      <div
        aria-hidden={!collapsed}
        className={cn(
          "absolute inset-y-0 left-0 flex w-14 flex-col items-center py-3 transition-opacity duration-150 ease-[cubic-bezier(0.4,0,0.2,1)]",
          collapsed ? "rail-in opacity-100" : "pointer-events-none opacity-0"
        )}
      >
        <button title="expand sidebar" onClick={toggleSidebar} className="mb-3 flex size-9 items-center justify-center rounded-[10px] transition-colors duration-100 hover:bg-accent">
          <img src="/logo.png" alt="" draggable={false} className="h-6 w-6 rounded-[6px]" />
        </button>
        <Hint label="new chat" keys={keyChips(keys.new_session)} side="right">
          <button onClick={newChat} className="flex size-9 items-center justify-center rounded-[12px] text-foreground transition-colors duration-100 hover:bg-accent">
            <Icon name="plus" className="[&>svg]:size-4.5" />
          </button>
        </Hint>
        <div className="mt-3 flex flex-col items-center gap-1.5">
          {TOOLS.map((t) => (
            <button
              key={t.id}
              title={t.label}
              onClick={() => setView(t.id)}
              className={cn(
                "flex size-9 items-center justify-center rounded-[12px] transition-colors duration-100",
                view === t.id ? "bg-accent text-foreground" : "text-tertiary hover:bg-accent/60 hover:text-foreground"
              )}
            >
              <Icon name={t.icon} className="[&>svg]:size-4.5" />
            </button>
          ))}
        </div>
        <span className="flex-1" />
        <Hint label="toggle theme" keys={keyChips(keys.toggle_theme)} side="right">
          <button
            onClick={toggleTheme}
            className="flex size-9 items-center justify-center rounded-[10px] text-tertiary transition-colors duration-100 hover:bg-accent hover:text-foreground"
          >
            <Icon name={theme === "light" ? "moon" : "sun"} className="[&>svg]:size-4.5" />
          </button>
        </Hint>
        <Hint label="settings" keys={keyChips(keys.open_settings)} side="right">
          <button
            onClick={openSettings}
            className="mt-1 flex size-9 items-center justify-center rounded-[10px] text-tertiary transition-colors duration-100 hover:bg-accent hover:text-foreground"
          >
            <Icon name="sliders" className="[&>svg]:size-4.5" />
          </button>
        </Hint>
      </div>
    </aside>
  );
}
