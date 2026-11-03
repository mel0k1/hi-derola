import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { useStore } from "@/store/app";
import { DiffLines } from "@/lib/md";
import { useEffect, useRef } from "react";

export function ConfirmDialog() {
  const confirm = useStore((s) => s.confirm);
  const resolveConfirm = useStore((s) => s.resolveConfirm);
  const allowAll = useStore((s) => s.allowAll);
  const feedbackRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (confirm?.open) {
      (document.activeElement as HTMLElement | null)?.blur();
    }
  }, [confirm?.open]);

  return (
    <Dialog open={!!confirm?.open} onOpenChange={(o) => !o && resolveConfirm(false)}>
      <DialogContent
        className="w-[min(680px,92vw)] gap-3"
        showCloseButton={false}
        onOpenAutoFocus={(e) => e.preventDefault()}
      >
        <DialogHeader>
          <DialogTitle className="text-primary">{confirm ? "run " + confirm.name + "?" : ""}</DialogTitle>
        </DialogHeader>
        <pre className="max-h-48 overflow-y-auto font-mono text-[12.5px] break-words whitespace-pre-wrap">{confirm?.detail}</pre>
        {confirm && confirm.diff.length > 0 && <DiffLines rows={confirm.diff} />}
        <input
          id="confirm-feedback"
          ref={feedbackRef}
          placeholder="feedback on reject (optional) - tell the model what to change"
          spellCheck={false}
          className="w-full rounded-lg border border-input bg-background px-3 py-2 text-[13px] outline-none focus:border-ring"
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              resolveConfirm(false);
            }
          }}
        />
        <DialogFooter className="gap-2">
          <Button variant="ghost" size="sm" onClick={() => resolveConfirm(false)}>
            reject <kbd className="font-mono text-[10px] text-muted-foreground">n</kbd>
          </Button>
          <Button variant="ghost" size="sm" onClick={() => resolveConfirm(true, true)}>
            always <kbd className="font-mono text-[10px] text-muted-foreground">w</kbd>
          </Button>
          <Button variant="ghost" size="sm" onClick={allowAll}>
            allow all <kbd className="font-mono text-[10px] text-muted-foreground">a</kbd>
          </Button>
          <Button size="sm" onClick={() => resolveConfirm(true)}>
            run <kbd className="font-mono text-[10px] opacity-70">y</kbd>
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
