import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
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
    <Select value={model} onValueChange={(v) => v && v !== model && runCmd("/model " + v)}>
      <SelectTrigger size="sm" className="h-7.5 max-w-56 border-input bg-secondary/60 font-mono text-[12px] shadow-none hover:border-primary/50">
        <SelectValue placeholder="model" />
      </SelectTrigger>
      <SelectContent className="max-h-80 font-mono text-[12.5px]">
        {list.map((m) => (
          <SelectItem key={m} value={m}>
            {m}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

export function Topbar() {
  const plan = useStore((s) => s.plan);
  const togglePlan = useStore((s) => s.togglePlan);
  const toggleSidebar = useStore((s) => s.toggleSidebar);
  const changes = useStore((s) => s.changes);
  const openReview = useStore((s) => s.openReview);
  const runCmd = useStore((s) => s.runCmd);
  const totals = changesTotals(changes);

  return (
    <header className="flex h-11 flex-none items-center gap-1.5 border-b border-border bg-background/95 px-2.5">
      <Button variant="ghost" size="icon-sm" className="text-muted-foreground" title="toggle sidebar (ctrl+b)" onClick={toggleSidebar}>
        <Icon name="menu" />
      </Button>
      <div className="mx-1 h-4 w-px bg-border" />
      <Button
        variant={plan ? "default" : "ghost"}
        size="sm"
        title="plan mode: read-only research, the agent proposes a plan (ctrl+shift+p)"
        onClick={togglePlan}
        className={cn("h-7.5 px-3 text-[12.5px]", !plan && "text-muted-foreground")}
      >
        plan
      </Button>
      <ModelSelect />
      <span className="flex-1" />
      {totals.files > 0 && (
        <Button
          variant="ghost"
          size="icon-sm"
          className="relative text-muted-foreground"
          title="review session changes"
          onClick={openReview}
        >
          <Icon name="diff" />
          <span
            className={cn(
              "msg-in absolute -top-0.5 -right-1 rounded-full px-1.5 text-[9px] leading-4 font-bold font-mono",
              totals.dels > 0 ? "bg-destructive text-white" : "bg-primary text-primary-foreground"
            )}
          >
            {totals.files}
          </span>
        </Button>
      )}
      <Button variant="ghost" size="icon-sm" className="text-muted-foreground" title="undo file changes (ctrl+z)" onClick={() => runCmd("/undo")}>
        <Icon name="undo" />
      </Button>
      <Button variant="ghost" size="icon-sm" className="text-muted-foreground" title="redo (ctrl+shift+z)" onClick={() => runCmd("/redo")}>
        <Icon name="redo" />
      </Button>
      <Button
        variant="ghost"
        size="icon-sm"
        className="text-muted-foreground"
        title="command palette (ctrl+k)"
        onClick={() => useStore.setState({ palette: { open: true, query: "", idx: 0 } })}
      >
        <Icon name="command" />
      </Button>
    </header>
  );
}
