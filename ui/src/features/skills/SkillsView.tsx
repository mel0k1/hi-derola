import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Icon } from "@/lib/icons";
import { Card, EmptyHint, PanelShell } from "@/features/panels/Panels";
import { useStore } from "@/store/app";
import { invoke } from "@/lib/tauri";

export function SkillsView() {
  const skills = useStore((s) => s.skills);
  const refreshSkills = useStore((s) => s.refreshSkills);
  const note = useStore((s) => s.note);
  const [url, setUrl] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    refreshSkills();
  }, [refreshSkills]);

  return (
    <PanelShell
      icon="skill"
      title="skills"
      sub={
        <>
          any folder with SKILL.md (project <span className="font-mono text-primary">.hi-derola/skills/</span> or global{" "}
          <span className="font-mono text-primary">~/.config/hi-derola/skills/</span>), recursive, model-agnostic, toggleable
        </>
      }
      action={
        <Button size="sm" onClick={() => refreshSkills()}>
          <Icon name="check" /> rescan
        </Button>
      }
    >
      <Card className="mb-3">
        <label className="flex items-center gap-2.5 text-[13px] text-muted-foreground">
          git install
          <input
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            placeholder="https://github.com/addyosmani/agent-skills.git"
            spellCheck={false}
            className="h-8.5 min-w-0 flex-1 rounded-lg border border-input bg-background px-2.5 font-mono text-[13px] text-foreground outline-none focus:border-ring"
          />
        </label>
        <div className="mt-2.5 flex items-center gap-2.5">
          <Button
            size="sm"
            disabled={busy}
            onClick={async () => {
              if (!url.trim()) return;
              setBusy(true);
              note("cloning…");
              try {
                const msg = await invoke<string>("skills_install", { url: url.trim() });
                setUrl("");
                await refreshSkills();
                note(msg);
              } catch (e) {
                note(String(e));
              } finally {
                setBusy(false);
              }
            }}
          >
            <Icon name="download" /> clone &amp; register
          </Button>
          <span className="text-[11.5px] text-muted-foreground">a shallow clone into the global skills dir; every SKILL.md inside is discovered</span>
        </div>
      </Card>
      {skills.list.length === 0 ? (
        <EmptyHint>no skills found: clone a collection above, or drop a folder with SKILL.md into .hi-derola/skills/</EmptyHint>
      ) : (
        <div className="flex flex-col gap-2.5">
          {skills.list.map((sk) => (
            <Card key={sk.name}>
              <div className="flex items-start gap-3">
                <Checkbox
                  checked={sk.enabled}
                  className="mt-1"
                  onCheckedChange={async (c) => {
                    try {
                      await invoke("skills_toggle", { name: sk.name, enabled: !!c });
                      await refreshSkills();
                    } catch (e) {
                      note(String(e));
                    }
                  }}
                />
                <div className="flex min-w-0 flex-1 flex-col gap-0.5">
                  <div className="text-[14px] font-semibold">{sk.name}</div>
                  {sk.description && <div className="text-[12px] text-muted-foreground">{sk.description}</div>}
                  <div className="font-mono text-[11px] text-muted-foreground">{sk.path}</div>
                </div>
              </div>
            </Card>
          ))}
        </div>
      )}
    </PanelShell>
  );
}
