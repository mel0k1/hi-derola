import { Icon } from "@/lib/icons";
import { changesTotals, useStore } from "@/store/app";
import { cn } from "@/lib/utils";

export function Topbar() {
  const view = useStore((s) => s.view);
  const plan = useStore((s) => s.plan);
  const togglePlan = useStore((s) => s.togglePlan);
  const toggleSidebar = useStore((s) => s.toggleSidebar);
  const changes = useStore((s) => s.changes);
  const sessions = useStore((s) => s.sessions);
  const sid = useStore((s) => s.sid);
  const openReview = useStore((s) => s.openReview);
  const runCmd = useStore((s) => s.runCmd);
  const totals = changesTotals(changes);

  const title =
    view === "chat" ? (sessions.find((s) => s.id === sid)?.title || "new chat") : view === "sandbox" ? "Sandbox" : view === "hosts" ? "Remote hosts" : view === "skills" ? "Skills" : "Crew";

  return (
    <header className="flex h-12 flex-none items-center gap-1 border-b-[0.5px] border-hairline px-2.5">
      <button
        title="toggle sidebar (ctrl+b)"
        onClick={toggleSidebar}
        className="flex size-8 items-center justify-center rounded-[8px] text-tertiary transition-colors duration-100 hover:bg-accent hover:text-foreground"
      >
        <Icon name="menu" className="[&>svg]:size-4" />
      </button>
      <span className="mx-1 h-4 w-px flex-none bg-hairline" />
      <h1 className="min-w-0 flex-1 truncate text-[14px] font-medium text-foreground">{title}</h1>
      <button
        title="plan mode: read-only research, the agent proposes a plan (ctrl+shift+p)"
        onClick={togglePlan}
        className={cn(
          "flex h-7 items-center gap-1.5 rounded-[8px] px-2.5 text-[12.5px] font-medium transition-colors duration-100",
          plan ? "bg-info/12 text-info" : "text-muted-foreground hover:bg-accent hover:text-foreground"
        )}
      >
        plan
      </button>
      {view === "chat" && (
        <>
          <span className="mx-0.5 h-4 w-px flex-none bg-hairline" />
          {totals.files > 0 && (
            <button
              title={`review session changes: ${totals.files} file(s), +${totals.adds} -${totals.dels}`}
              onClick={openReview}
              className="relative flex size-8 items-center justify-center rounded-[8px] text-tertiary transition-colors duration-100 hover:bg-accent hover:text-foreground"
            >
              <Icon name="diff" className="[&>svg]:size-4" />
              <span
                className={cn(
                  "msg-in absolute top-0.5 right-0.5 flex size-3.5 items-center justify-center rounded-full font-mono text-[8.5px] font-bold",
                  totals.dels > 0 ? "bg-destructive text-white" : "bg-info text-white"
                )}
              >
                {totals.files}
              </span>
            </button>
          )}
          <button
            title="undo file changes (ctrl+z)"
            onClick={() => runCmd("/undo")}
            className="flex size-8 items-center justify-center rounded-[8px] text-tertiary transition-colors duration-100 hover:bg-accent hover:text-foreground"
          >
            <Icon name="undo" className="[&>svg]:size-4" />
          </button>
          <button
            title="redo (ctrl+shift+z)"
            onClick={() => runCmd("/redo")}
            className="flex size-8 items-center justify-center rounded-[8px] text-tertiary transition-colors duration-100 hover:bg-accent hover:text-foreground"
          >
            <Icon name="redo" className="[&>svg]:size-4" />
          </button>
        </>
      )}
      <button
        title="command palette (ctrl+k)"
        onClick={() => useStore.setState({ palette: { open: true, query: "", idx: 0 } })}
        className="flex size-8 items-center justify-center rounded-[8px] text-tertiary transition-colors duration-100 hover:bg-accent hover:text-foreground"
      >
        <Icon name="command" className="[&>svg]:size-4" />
      </button>
    </header>
  );
}
