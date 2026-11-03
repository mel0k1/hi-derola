import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Icon } from "@/lib/icons";
import { Card, EmptyHint, HostCard, PanelShell } from "@/features/panels/Panels";
import { useStore } from "@/store/app";
import { invoke } from "@/lib/tauri";

export function HostsView() {
  const hosts = useStore((s) => s.hosts);
  const refreshHosts = useStore((s) => s.refreshHosts);
  const note = useStore((s) => s.note);
  const [formOpen, setFormOpen] = useState(false);
  const [name, setName] = useState("");
  const [user, setUser] = useState("");
  const [host, setHost] = useState("");
  const [port, setPort] = useState("22");

  useEffect(() => {
    refreshHosts();
  }, [refreshHosts]);

  const save = async () => {
    try {
      const st = await invoke<{ id: string; name: string }>("host_add", {
        name: name.trim(),
        user: user.trim() || "root",
        host: host.trim(),
        port: parseInt(port, 10) || 22,
      });
      setFormOpen(false);
      await refreshHosts();
      const key = await invoke<string>("host_pubkey", { id: st.id });
      note(`host "${st.name}" saved: install this key on the host, then check:\n${key}`);
    } catch (e) {
      note(String(e));
    }
  };

  return (
    <PanelShell
      icon="server"
      title="remote hosts"
      sub="your servers over ssh: attach one and the agent works there (bash, files, undo/redo, background tasks)"
      action={
        <Button size="sm" onClick={() => setFormOpen(!formOpen)}>
          <Icon name="plus" /> add host
        </Button>
      }
    >
      {formOpen && (
        <Card className="mb-3">
          <div className="grid gap-2.5 sm:grid-cols-2">
            {[
              { label: "name", value: name, set: setName, ph: "web-prod" },
              { label: "user", value: user, set: setUser, ph: "root" },
              { label: "host", value: host, set: setHost, ph: "203.0.113.10 or my.server.dev" },
              { label: "port", value: port, set: setPort, ph: "22", num: true },
            ].map((f) => (
              <label key={f.label} className="flex items-center gap-2.5 text-[13px] text-muted-foreground">
                {f.label}
                <input
                  value={f.value}
                  onChange={(e) => f.set(e.target.value)}
                  placeholder={f.ph}
                  type={f.num ? "number" : "text"}
                  min={f.num ? 1 : undefined}
                  max={f.num ? 65535 : undefined}
                  spellCheck={false}
                  className="h-8.5 min-w-0 flex-1 rounded-lg border border-input bg-background px-2.5 font-mono text-[13px] text-foreground outline-none focus:border-ring"
                />
              </label>
            ))}
          </div>
          <div className="mt-3 flex items-center gap-2">
            <Button size="sm" onClick={save}>
              save &amp; generate key
            </Button>
            <Button variant="ghost" size="sm" onClick={() => setFormOpen(false)}>
              cancel
            </Button>
          </div>
          <div className="mt-2 text-[11.5px] text-muted-foreground">a key pair is generated per host: add the shown public key to the host's authorized_keys, then run check</div>
        </Card>
      )}
      {hosts.list.length === 0 ? (
        <EmptyHint>no remote hosts yet: add one and attach it to move the agent's bash and file tools to your server.</EmptyHint>
      ) : (
        <div className="flex flex-col gap-2.5">
          {hosts.list.map((h) => (
            <HostCard key={h.id} h={h} />
          ))}
        </div>
      )}
    </PanelShell>
  );
}
