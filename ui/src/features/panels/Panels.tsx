import { useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { Icon } from "@/lib/icons";
import { invoke } from "@/lib/tauri";
import { useStore } from "@/store/app";
import { cn, fmtGiB } from "@/lib/utils";
import type { SandboxInfo, HostInfo } from "@/lib/types";

export function PanelShell({ icon, title, sub, action, msg, children }: { icon: string; title: string; sub: ReactNode; action?: ReactNode; msg?: ReactNode; children: ReactNode }) {
  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto max-w-3xl px-5 py-6 pb-10">
        <div className="msg-in mb-4 flex items-start justify-between gap-3">
          <div>
            <div className="flex items-center gap-2.5 text-[21px] font-semibold tracking-tight">
              <Icon name={icon} className="text-primary [&>svg]:size-5" />
              {title}
            </div>
            <div className="mt-1 max-w-xl text-[13px] text-muted-foreground">{sub}</div>
          </div>
          {action}
        </div>
        {msg}
        {children}
      </div>
    </div>
  );
}

export function EmptyHint({ children }: { children: ReactNode }) {
  return <div className="msg-in mx-auto mt-6 max-w-md px-4 py-6 text-center text-[13px] leading-relaxed text-muted-foreground">{children}</div>;
}

export function StateBadge({ state, className }: { state: string; className?: string }) {
  const map: Record<string, string> = {
    running: "border-success/50 text-success",
    ready: "border-success/50 text-success",
    done: "border-success/50 text-success",
    downloading: "border-primary/50 text-primary",
    failed: "border-destructive/50 text-destructive",
    waiting: "border-warning/50 text-warning",
    new: "border-border text-muted-foreground",
  };
  return (
    <span
      className={cn(
        "flex-none rounded-full border px-2.5 py-px font-mono text-[10px] font-semibold tracking-wider uppercase",
        map[state] || "border-border text-muted-foreground",
        className
      )}
    >
      {state}
    </span>
  );
}

export function Card({ children, title, className }: { children: ReactNode; title?: string; className?: string }) {
  return (
    <div title={title} className={cn("msg-in rounded-xl border border-border bg-card p-3.5 transition-colors hover:border-primary/40", className)}>
      {children}
    </div>
  );
}

export function SbxSshBlock({ s }: { s: SandboxInfo }) {
  const info = s.ssh!;
  const ag = info.agent || ({} as NonNullable<NonNullable<SandboxInfo["ssh"]>["agent"]>);
  return (
    <div className="mt-2.5 flex flex-col gap-1.5">
      {info.state === "waiting" && (
        <div className="flex flex-wrap items-center gap-1.5 rounded-lg border border-warning/50 bg-warning/5 px-2.5 py-1.5 font-mono text-[12px] text-warning">
          <Icon name="download" />
          ssh waiting… {info.elapsed_secs || 0}s
          <span className="text-[11px] text-muted-foreground">(first boot applies the cloud-init seed)</span>
        </div>
      )}
      {info.state === "failed" && (
        <div
          className="flex flex-wrap items-center gap-1.5 rounded-lg border border-destructive/50 bg-destructive/5 px-2.5 py-1.5 font-mono text-[12px] text-destructive"
          title="reboot the VM (stop / start) or check qemu.log"
        >
          <Icon name="warn" />
          ssh failed: {info.error || "unknown error"}
        </div>
      )}
      {info.state === "ready" && (
        <>
          <div className="flex flex-wrap items-center gap-1.5 rounded-lg border border-success/40 bg-success/5 px-2.5 py-1.5 font-mono text-[12px] text-success">
            <Icon name="check" />
            ssh ready · {info.user}@127.0.0.1:{info.port}
            <span className="text-[11px] text-muted-foreground">· up in {info.elapsed_secs || 0}s</span>
          </div>
          <div className="flex items-center gap-2 text-[12px]">
            {ag.state === "unknown" && (
              <span className="text-muted-foreground">
                the agent is not installed in this VM yet{s.spec.root ? "" : ' (installing needs root: enable "allow root" at creation'}
              </span>
            )}
            {ag.state === "installing" && <span className="animate-pulse font-mono text-[11px] font-semibold text-primary">installing agent…</span>}
            {ag.state === "installed" && (
              <span className="flex items-center gap-1.5 text-foreground">
                <Icon name="check" className="text-success" />
                agent{ag.version ? ` v${ag.version}` : ""} ready inside the VM
              </span>
            )}
            {ag.state === "failed" && (
              <span className="flex items-center gap-1.5 text-destructive">
                <Icon name="warn" /> install failed: {ag.error || "error"}
              </span>
            )}
          </div>
          {ag.log && ag.log.length > 0 && (
            <pre className="max-h-36 overflow-y-auto rounded-lg border border-border bg-secondary/50 px-2.5 py-2 font-mono text-[11px] break-words whitespace-pre-wrap text-muted-foreground">
              {(ag.log.length > 14 ? `… ${ag.log.length - 14} earlier lines\n` : "") + ag.log.slice(-14).join("\n")}
            </pre>
          )}
        </>
      )}
    </div>
  );
}

