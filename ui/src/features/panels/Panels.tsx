import { useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { Icon } from "@/lib/icons";
import { invoke } from "@/lib/tauri";
import { useStore } from "@/store/app";
import { cn, fmtGiB } from "@/lib/utils";
import { tr, useT } from "@/lib/i18n";
import type { SandboxInfo, HostInfo } from "@/lib/types";

export function PanelShell({ icon, title, sub, action, msg, children }: { icon: string; title: string; sub: ReactNode; action?: ReactNode; msg?: ReactNode; children: ReactNode }) {
  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto max-w-3xl px-5 py-7 pb-12">
        <div className="msg-in mb-5 flex items-start justify-between gap-3">
          <div>
            <div className="flex items-center gap-2.5 text-[19px] font-semibold tracking-tight">
              <Icon name={icon} className="text-tertiary [&>svg]:size-5" />
              {title}
            </div>
            <div className="mt-1 max-w-xl text-[13px] leading-[19px] text-muted-foreground">{sub}</div>
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
  const dot: Record<string, string> = {
    running: "bg-success",
    ready: "bg-success",
    done: "bg-success",
    downloading: "bg-info",
    failed: "bg-destructive",
    waiting: "bg-warning",
    new: "bg-caption/60",
  };
  const label: Record<string, string> = {
    running: "text-success",
    ready: "text-success",
    done: "text-success",
    downloading: "text-info",
    failed: "text-destructive",
    waiting: "text-warning",
  };
  return (
    <span
      className={cn(
        "flex flex-none items-center gap-1.5 font-mono text-[11px] font-medium tracking-wide uppercase",
        label[state] || "text-caption",
        className
      )}
    >
      <span className={cn("size-1.5 rounded-full", dot[state] || "bg-caption/60", state === "running" && "pulse-dot", state === "downloading" && "pulse-dot")} />
      {state}
    </span>
  );
}

export function Card({ children, title, className }: { children: ReactNode; title?: string; className?: string }) {
  return (
    <div title={title} className={cn("msg-in rounded-[16px] border-[0.5px] border-border bg-card p-4 transition-[border-color,box-shadow] duration-150 hover:border-input hover:shadow-sm", className)}>
      {children}
    </div>
  );
}

export function SbxSshBlock({ s }: { s: SandboxInfo }) {
  const info = s.ssh!;
  const ag = info.agent || ({} as NonNullable<NonNullable<SandboxInfo["ssh"]>["agent"]>);
  const t = useT();
  return (
    <div className="mt-2.5 flex flex-col gap-1.5">
      {info.state === "waiting" && (
        <div className="flex flex-wrap items-center gap-1.5 rounded-lg border border-warning/50 bg-warning/5 px-2.5 py-1.5 font-mono text-[12px] text-warning">
          <Icon name="download" />
          ssh waiting… {info.elapsed_secs || 0}s
          <span className="text-[11px] text-muted-foreground">{t("sx.first_boot")}</span>
        </div>
      )}
      {info.state === "failed" && (
        <div
          className="flex flex-wrap items-center gap-1.5 rounded-lg border border-destructive/50 bg-destructive/5 px-2.5 py-1.5 font-mono text-[12px] text-destructive"
          title={t("sx.reboot_hint")}
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
            <span className="text-[11px] text-muted-foreground">· {t("sx.up_in", { n: info.elapsed_secs || 0 })}</span>
          </div>
          <div className="flex items-center gap-2 text-[12px]">
            {ag.state === "unknown" && (
              <span className="text-muted-foreground">
                {t("sx.agent_not_installed")}{s.spec.root ? "" : t("sx.needs_root")}
              </span>
            )}
            {ag.state === "installing" && <span className="animate-pulse font-mono text-[11px] font-semibold text-primary">{t("sx.installing_agent")}</span>}
            {ag.state === "installed" && (
              <span className="flex items-center gap-1.5 text-foreground">
                <Icon name="check" className="text-success" />
                {t("sx.agent_ready", { v: ag.version ? ` v${ag.version}` : "" })}
              </span>
            )}
            {ag.state === "failed" && (
              <span className="flex items-center gap-1.5 text-destructive">
                <Icon name="warn" /> {t("sx.install_failed")}: {ag.error || "error"}
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
  const t = useT();

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
            title={t("sx.bash_here_t")}
          >
            ← {t("sx.bash_here")}
          </span>
        )}
      </div>
      <div className="mt-1.5 flex flex-wrap gap-x-3.5 gap-y-1 font-mono text-[11.5px] text-muted-foreground">
        <span className="text-primary">{s.spec.kind}</span>
        <span>{s.spec.disk_gib} GiB {t("sx.disk")}</span>
        <span>{fmtGiB(s.spec.ram_mib)} {t("sx.ram")}</span>
        <span>{s.spec.cpus} vcpu</span>
        <span>ssh :{s.spec.ssh_port}</span>
        <span>{s.spec.root ? t("sx.root_ok") : t("sx.no_root")}</span>
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
              ? `${t("sx.downloading")} ${(s.download.downloaded / 1048576).toFixed(1)} MiB`
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
              title={t("sx.fwd_del_t")}
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
              const spec = prompt(t("sx.fwd_prompt"), "8080:80");
              if (!spec) return;
              const [ports, ghost] = spec.split(/\s+/);
              const [hp, gp] = (ports || "").split(":").map((x) => parseInt(x, 10));
              if (!hp || !gp) {
                note(t("sx.fwd_need"));
                return;
              }
              run(invoke("sandbox_fwd_add", { id: s.spec.id, hostPort: hp, guestPort: gp, guestHost: ghost || "127.0.0.1" }));
            }}
          >
            + {t("sx.fwd_add")}
          </span>
        </div>
      )}

      <div className="mt-3 flex flex-wrap items-center justify-end gap-1.5">
        {seed && s.state === "running" && s.ssh!.state === "ready" && (
          <>
            <Button variant="ghost" size="sm" title={t("sx.terminal_t")} onClick={() => run(invoke("sandbox_ssh_terminal", { id: s.spec.id, agent: false }))}>
              <Icon name="terminal" /> {t("sx.terminal")}
            </Button>
            {s.ssh!.agent?.state === "installed" && (
              <Button variant="ghost" size="sm" title={t("sx.run_agent_t")} onClick={() => run(invoke("sandbox_ssh_terminal", { id: s.spec.id, agent: true }))}>
                <Icon name="spark" /> {t("sx.run_agent")}
              </Button>
            )}
            <Button
              variant="ghost"
              size="sm"
              disabled={s.ssh!.agent?.state === "installing"}
              title={t("sx.build_agent_t")}
              onClick={() => run(invoke("sandbox_agent_install", { id: s.spec.id }))}
            >
              <Icon name="download" />{" "}
              {s.ssh!.agent?.state === "installing" ? t("sx.installing_agent") : s.ssh!.agent?.state === "installed" ? t("sx.reinstall_agent") : t("sx.install_agent")}
            </Button>
            <Button
              variant="ghost"
              size="sm"
              title={attached === s.spec.id ? t("sx.detach_t") : t("sx.attach_t")}
              onClick={() =>
                run(
                  attached === s.spec.id
                    ? invoke("sandbox_detach")
                    : invoke("sandbox_attach", { id: s.spec.id })
                )
              }
            >
              <Icon name={attached === s.spec.id ? "x" : "box"} /> {attached === s.spec.id ? t("sx.detach") : t("sx.attach")}
            </Button>
            <Button
              variant="ghost"
              size="sm"
              title={t("sx.fetch_t")}
              onClick={() => {
                const vmPath = prompt(t("sx.fetch_prompt", { name: s.spec.name }), "~/README.md");
                if (!vmPath) return;
                note(tr("sx.fetching"));
                invoke<string>("sandbox_fetch_vm", { id: s.spec.id, vmPath })
                  .then((saved) => note(tr("sx.saved_to", { path: saved })))
                  .catch((e) => note(String(e)));
              }}
            >
              <Icon name="download" /> {t("sx.fetch")}
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
          {s.state === "running" ? t("in.stop") : s.state === "downloading" ? t("a.cancel") : s.state === "failed" ? t("sx.retry") : t("sx.start")}
        </Button>
        <Button
          variant="ghost"
          size="icon-sm"
          className={cn("text-muted-foreground hover:text-destructive", confirming && "text-destructive")}
          title={s.state === "downloading" ? t("sx.cancel_delete") : t("sx.delete_sbx")}
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
  const t = useT();

  return (
    <Card>
      <div className="flex flex-wrap items-center gap-2">
        <div className="min-w-0 flex-1 truncate text-[15px] font-semibold">{h.name}</div>
        <StateBadge state={h.state === "ready" ? "ready" : h.state === "failed" ? "failed" : h.state} />
        {attached === h.id && (
          <span className="flex-none rounded-full border border-success/50 px-2.5 py-px font-mono text-[10px] font-semibold tracking-wider text-success uppercase">← {t("sx.bash_here")}</span>
        )}
      </div>
      <div className="mt-1.5 flex flex-wrap gap-x-3.5 gap-y-1 font-mono text-[11.5px] text-muted-foreground">
        <span>{h.label}</span>
        {h.agent === true && <span>{t("hs.agent_installed")}</span>}
      </div>
      {h.error && <div className="mt-2 font-mono text-[11.5px] break-words whitespace-pre-wrap text-destructive">{h.error}</div>}
      <div className="mt-3 flex flex-wrap items-center justify-end gap-1.5">
        <Button
          variant="ghost"
          size="sm"
          title={t("hs.probe_t")}
          onClick={async (e) => {
            const btn = e.currentTarget;
            btn.disabled = true;
            try {
              const st = await invoke<{ state: string; error?: string; name: string }>("host_check", { id: h.id });
              await refreshHosts();
              note(st.state !== "ready" ? tr("hs.check_failed", { err: st.error || "unknown" }) : tr("hs.reachable", { name: st.name }));
            } catch (err) {
              note(String(err));
            } finally {
              btn.disabled = false;
            }
          }}
        >
          <Icon name="check" /> {t("hs.check")}
        </Button>
        <Button
          variant="ghost"
          size="sm"
          title={t("hs.attach_t")}
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
          <Icon name={attached === h.id ? "x" : "box"} /> {attached === h.id ? t("sx.detach") : t("sx.attach")}
        </Button>
        <Button variant="ghost" size="sm" onClick={() => invoke("host_terminal", { id: h.id, agent: false }).catch((e) => note(String(e)))}>
          <Icon name="terminal" /> {t("sx.terminal")}
        </Button>
        <Button
          variant="ghost"
          size="sm"
          title={t("hs.pubkey_t")}
          onClick={async () => {
            try {
              const key = await invoke<string>("host_pubkey", { id: h.id });
              note(tr("hs.add_key", { label: h.label, key }));
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
