import { useEffect, useState } from "react";
import { Sidebar } from "@/features/sidebar/Sidebar";
import { Topbar } from "@/features/topbar/Topbar";
import { ChatView } from "@/features/chat/ChatView";
import { SandboxView } from "@/features/sandbox/SandboxView";
import { HostsView } from "@/features/hosts/HostsView";
import { SkillsView } from "@/features/skills/SkillsView";
import { CrewView } from "@/features/crew/CrewView";
import { ConfirmDialog } from "@/features/dialogs/ConfirmDialog";
import { AskDialog } from "@/features/dialogs/AskDialog";
import { FilesDialog } from "@/features/dialogs/FilesDialog";
import { SettingsDialog } from "@/features/dialogs/SettingsDialog";
import { HotkeysDialog } from "@/features/dialogs/HotkeysDialog";
import { ReviewDialog } from "@/features/dialogs/ReviewDialog";
import { PaletteDialog } from "@/features/dialogs/PaletteDialog";
import { Toasts } from "@/features/dialogs/Toasts";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog";
import { useStore } from "@/store/app";
import { comboFromEvent, parseCombo, sameCombo } from "@/lib/keys";
import { invoke, listenRaw } from "@/lib/tauri";
import { ContextMenuHost } from "@/lib/menu";
import { Icon } from "@/lib/icons";

function DropOverlay({ show }: { show: boolean }) {
  if (!show) return null;
  return (
    <div className="pointer-events-none fixed inset-0 z-[70] flex items-center justify-center bg-background/60 backdrop-blur-[2px]">
      <div className="msg-in flex flex-col items-center gap-3 rounded-[24px] border-2 border-dashed border-info/50 bg-card/95 px-10 py-8 shadow-panel">
        <Icon name="download" className="text-info [&>svg]:size-8" />
        <div className="text-[15px] font-medium">drop files or folders to attach</div>
        <div className="text-[12px] text-caption">they will be added to your next message</div>
      </div>
    </div>
  );
}

// dsh StopSequence: the stop shortcut must be pressed twice — a single press
// only nudges, a second press within the window actually cancels the run
const STOP_PRESS_WINDOW = 700;
let lastStopPress = 0;

function stopSequence(event: KeyboardEvent, st: ReturnType<typeof useStore.getState>): boolean {
  const now = Date.now();
  if (event.repeat) return true;
  if (now - lastStopPress < STOP_PRESS_WINDOW) {
    lastStopPress = 0;
    return false; // caller stops the run
  }
  lastStopPress = now;
  st.toast("press the stop shortcut again to cancel");
  return true;
}