export function SbxCard({ s }: { s: SandboxInfo }) {
  const refreshSandbox = useStore((s) => s.refreshSandbox);
  const note = useStore((s) => s.note);
  const attached = useStore((st) => st.sbx.attached);
  const [confirming, setConfirming] = useState(false);

  const seed = !!s.ssh;
  const pct = s.download && s.download.total ? Math.min(100, Math.round((s.download.downloaded * 100) / s.download.total)) : null;

  const run = (p: Promise<unknown>) => p.then(() => refreshSandbox()).catch((e) => note(String(e)));

  return (
    <Card title={s.spec.name}>
      <div className="flex flex-wrap items-center gap-2">
        <div className="min-w-0 flex-1 truncate text-[15px] font-semibold">{s.spec.name}</div>
        <StateBadge state={s.state} />
        {attached === s.spec.id && (
          <span
            className="flex-none rounded-full border border-success/50 px-2.5 py-px font-mono text-[10px] font-semibold tracking-wider text-success uppercase"
            title="the chat's bash and file tools execute inside this VM"
          >
            ← bash here
          </span>
        )}
      </div>
      <div className="mt-1.5 flex flex-wrap gap-x-3.5 gap-y-1 font-mono text-[11.5px] text-muted-foreground">
        <span className="text-primary">{s.spec.kind}</span>
        <span>{s.spec.disk_gib} GiB disk</span>
        <span>{fmtGiB(s.spec.ram_mib)} ram</span>
        <span>{s.spec.cpus} vcpu</span>
        <span>ssh :{s.spec.ssh_port}</span>
        <span>{s.spec.root ? "root allowed" : "no root"}</span>
      </div>

      {s.state === "downloading" && s.download && (
        <>
          <div className="mt-2.5 h-1.5 overflow-hidden rounded bg-secondary">
            <div
              className={cn("h-full rounded bg-primary transition-[width] duration-500", pct === null && "w-2/5 animate-indet")}
              style={pct !== null ? { width: pct + "%" } : undefined}
            />
          </div>
          <div className="mt-1 font-mono text-[11px] text-muted-foreground">
            {pct === null
              ? `downloading… ${(s.download.downloaded / 1048576).toFixed(1)} MiB`
              : `${pct}% · ${(s.download.downloaded / 1048576).toFixed(1)} / ${(s.download.total / 1048576).toFixed(1)} MiB`}
          </div>
        </>
      )}

      {seed && s.state === "running" && <SbxSshBlock s={s} />}
      {s.error && <div className="mt-2 font-mono text-[11.5px] break-words whitespace-pre-wrap text-destructive">{s.error}</div>}

      {s.spec.forwards && s.spec.forwards.length > 0 && (
        <div className="mt-2 flex flex-wrap gap-1.5">
          {s.spec.forwards.map((f) => (
            <span
              key={f.host_port}
              title="extra port forward (applies on VM start), click to remove"
              className="cursor-pointer rounded-md border border-border bg-secondary/60 px-2 py-0.5 font-mono text-[11.5px] text-primary hover:border-destructive/50 hover:text-destructive"
              onClick={() => run(invoke("sandbox_fwd_del", { id: s.spec.id, hostPort: f.host_port }))}
            >
              127.0.0.1:{f.host_port} → {f.guest_host}:{f.guest_port}
            </span>
          ))}
        </div>
      )}
      {s.state === "running" && (
        <div className="mt-2">
          <span
            className="cursor-pointer rounded-md border border-dashed border-border px-2 py-0.5 font-mono text-[11.5px] text-muted-foreground hover:border-primary/50 hover:text-primary"
            onClick={() => {
              const spec = prompt("port forward as host:guest [guest-host]", "8080:80");
              if (!spec) return;
              const [ports, ghost] = spec.split(/\s+/);
              const [hp, gp] = (ports || "").split(":").map((x) => parseInt(x, 10));
              if (!hp || !gp) {
                note("need host:guest ports, e.g. 8080:80");
                return;
              }
              run(invoke("sandbox_fwd_add", { id: s.spec.id, hostPort: hp, guestPort: gp, guestHost: ghost || "127.0.0.1" }));
            }}
          >
            + port forward
          </span>
        </div>
      )}

      <div className="mt-3 flex flex-wrap items-center justify-end gap-1.5">
        {seed && s.state === "running" && s.ssh!.state === "ready" && (
          <>
            <Button variant="ghost" size="sm" title="open a shell in the VM (a new terminal window)" onClick={() => run(invoke("sandbox_ssh_terminal", { id: s.spec.id, agent: false }))}>
              <Icon name="terminal" /> terminal
            </Button>
            {s.ssh!.agent?.state === "installed" && (
              <Button variant="ghost" size="sm" title="open the agent TUI inside the VM (a new terminal window)" onClick={() => run(invoke("sandbox_ssh_terminal", { id: s.spec.id, agent: true }))}>
                <Icon name="spark" /> run agent
              </Button>
            )}
            <Button
              variant="ghost"
              size="sm"
              disabled={s.ssh!.agent?.state === "installing"}
              title="build the agent inside the VM (apt tools + rustup + cargo install; needs root)"
              onClick={() => run(invoke("sandbox_agent_install", { id: s.spec.id }))}
            >
              <Icon name="download" />{" "}
              {s.ssh!.agent?.state === "installing" ? "installing…" : s.ssh!.agent?.state === "installed" ? "reinstall agent" : "install agent"}
            </Button>
            <Button
              variant="ghost"
              size="sm"
              title={attached === s.spec.id ? "stop routing the chat's bash + file tools into this VM" : "run the chat's bash + file tools inside this VM over ssh"}
              onClick={() =>
                run(
                  attached === s.spec.id
                    ? invoke("sandbox_detach")
                    : invoke("sandbox_attach", { id: s.spec.id })
                )
              }
            >
              <Icon name={attached === s.spec.id ? "x" : "box"} /> {attached === s.spec.id ? "detach chat" : "attach chat"}
            </Button>
            <Button
              variant="ghost"
              size="sm"
              title="copy a file out of the VM via a save dialog"
              onClick={() => {
                const vmPath = prompt(`file path inside the VM ${s.spec.name}:`, "~/README.md");
                if (!vmPath) return;
                note("fetching…");
                invoke("sandbox_fetch_vm", { id: s.spec.id, vmPath })
                  .then((saved) => note(`saved: ${saved}`))
                  .catch((e) => note(String(e)));
              }}
            >
              <Icon name="download" /> fetch file
            </Button>
          </>
        )}
        <Button
          variant="ghost"
          size="sm"
          onClick={async (e) => {
            const btn = e.currentTarget;
            btn.disabled = true;
            const action = s.state === "running" ? "stop" : s.state === "downloading" ? "delete" : "start";
            try {
              await invoke("sandbox_action", { id: s.spec.id, action });
              await refreshSandbox();
            } catch (err) {
              note(String(err));
              btn.disabled = false;
            }
          }}
        >
          <Icon name={s.state === "running" ? "stop" : s.state === "downloading" ? "x" : "play"} />
          {s.state === "running" ? "stop" : s.state === "downloading" ? "cancel" : s.state === "failed" ? "retry" : "start"}
        </Button>
        <Button
          variant="ghost"
          size="icon-sm"
          className={cn("text-muted-foreground hover:text-destructive", confirming && "text-destructive")}
          title={s.state === "downloading" ? "cancel and delete" : "delete sandbox (two clicks)"}
          onClick={() => {
            if (!confirming) {
              setConfirming(true);
              setTimeout(() => setConfirming(false), 2500);
              return;
            }
            run(invoke("sandbox_action", { id: s.spec.id, action: "delete" }));
          }}
        >
          <Icon name={confirming ? "check" : "trash"} />
        </Button>
      </div>
    </Card>
  );
}

