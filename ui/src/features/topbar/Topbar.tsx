import { Icon } from "@/lib/icons";
import { changesTotals, useStore } from "@/store/app";
import { cn } from "@/lib/utils";

function ModelSelect() {
  const model = useStore((s) => s.model);
  const models = useStore((s) => s.models);
  const runCmd = useStore((s) => s.runCmd);
  const list = [model, ...models.filter((m) => m !== model)].filter(Boolean);
  if (!list.length) return null;
  return (
    <select
      value={model}
      title="switch model"
      onChange={(e) => {
        const v = e.target.value;
        if (v && v !== model) runCmd("/model " + v);
      }}
      className="h-7 max-w-52 cursor-pointer appearance-none truncate rounded-[8px] border-[0.5px] border-border bg-transparent px-2 pr-6 font-mono text-[12px] text-muted-foreground transition-colors duration-100 outline-none hover:bg-accent focus:border-info"
      style={{
        backgroundImage:
          "url(\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 24 24' fill='none' stroke='%2381858C' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpath d='M6 9l6 6 6-6'/%3E%3C/svg%3E\")",
        backgroundRepeat: "no-repeat",
        backgroundPosition: "right 6px center",
      }}
    >
      {list.map((m) => (
        <option key={m} value={m} className="bg-popover text-popover-foreground">
          {m}
        </option>
      ))}
    </select>
  );
}

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
      {view === "chat" && <ModelSelect />}
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
