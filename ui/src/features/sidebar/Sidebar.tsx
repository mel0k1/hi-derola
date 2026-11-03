import { Button } from "@/components/ui/button";
import { Icon } from "@/lib/icons";
import { fmtCost, fmtRel } from "@/lib/utils";
import { useStore } from "@/store/app";
import { invoke } from "@/lib/tauri";
import { useState } from "react";
import { cn } from "@/lib/utils";

const TOOLS: { id: "sandbox" | "hosts" | "skills" | "crew"; icon: string; label: string; title: string }[] = [
  { id: "sandbox", icon: "box", label: "sandbox", title: "sandbox - local QEMU VMs for isolated agent work" },
  { id: "hosts", icon: "server", label: "remote hosts", title: "remote hosts - run the agent on your servers over ssh" },
  { id: "skills", icon: "skill", label: "skills", title: "skills - plugin folders with SKILL.md, toggles, git install" },
  { id: "crew", icon: "crew", label: "crew", title: "crew - several AI agents in one session" },
];

function SessionRow({ id, title, count, updated, cost, active }: { id: string; title: string; count: number; updated: number; cost?: number; active: boolean }) {
  const openSession = useStore((s) => s.openSession);
  const waiting = useStore((s) => s.waiting);
  const note = useStore((s) => s.note);
  const refreshSessions = useStore((s) => s.refreshSessions);
  const [confirming, setConfirming] = useState(false);

  return (
    <div
      role="button"
      tabIndex={0}
      onClick={() => openSession(id)}
      onKeyDown={(e) => e.key === "Enter" && openSession(id)}
      className={cn(
        "group flex cursor-pointer items-center gap-1 rounded-md border border-transparent py-1.5 pr-1 pl-2.5 transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring/50",
        "hover:bg-accent/70",
        active && "border-border bg-secondary/70"
      )}
    >
      <div className="min-w-0 flex-1">
        <div className={cn("truncate text-[13px] leading-tight", active && "text-foreground")}>{title || "new chat"}</div>
        <div className="font-mono text-[10.5px] text-muted-foreground">
          {count} msgs · {fmtRel(updated)}
          {cost && cost > 0 ? ` · ${fmtCost(cost)}` : ""}
        </div>
      </div>
      <Button
        variant="ghost"
        size="icon-xs"
        title="delete"
        className={cn(
          "text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100 hover:bg-transparent hover:text-destructive",
          confirming && "text-destructive opacity-100"
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
        <Icon name={confirming ? "check" : "trash"} />
      </Button>
    </div>
  );
}

export function Sidebar() {
  const hidden = useStore((s) => s.sidebarHidden);
  const view = useStore((s) => s.view);
  const setView = useStore((s) => s.setView);
  const sessions = useStore((s) => s.sessions);
  const sid = useStore((s) => s.sid);
  const cwd = useStore((s) => s.cwd);
  const newChat = useStore((s) => s.newChat);

  return (
    <aside
      className={cn(
        "z-10 flex w-68 flex-none flex-col border-r border-border bg-sidebar text-sidebar-foreground transition-[margin] duration-200 ease-out",
        hidden && "-ml-72"
      )}
    >
      <div className="flex items-center gap-2 px-4 pt-3.5 pb-1">
        <img src="/logo.png" alt="" draggable={false} className="h-6 w-6 flex-none rounded-[6px]" />
        <div className="text-[15px] font-bold tracking-tight">
          Hi!<span className="font-medium">Derola</span>
        </div>
      </div>

      <div className="px-3 pt-2 pb-1">
        <Button variant="outline" onClick={newChat} className="w-full justify-start gap-2 border-input bg-transparent font-semibold shadow-none hover:border-primary/50 hover:text-foreground dark:bg-transparent">
          <Icon name="plus" className="text-primary" />
          new chat
        </Button>
      </div>

      <div className="px-4 pt-4 pb-1.5 font-mono text-[10.5px] font-semibold tracking-[0.12em] text-muted-foreground uppercase">tools</div>
      <nav className="flex flex-col gap-0.5 px-2">
        {TOOLS.map((t) => (
          <button
            key={t.id}
            title={t.title}
            onClick={() => setView(t.id)}
            className={cn(
              "flex items-center gap-2.5 rounded-md px-2 py-1.5 text-left text-[13px] transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring/50",
              "text-sidebar-foreground/85 hover:bg-accent hover:text-accent-foreground",
              view === t.id && "bg-primary/12 font-medium text-primary hover:bg-primary/15 hover:text-primary"
            )}
          >
            <Icon name={t.icon} className={view === t.id ? "text-primary" : "text-muted-foreground"} />
            {t.label}
          </button>
        ))}
      </nav>

      <div className="px-4 pt-4 pb-1.5 font-mono text-[10.5px] font-semibold tracking-[0.12em] text-muted-foreground uppercase">history</div>
      <div className="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
        {sessions.length === 0 ? (
          <div className="px-2.5 py-1.5 text-[12.5px] text-muted-foreground">no history yet</div>
        ) : (
          sessions.map((s) => (
            <SessionRow key={s.id} id={s.id} title={s.title} count={s.count} updated={s.updated} cost={s.cost} active={s.id === sid} />
          ))
        )}
      </div>

      <div className="flex items-center gap-1 border-t border-border px-2.5 py-2">
        <ThemeBtn />
        <SettingsBtn />
        <span className="flex-1" />
        <span className="max-w-32 truncate font-mono text-[10.5px] text-muted-foreground" title={cwd}>
          {cwd}
        </span>
      </div>
    </aside>
  );
}

function ThemeBtn() {
  const theme = useStore((s) => s.theme);
  const toggleTheme = useStore((s) => s.toggleTheme);
  return (
    <Button variant="ghost" size="icon-sm" className="text-muted-foreground" title="toggle theme (ctrl+shift+t)" onClick={toggleTheme}>
      <Icon name={theme === "light" ? "moon" : "sun"} />
    </Button>
  );
}

function SettingsBtn() {
  return (
    <Button variant="ghost" size="icon-sm" className="text-muted-foreground" title="settings (ctrl+,)" onClick={() => useStore.setState({ settingsOpen: true })}>
      <Icon name="sliders" />
    </Button>
  );
}
