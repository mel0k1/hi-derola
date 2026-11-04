import { useEffect, useMemo, useState } from "react";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Icon } from "@/lib/icons";
import { useStore } from "@/store/app";
import { invoke } from "@/lib/tauri";
import { fmtTokens, joinPath, copyText } from "@/lib/utils";
import { fileManagerLabel, openContextMenu } from "@/lib/menu";

function Crumbs({ path }: { path: string }) {
  const browse = useStore((s) => s.browse);
  const isWin = /\\/.test(path) || /^[A-Za-z]:/.test(path);
  const sep = isWin ? "\\" : "/";
  const parts = useMemo(() => path.split(/[\\/]/).filter(Boolean), [path]);
  const target = (i: number) => (isWin ? parts.slice(0, i + 1).join(sep) : "/" + parts.slice(0, i + 1).join("/"));
  return (
    <div className="flex min-w-0 flex-1 items-center gap-0.5 overflow-hidden font-mono text-[11.5px]">
      {isWin ? (
        <button className="rounded-[6px] px-1 py-0.5 text-muted-foreground transition-colors duration-100 hover:bg-accent hover:text-foreground" onClick={() => browse(parts[0] ? parts[0] : path)}>
          {parts[0] || path}
        </button>
      ) : (
        <button className="rounded-[6px] px-1 py-0.5 text-muted-foreground transition-colors duration-100 hover:bg-accent hover:text-foreground" onClick={() => browse("/")}>
          /
        </button>
      )}
      {parts.slice(isWin ? 1 : 0).map((p, i) => {
        const idx = i + (isWin ? 1 : 0);
        const last = idx === parts.length - 1;
        return (
          <span key={idx} className="flex min-w-0 items-center gap-0.5">
            <span className="text-caption">›</span>
            <button
              className={last ? "truncate rounded-[6px] px-1 py-0.5 text-foreground" : "truncate rounded-[6px] px-1 py-0.5 text-muted-foreground transition-colors duration-100 hover:bg-accent hover:text-foreground"}
              onClick={() => !last && browse(target(idx))}
            >
              {p}
            </button>
          </span>
        );
      })}
    </div>
  );
}

export function FilesDialog() {
  const files = useStore((s) => s.files);
  const browse = useStore((s) => s.browse);
  const closeFiles = useStore((s) => s.closeFiles);
  const attachPath = useStore((s) => s.attachPath);
  const note = useStore((s) => s.note);
  const [filter, setFilter] = useState("");

  useEffect(() => {
    if (files.open) setFilter("");
  }, [files.open, files.data?.path]);

  const path = files.data?.path || "";
  const entries = (files.data?.entries || []).filter((e) => !filter.trim() || e.name.toLowerCase().includes(filter.trim().toLowerCase()));

  const pickNative = async (kind: "files" | "folder") => {
    const st = useStore.getState();
    try {
      if (kind === "files") {
        const paths = await invoke<string[]>("pick_files");
        for (const p of paths) await st.attachPath(p);
        if (paths.length) st.toast(`attached ${paths.length} file(s)`);
      } else {
        const p = await invoke<string | null>("pick_folder");
        if (p) {
          await st.attachPath(p);
          st.toast("folder attached");
        }
      }
    } catch (e) {
      note(String(e));
    }
  };

  return (
    <Dialog open={files.open} onOpenChange={(o) => !o && closeFiles()}>
      <DialogContent className="flex h-[70vh] w-[min(680px,94vw)] flex-col gap-0 p-0" showCloseButton={false}>
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
          {path && <Crumbs path={path} />}
          <span className="flex-1" />
          <div className="relative w-44 flex-none">
            <Icon name="search" className="absolute top-1/2 left-2 -translate-y-1/2 text-caption [&>svg]:size-3" />
            <input
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              placeholder="filter"
              spellCheck={false}
              className="h-7 w-full rounded-[8px] border-[0.5px] border-hairline bg-background pr-2 pl-7 text-[12px] outline-none placeholder:text-caption focus-visible:border-info focus-visible:ring-1 focus-visible:ring-info/30"
            />
          </div>
        </div>
        <ScrollArea className="min-h-0 flex-1">
          <div className="p-1.5">
            {files.data && files.data.entries.length === 0 && <div className="p-3 text-[12.5px] text-muted-foreground">empty folder</div>}
            {files.data && files.data.entries.length > 0 && entries.length === 0 && <div className="p-3 text-[12.5px] text-muted-foreground">no files match "{filter.trim()}"</div>}
            {entries.map((e) => {
              const full = files.data ? joinPath(files.data.path, e.name) : "";
              return (
              <div
                key={e.name}
                className="group flex cursor-pointer items-center gap-2.5 rounded-[10px] px-2.5 py-1.5 transition-colors duration-100 hover:bg-accent"
                onContextMenu={(ev) =>
                  openContextMenu(ev, [
                    {
                      id: "reveal",
                      label: fileManagerLabel(),
                      icon: "folder",
                      onSelect: () => invoke("reveal_path", { path: full }).catch(() => {}),
                    },
                    {
                      id: "copy",
                      label: "Copy Path",
                      icon: "copy",
                      onSelect: () => {
                        void copyText(full).then((ok) => ok && useStore.getState().toast("copied"));
                      },
                    },
                  ])
                }
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
                {!e.dir && (
                  <button
                    title="attach this file"
                    onClick={(ev) => {
                      ev.stopPropagation();
                      if (files.data) {
                        attachPath(joinPath(files.data.path, e.name));
                        closeFiles();
                      }
                    }}
                    className="rounded-[6px] p-0.5 text-caption opacity-0 transition-all duration-100 group-hover:opacity-100 hover:text-foreground"
                  >
                    <Icon name="plus" className="[&>svg]:size-3.5" />
                  </button>
                )}
              </div>
              );
            })}
          </div>
        </ScrollArea>
        <div className="flex flex-wrap items-center justify-between gap-2 border-t border-border px-4 py-2.5">
          <span className="text-[11.5px] text-muted-foreground">click a file to attach · folders attach as tree</span>
          <div className="flex items-center gap-1.5">
            <Button variant="ghost" size="sm" onClick={() => pickNative("files")}>
              <Icon name="file" className="[&>svg]:size-3.5" /> files…
            </Button>
            <Button variant="ghost" size="sm" onClick={() => pickNative("folder")}>
              <Icon name="folder" className="[&>svg]:size-3.5" /> folder…
            </Button>
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
              <Icon name="up" className="[&>svg]:size-3.5" /> this folder
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
