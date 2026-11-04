import { Icon } from "@/lib/icons";
import { Hint, keyChips } from "@/lib/hint";
import { changesTotals, useStore } from "@/store/app";
import { sessionToMarkdown } from "@/lib/sessionMd";
import { copyText, cn } from "@/lib/utils";

export function Topbar() {
  const view = useStore((s) => s.view);
  const plan = useStore((s) => s.plan);
  const togglePlan = useStore((s) => s.togglePlan);
  const toggleSidebar = useStore((s) => s.toggleSidebar);
  const changes = useStore((s) => s.changes);
  const sessions = useStore((s) => s.sessions);
  const sid = useStore((s) => s.sid);
  const transcript = useStore((s) => s.transcript);
  const openReview = useStore((s) => s.openReview);
  const runCmd = useStore((s) => s.runCmd);
  const keys = useStore((s) => s.keys);
  const totals = changesTotals(changes);

  const title =
    view === "chat" ? (sessions.find((s) => s.id === sid)?.title || "new chat") : view === "sandbox" ? "Sandbox" : view === "hosts" ? "Remote hosts" : view === "skills" ? "Skills" : "Crew";

  const iconBtn =
    "flex size-8 items-center justify-center rounded-[8px] text-tertiary transition-all duration-100 hover:bg-accent hover:text-foreground active:scale-95";

  // dsh session-log-export: the whole transcript on the clipboard as markdown
  const copySessionMd = () => {
    const t = sessions.find((s) => s.id === sid)?.title || "session";
    void copyText(sessionToMarkdown(t, transcript)).then((ok) => {
      useStore.getState().toast(ok ? "session copied as markdown" : "copy failed");
    });
  };

  return (
    <header className="flex h-12 flex-none items-center gap-1 border-b-[0.5px] border-hairline px-2.5">
      <Hint label="toggle sidebar" keys={keyChips(keys.toggle_sidebar)}>
        <button onClick={toggleSidebar} className={iconBtn}>
          <Icon name="menu" className="[&>svg]:size-4" />
        </button>
      </Hint>
      <span className="mx-1 h-4 w-px flex-none bg-hairline" />
      <h1 className="min-w-0 flex-1 truncate text-[14px] font-medium text-foreground">{title}</h1>
      <Hint label="plan mode: read-only research, the agent proposes a plan" keys={keyChips(keys.toggle_plan)}>
        <button
          onClick={togglePlan}
          className={cn(
            "flex h-7 items-center gap-1.5 rounded-[8px] px-2.5 text-[12.5px] font-medium transition-colors duration-100",
            plan ? "bg-info/12 text-info" : "text-muted-foreground hover:bg-accent hover:text-foreground"
          )}
        >
          plan
        </button>
      </Hint>
      {view === "chat" && (
        <>
          <span className="mx-0.5 h-4 w-px flex-none bg-hairline" />
          {totals.files > 0 && (
            <Hint label={`review session changes: ${totals.files} file(s), +${totals.adds} -${totals.dels}`}>
              <button onClick={openReview} className={cn(iconBtn, "relative")}>
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
            </Hint>
          )}
          <Hint label="undo file changes" keys={keyChips(keys.undo)}>
            <button onClick={() => runCmd("/undo")} className={iconBtn}>
              <Icon name="undo" className="[&>svg]:size-4" />
            </button>
          </Hint>
          <Hint label="redo file changes" keys={keyChips(keys.redo)}>
            <button onClick={() => runCmd("/redo")} className={iconBtn}>
              <Icon name="redo" className="[&>svg]:size-4" />
            </button>
          </Hint>
          {transcript.length > 0 && (
            <Hint label="copy session as markdown">
              <button onClick={copySessionMd} className={iconBtn}>
                <Icon name="copy" className="[&>svg]:size-4" />
              </button>
            </Hint>
          )}
        </>
      )}
      <Hint label="command palette" keys={keyChips(keys.palette)}>
        <button
          onClick={() => useStore.setState({ palette: { open: true, query: "", idx: 0 } })}
          className={iconBtn}
        >
          <Icon name="command" className="[&>svg]:size-4" />
        </button>
      </Hint>
    </header>
  );
}
