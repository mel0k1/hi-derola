import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Icon } from "@/lib/icons";
import { DiffLines } from "@/lib/md";
import { changesTotals, useStore } from "@/store/app";
import { cn } from "@/lib/utils";
import { useT } from "@/lib/i18n";

function RevPath({ path }: { path: string }) {
  const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  if (slash > 0) {
    return (
      <span className="min-w-0 flex-1 truncate">
        <span className="text-muted-foreground">{path.slice(0, slash + 1)}</span>
        {path.slice(slash + 1)}
      </span>
    );
  }
  return <span className="min-w-0 flex-1 truncate">{path}</span>;
}

export function ReviewDialog() {
  const open = useStore((s) => s.reviewOpen);
  const closeReview = useStore((s) => s.closeReview);
  const changes = useStore((s) => s.changes);
  const expanded = useStore((s) => s.reviewExpanded);
  const t = useT();

  const files = Object.entries(changes).filter(([k]) => !k.startsWith("__patch__"));
  const patches = Object.entries(changes).filter(([k]) => k.startsWith("__patch__"));
  files.sort((a, b) => b[1].adds + b[1].dels - (a[1].adds + a[1].dels));
  const totals = changesTotals(changes);
  const pt = { adds: 0, dels: 0 };
  for (const [, c] of patches) {
    pt.adds += c.adds;
    pt.dels += c.dels;
  }

  const toggle = (key: string) => {
    useStore.setState((s) => {
      const next = new Set(s.reviewExpanded);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return { reviewExpanded: next };
    });
  };

  return (
    <Dialog open={open} onOpenChange={(o) => !o && closeReview()}>
      <DialogContent className="flex h-[82vh] w-[min(760px,94vw)] flex-col gap-0 p-0" showCloseButton={false}>
        <DialogHeader className="flex-row items-center justify-between border-b border-border px-4 py-3">
          <DialogTitle className="flex items-center gap-2 text-[14px]">
            <Icon name="diff" className="text-primary" /> {t("rv.title")}
          </DialogTitle>
          <Button variant="ghost" size="icon-sm" className="text-muted-foreground" onClick={closeReview}>
            <Icon name="x" />
          </Button>
        </DialogHeader>
        <div className="min-h-0 flex-1 overflow-y-auto p-2.5">
          {!files.length && !patches.length ? (
            <div className="p-4 text-center text-[13px] text-muted-foreground">{t("rv.empty")}</div>
          ) : (
            <div className="flex flex-col gap-1.5">
              {files.map(([path, c]) => {
                const isOpen = expanded.has(path);
                return (
                  <div key={path} className="overflow-hidden rounded-lg border border-border">
                    <div
                      className={cn("flex items-center gap-2 bg-secondary/60 px-3 py-2 font-mono text-[12px]", c.rows && "cursor-pointer select-none hover:text-primary")}
                      onClick={() => c.rows && toggle(path)}
                    >
                      <Icon name="file" className="flex-none text-muted-foreground" />
                      <RevPath path={path} />
                      <span className="flex flex-none gap-2 text-[11.5px] font-semibold">
                        {c.adds > 0 && <span className="text-success">+{c.adds}</span>}
                        {c.dels > 0 && <span className="text-destructive">−{c.dels}</span>}
                        {!c.rows && <span className="font-normal text-muted-foreground">({t("rv.counts_only")})</span>}
                      </span>
                    </div>
                    {c.rows && isOpen && (
                      <div className="px-2 pb-2">
                        <DiffLines rows={c.rows} />
                      </div>
                    )}
                  </div>
                );
              })}
              {patches.map(([key, c]) => {
                const isOpen = expanded.has(key);
                return (
                  <div key={key} className="overflow-hidden rounded-lg border border-border">
                    <div
                      className="flex cursor-pointer items-center gap-2 bg-secondary/60 px-3 py-2 font-mono text-[12px] select-none hover:text-primary"
                      onClick={() => toggle(key)}
                    >
                      <span className="min-w-0 flex-1 truncate">{c.label || "patch"}</span>
                      <span className="flex flex-none gap-2 text-[11.5px] font-semibold">
                        {c.adds > 0 && <span className="text-success">+{c.adds}</span>}
                        {c.dels > 0 && <span className="text-destructive">−{c.dels}</span>}
                      </span>
                    </div>
                    {isOpen && (
                      <div className="px-2 pb-2">
                        <DiffLines rows={c.rows || []} />
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </div>
        <div className="flex items-center justify-between gap-3 border-t border-border px-4 py-2.5">
          <span className="text-[11.5px] text-muted-foreground">
            {t("rv.footer", { n: totals.files, adds: totals.adds, dels: totals.dels })}
            {patches.length ? ` · ${t("rv.patches", { n: patches.length, adds: pt.adds, dels: pt.dels })}` : ""}
          </span>
          <span className="text-[11.5px] text-muted-foreground">{t("rv.note")}</span>
        </div>
      </DialogContent>
    </Dialog>
  );
}