export default function App() {
  const view = useStore((s) => s.view);
  const init = useStore((s) => s.init);
  const [dragOver, setDragOver] = useState(false);

  useEffect(() => {
    init();
  }, [init]);

  // native drag & drop: tauri webview events carry real file paths
  useEffect(() => {
    if (!window.__TAURI__) return;
    let depth = 0;
    const unsubs: (() => void)[] = [];
    const on = (name: string, cb: (p: unknown) => void) => {
      listenRaw(name, cb)
        .then((u) => unsubs.push(u))
        .catch(() => {});
    };
    on("tauri://drag-enter", () => {
      depth++;
      setDragOver(true);
    });
    on("tauri://drag-over", () => setDragOver(true));
    on("tauri://drag-leave", () => {
      depth = Math.max(0, depth - 1);
      if (!depth) setDragOver(false);
    });
    on("tauri://drag-drop", (p) => {
      depth = 0;
      setDragOver(false);
      const paths = (p as { paths?: string[] })?.paths || [];
      if (!paths.length) return;
      const st = useStore.getState();
      void (async () => {
        for (const x of paths) await st.attachPath(x);
        st.toast(`attached ${paths.length} item(s)`);
      })();
    });
    return () => unsubs.forEach((u) => u());
  }, []);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      const st = useStore.getState();
      if (st.ask) {
        if (e.key === "Escape") {
          e.preventDefault();
          st.skipAsk();
        }
        return;
      }
      if (st.confirm?.open) {
        const typing = e.target instanceof HTMLElement && (e.target.tagName === "INPUT" || e.target.tagName === "TEXTAREA");
        if (typing) {
          if (e.key === "Escape") {
            e.preventDefault();
            st.resolveConfirm(false);
          }
          return;
        }
        const c = comboFromEvent(e);
        if (!c) return;
        const plain = !c.ctrl && !c.alt && !c.meta;
        if (c.name === "y" && plain) {
          e.preventDefault();
          st.resolveConfirm(true);
        } else if ((c.name === "n" || (c.name === "escape" && sameCombo(c, parseCombo(st.keys.stop)))) && plain) {
          e.preventDefault();
          st.resolveConfirm(false);
        } else if (c.name === "a" && plain) {
          e.preventDefault();
          st.allowAll();
        } else if (c.name === "w" && plain) {
          e.preventDefault();
          st.resolveConfirm(true, true);
        }
        return;
      }
      if (st.view === "sandbox" && e.key === "Escape" && st.sbxWizard && !st.settingsOpen && !st.files.open && !st.palette.open) {
        e.preventDefault();
        st.closeWizard();
        return;
      }
      if (st.files.open) {
        if (e.key === "Escape") {
          e.preventDefault();
          st.closeFiles();
        }
        return;
      }
      if (st.settingsOpen) {
        return;
      }
      if (st.reviewOpen) {
        if (e.key === "Escape") {
          e.preventDefault();
          st.closeReview();
        }
        return;
      }
      if (st.palette.open) {
        return;
      }
      const hit = (action: string) => sameCombo(comboFromEvent(e), parseCombo(st.keys[action]));
      if (st.waiting && hit("stop")) {
        e.preventDefault();
        if (stopSequence(e, st)) return;
        invoke("stop").catch(() => {});
        return;
      }
      const inInput = e.target instanceof HTMLElement && (e.target.id === "chat-input" || e.target.tagName === "TEXTAREA");
      if (inInput) {
        if (hit("send")) {
          e.preventDefault();
          const el = document.getElementById("chat-input") as HTMLTextAreaElement | null;
          if (el) st.doSend(el.value);
          return;
        }
      }
      if (hit("new_session")) {
        e.preventDefault();
        st.newChat();
      } else if (hit("open_settings")) {
        e.preventDefault();
        useStore.setState({ settingsOpen: true });
      } else if (hit("toggle_sidebar")) {
        e.preventDefault();
        st.toggleSidebar();
      } else if (hit("toggle_theme")) {
        e.preventDefault();
        st.toggleTheme();
      } else if (hit("toggle_plan")) {
        e.preventDefault();
        st.togglePlan();
      } else if (hit("undo")) {
        e.preventDefault();
        st.runCmd("/undo");
      } else if (hit("redo")) {
        e.preventDefault();
        st.runCmd("/redo");
      } else if (hit("toggle_thinking")) {
        e.preventDefault();
        const items = st.transcript.filter((t) => t.kind === "think");
        if (items.length) {
          const last = items[items.length - 1];
          useStore.setState((s) => ({
            transcript: s.transcript.map((t) => (t.id === last.id && t.kind === "think" ? { ...t, open: !t.open } : t)),
          }));
        }
      } else if (hit("shortcuts")) {
        e.preventDefault();
        useStore.setState({ hotkeysOpen: true });
      } else if (hit("palette")) {
        e.preventDefault();
        useStore.setState({ palette: { open: true, query: "", idx: 0 } });
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  return (
    <div className="flex h-full overflow-hidden bg-background text-foreground">
      <Sidebar />
      <div className="flex min-w-0 flex-1 flex-col">
        <Topbar />
        <main key={view} className="view-in min-h-0 flex-1">
          {view === "chat" && <ChatView />}
          {view === "sandbox" && <SandboxView />}
          {view === "hosts" && <HostsView />}
          {view === "skills" && <SkillsView />}
          {view === "crew" && <CrewView />}
        </main>
      </div>
      <ConfirmDialog />
      <AskDialog />
      <FilesDialog />
      <SettingsDialog />
      <HotkeysDialog />
      <ReviewDialog />
      <PaletteDialog />
      <Toasts />
      <ContextMenuHost />
      <DropOverlay show={dragOver} />
      <QuitDialog />
    </div>
  );
}

// shown when the window is closed (or tray quit chosen) while a run is live
function QuitDialog() {
  const quitAsk = useStore((s) => s.quitAsk);
  const quitHide = useStore((s) => s.quitHide);
  const quitNow = useStore((s) => s.quitNow);
  return (
    <AlertDialog open={quitAsk}>
      <AlertDialogContent className="max-w-sm gap-3 rounded-[20px] p-5">
        <AlertDialogHeader className="gap-1.5">
          <AlertDialogTitle className="text-[15px]">agent is still running</AlertDialogTitle>
          <AlertDialogDescription className="text-[13px]">
            quitting now interrupts the current task and its queued messages.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter className="gap-2">
          <AlertDialogCancel
            onClick={() => useStore.setState({ quitAsk: false })}
            className="h-8 rounded-[10px] text-[12.5px]"
          >
            stay
          </AlertDialogCancel>
          <button
            onClick={quitHide}
            className="h-8 rounded-[10px] bg-secondary px-3 text-[12.5px] font-medium text-foreground transition-colors duration-100 hover:bg-muted"
          >
            hide to tray
          </button>
          <AlertDialogAction
            onClick={quitNow}
            className="h-8 rounded-[10px] bg-destructive text-[12.5px] text-white hover:bg-destructive/90"
          >
            quit
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
