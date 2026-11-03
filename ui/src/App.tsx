import { useEffect } from "react";
import { Sidebar } from "@/features/sidebar/Sidebar";
import { Topbar } from "@/features/topbar/Topbar";
import { ChatView } from "@/features/chat/ChatView";
import { InputBar } from "@/features/chat/InputBar";
import { Statusbar } from "@/features/statusbar/Statusbar";
import { SandboxView } from "@/features/sandbox/SandboxView";
import { HostsView } from "@/features/hosts/HostsView";
import { SkillsView } from "@/features/skills/SkillsView";
import { CrewView } from "@/features/crew/CrewView";
import { ConfirmDialog } from "@/features/dialogs/ConfirmDialog";
import { AskDialog } from "@/features/dialogs/AskDialog";
import { FilesDialog } from "@/features/dialogs/FilesDialog";
import { SettingsDialog } from "@/features/dialogs/SettingsDialog";
import { ReviewDialog } from "@/features/dialogs/ReviewDialog";
import { PaletteDialog } from "@/features/dialogs/PaletteDialog";
import { useStore } from "@/store/app";
import { comboFromEvent, parseCombo, sameCombo } from "@/lib/keys";
import { invoke } from "@/lib/tauri";
import { cn } from "@/lib/utils";

export default function App() {
  const view = useStore((s) => s.view);
  const init = useStore((s) => s.init);

  useEffect(() => {
    init();
  }, [init]);

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
        <div className={cn("relative min-h-0 flex-1", view !== "chat" && "hidden")}>
          <ChatView />
        </div>
        {view === "sandbox" && <SandboxView />}
        {view === "hosts" && <HostsView />}
        {view === "skills" && <SkillsView />}
        {view === "crew" && <CrewView />}
        {view === "chat" && <InputBar />}
        <Statusbar />
      </div>
      <ConfirmDialog />
      <AskDialog />
      <FilesDialog />
      <SettingsDialog />
      <ReviewDialog />
      <PaletteDialog />
    </div>
  );
}
