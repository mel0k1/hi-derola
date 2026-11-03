import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Icon } from "@/lib/icons";
import { useStore } from "@/store/app";
import { fmtTokens, joinPath } from "@/lib/utils";

export function FilesDialog() {
  const files = useStore((s) => s.files);
  const browse = useStore((s) => s.browse);
  const closeFiles = useStore((s) => s.closeFiles);
  const attachPath = useStore((s) => s.attachPath);

  return (
    <Dialog open={files.open} onOpenChange={(o) => !o && closeFiles()}>
      <DialogContent className="flex h-[70vh] w-[min(640px,94vw)] flex-col gap-0 p-0" showCloseButton={false}>
        <DialogHeader className="flex-row items-center justify-between border-b border-border px-4 py-3">
          <DialogTitle className="flex items-center gap-2 text-[14px]">
            <Icon name="paperclip" className="text-primary" /> attach files
          </DialogTitle>
          <Button variant="ghost" size="icon-sm" className="text-muted-foreground" onClick={closeFiles}>
            <Icon name="x" />
          </Button>
        </DialogHeader>
        <div className="flex items-center gap-1.5 border-b border-border bg-secondary/50 px-3 py-2">
          <Button
            variant="ghost"
            size="icon-xs"
            className="text-muted-foreground"
            title="parent folder"
            disabled={!files.parent}
            onClick={() => files.parent && browse(files.parent)}
          >
            <Icon name="up" />
          </Button>
          <span className="truncate font-mono text-[11.5px] text-muted-foreground">
            {files.data?.path}
            {files.data?.is_cwd ? "  ·  cwd" : ""}
          </span>
        </div>
        <ScrollArea className="min-h-0 flex-1">
          <div className="p-1.5">
            {files.data && files.data.entries.length === 0 && (
              <div className="p-3 text-[12.5px] text-muted-foreground">empty folder</div>
            )}
            {files.data?.entries.map((e) => (
              <div
                key={e.name}
                className="flex cursor-pointer items-center gap-2.5 rounded-md px-2.5 py-1.5 hover:bg-accent"
                onClick={async () => {
                  if (e.dir && files.data) {
                    browse(joinPath(files.data.path, e.name));
                  } else if (files.data) {
                    await attachPath(joinPath(files.data.path, e.name));
                    closeFiles();
                  }
                }}
              >
                <Icon name={e.dir ? "folder" : "file"} className={e.dir ? "text-primary" : "text-muted-foreground"} />
                <span className="flex-1 truncate text-[13px]">{e.dir ? e.name + "/" : e.name}</span>
                {!e.dir && <span className="font-mono text-[10.5px] text-muted-foreground">{fmtTokens(e.size)}b</span>}
              </div>
            ))}
          </div>
        </ScrollArea>
        <div className="flex items-center justify-between gap-2.5 border-t border-border px-4 py-2.5">
          <span className="text-[11.5px] text-muted-foreground">click a file to attach · folders attach as tree</span>
          <Button
            variant="ghost"
            size="sm"
            onClick={async () => {
              if (files.data) {
                await attachPath(files.data.path);
                closeFiles();
              }
            }}
          >
            <Icon name="folder" /> attach current folder
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