export function HostCard({ h }: { h: HostInfo }) {
  const refreshHosts = useStore((s) => s.refreshHosts);
  const note = useStore((s) => s.note);
  const attached = useStore((st) => st.hosts.attached);

  return (
    <Card>
      <div className="flex flex-wrap items-center gap-2">
        <div className="min-w-0 flex-1 truncate text-[15px] font-semibold">{h.name}</div>
        <StateBadge state={h.state === "ready" ? "ready" : h.state === "failed" ? "failed" : h.state} />
        {attached === h.id && (
          <span className="flex-none rounded-full border border-success/50 px-2.5 py-px font-mono text-[10px] font-semibold tracking-wider text-success uppercase">← bash here</span>
        )}
      </div>
      <div className="mt-1.5 flex flex-wrap gap-x-3.5 gap-y-1 font-mono text-[11.5px] text-muted-foreground">
        <span>{h.label}</span>
        {h.agent === true && <span>agent installed</span>}
      </div>
      {h.error && <div className="mt-2 font-mono text-[11.5px] break-words whitespace-pre-wrap text-destructive">{h.error}</div>}
      <div className="mt-3 flex flex-wrap items-center justify-end gap-1.5">
        <Button
          variant="ghost"
          size="sm"
          title="probe ssh connectivity"
          onClick={async (e) => {
            const btn = e.currentTarget;
            btn.disabled = true;
            try {
              const st = await invoke<{ state: string; error?: string; name: string }>("host_check", { id: h.id });
              await refreshHosts();
              note(st.state !== "ready" ? `check failed: ${st.error || "unknown"}` : `host "${st.name}" is reachable`);
            } catch (err) {
              note(String(err));
            } finally {
              btn.disabled = false;
            }
          }}
        >
          <Icon name="check" /> check
        </Button>
        <Button
          variant="ghost"
          size="sm"
          title="run the chat's bash + file tools on this host over ssh"
          onClick={() =>
            attached === h.id
              ? invoke("host_detach").then(() => refreshHosts()).catch((e) => note(String(e)))
              : invoke("host_attach", { id: h.id })
                  .then((msg) => {
                    note(String(msg));
                    return refreshHosts();
                  })
                  .catch((e) => note(String(e)))
          }
        >
          <Icon name={attached === h.id ? "x" : "box"} /> {attached === h.id ? "detach chat" : "attach chat"}
        </Button>
        <Button variant="ghost" size="sm" onClick={() => invoke("host_terminal", { id: h.id, agent: false }).catch((e) => note(String(e)))}>
          <Icon name="terminal" /> terminal
        </Button>
        <Button
          variant="ghost"
          size="sm"
          title="show the public key to install on the host"
          onClick={async () => {
            try {
              const key = await invoke<string>("host_pubkey", { id: h.id });
              note(`add to ${h.label} ~/.ssh/authorized_keys:\n${key}`);
            } catch (e) {
              note(String(e));
            }
          }}
        >
          <Icon name="file" /> pubkey
        </Button>
        <Button
          variant="ghost"
          size="icon-sm"
          className="text-muted-foreground hover:text-destructive"
          onClick={() => invoke("host_del", { id: h.id }).then(() => refreshHosts()).catch((e) => note(String(e)))}
        >
          <Icon name="trash" />
        </Button>
      </div>
    </Card>
  );
}
