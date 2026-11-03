import { useStore } from "@/store/app";
import { fmtCost, fmtTokens, cn } from "@/lib/utils";

export function Statusbar() {
  const kind = useStore((s) => s.kind);
  const model = useStore((s) => s.model);
  const plan = useStore((s) => s.plan);
  const waiting = useStore((s) => s.waiting);
  const tokens = useStore((s) => s.tokens);
  const ctx = useStore((s) => s.ctx);
  const runCmd = useStore((s) => s.runCmd);

  let right = "";
  if (!waiting && (tokens.in || tokens.out)) {
    right = `${fmtTokens(tokens.in)} in · ${fmtTokens(tokens.out)} out`;
    if (tokens.cached > 0) right += ` (${fmtTokens(tokens.cached)} cached)`;
    if (tokens.cost > 0) right += " · " + fmtCost(tokens.cost);
  }

  const pct = ctx.limit > 0 && ctx.used > 0 ? Math.min(100, Math.round((ctx.used / ctx.limit) * 100)) : null;

  return (
    <footer className="flex h-7 flex-none items-center justify-between gap-3 border-t border-border bg-sidebar/60 px-3 font-mono text-[11px] text-muted-foreground">
      <span className="truncate">
        {kind}
        {model ? ` · ${model}` : ""}
        {plan ? " · plan" : ""}
      </span>
      <div className="flex items-center gap-2.5">
        {pct !== null && (
          <button
            title={`context: ${fmtTokens(ctx.used)} of ${fmtTokens(ctx.limit)} tokens · click to /compact`}
            onClick={() => runCmd("/compact")}
            className={cn(
              "rounded-full border px-2 py-px font-semibold transition-colors hover:border-primary/60 hover:text-primary",
              pct >= 92
                ? "border-destructive/60 text-destructive"
                : pct >= 80
                  ? "border-warning/60 text-warning"
                  : "border-border text-muted-foreground"
            )}
          >
            {pct}% ctx
          </button>
        )}
        <span className={cn(waiting && "text-primary")}>
          {waiting ? <span className="typing-dots">thinking</span> : right}
        </span>
      </div>
    </footer>
  );
}
