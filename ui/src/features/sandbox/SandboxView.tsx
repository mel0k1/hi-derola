import { Button } from "@/components/ui/button";
import { Icon } from "@/lib/icons";
import { EmptyHint, PanelShell, SbxCard } from "@/features/panels/Panels";
import { SBX_IMAGES, useStore } from "@/store/app";
import { useEffect } from "react";
import { cn } from "@/lib/utils";

function Wizard() {
  const w = useStore((s) => s.sbxWizard);
  const setWizard = useStore((s) => s.setWizard);
  const wizardNext = useStore((s) => s.wizardNext);
  const wizardBack = useStore((s) => s.wizardBack);
  const wizardCreate = useStore((s) => s.wizardCreate);
  if (!w) return null;

  return (
    <div className="msg-in mb-4 rounded-xl border border-border bg-card p-4">
      <div className="mb-4 flex items-center gap-2.5">
        {["image", "resources", "create"].map((label, i) => {
          const step = i + 1;
          return (
            <div key={label} className="flex flex-1 items-center gap-2.5">
              <span className={cn("flex items-center gap-1.5 font-mono text-[12px]", step === w.step ? "text-foreground" : step < w.step ? "text-success" : "text-muted-foreground")}>
                <b
                  className={cn(
                    "inline-flex size-5 items-center justify-center rounded-full border text-[10px] font-semibold",
                    step === w.step && "border-primary text-primary shadow-[0_0_0_3px_var(--ring)]",
                    step < w.step && "border-success text-success",
                    step > w.step && "border-border"
                  )}
                >
                  {step}
                </b>
                {label}
              </span>
              {step < 3 && <span className="h-px flex-1 bg-border" />}
            </div>
          );
        })}
      </div>

      {w.step === 1 && (
        <>
          <div className="grid gap-2.5 sm:grid-cols-3">
            {SBX_IMAGES.map((img) => (
              <button
                key={img.id}
                type="button"
                onClick={() => setWizard({ kind: img.id })}
                className={cn(
                  "flex flex-col gap-1 rounded-lg border p-3 text-left transition-colors",
                  w.kind === img.id ? "border-primary shadow-[0_0_0_3px_var(--ring)]" : "border-border bg-secondary/40 hover:border-primary/50"
                )}
              >
                <span className="text-[13.5px] font-semibold">{img.label}</span>
                <span className="font-mono text-[11px] text-primary">{img.size}</span>
                <span className="text-[11.5px] leading-relaxed text-muted-foreground">{img.hint}</span>
              </button>
            ))}
          </div>
          {w.kind === "custom" && (
            <label className="mt-3 flex flex-col gap-1 text-[12.5px] text-muted-foreground">
              image file path (iso or qcow2)
              <input
                autoFocus
                value={w.iso}
                onChange={(e) => setWizard({ iso: e.target.value })}
                placeholder="C:\\images\\debian-13-amd64.iso"
                spellCheck={false}
                className="h-8.5 rounded-lg border border-input bg-background px-2.5 font-mono text-[13px] text-foreground outline-none focus:border-ring"
              />
            </label>
          )}
        </>
      )}

      {w.step === 2 && (
        <>
          <div className="grid gap-2.5 sm:grid-cols-3">
            {[
              { label: "disk, GiB", key: "disk", min: 5, max: 512 },
              { label: "ram, MiB", key: "ram", min: 256, max: 65536 },
              { label: "cpu cores", key: "cpus", min: 1, max: 32 },
            ].map((f) => (
              <label key={f.key} className="flex flex-col gap-1 text-[12.5px] text-muted-foreground">
                {f.label}
                <input
                  type="number"
                  min={f.min}
                  max={f.max}
                  value={String(w[f.key as "disk" | "ram" | "cpus"])}
                  onChange={(e) => {
                    const n = Number(e.target.value);
                    if (e.target.value !== "" && Number.isFinite(n)) setWizard({ [f.key]: n });
                  }}
                  className="h-8.5 rounded-lg border border-input bg-background px-2.5 font-mono text-[13px] text-foreground outline-none focus:border-ring"
                />
              </label>
            ))}
          </div>
          <div className="mt-2.5 grid gap-2.5 sm:grid-cols-3">
            <label className="flex flex-col gap-1 text-[12.5px] text-muted-foreground">
              ssh port on the host
              <input
                type="number"
                min={1024}
                max={65535}
                value={String(w.port)}
                onChange={(e) => {
                  const n = Number(e.target.value);
                  if (e.target.value !== "" && Number.isFinite(n)) setWizard({ port: n });
                }}
                className="h-8.5 rounded-lg border border-input bg-background px-2.5 font-mono text-[13px] text-foreground outline-none focus:border-ring"
              />
            </label>
            <label className="flex flex-col gap-1 text-[12.5px] text-muted-foreground">
              login inside the VM
              <input
                value={w.login}
                onChange={(e) => setWizard({ login: e.target.value })}
                placeholder="derola"
                spellCheck={false}
                className="h-8.5 rounded-lg border border-input bg-background px-2.5 font-mono text-[13px] text-foreground outline-none focus:border-ring"
              />
            </label>
            <label className="flex flex-col gap-1 text-[12.5px] text-muted-foreground">
              name
              <input
                value={w.name}
                onChange={(e) => setWizard({ name: e.target.value })}
                placeholder="my-sandbox"
                spellCheck={false}
                className="h-8.5 rounded-lg border border-input bg-background px-2.5 font-mono text-[13px] text-foreground outline-none focus:border-ring"
              />
            </label>
          </div>
          <label className="mt-3 flex cursor-pointer items-center gap-2 text-[13px]">
            <input type="checkbox" checked={w.root} onChange={(e) => setWizard({ root: e.target.checked })} className="size-4 accent-[var(--primary)]" />
            allow root inside the sandbox <span className="text-muted-foreground">(passwordless sudo: the in-VM agent needs it to install itself)</span>
          </label>
        </>
      )}

      {w.step === 3 && (
        <>
          <div className="flex flex-col gap-1.5 rounded-lg border border-border bg-secondary/40 p-3.5">
            {[
              ["image", w.kind],
              ...(w.kind === "custom" ? [["file", w.iso]] : []),
              ["disk", `${w.disk} GiB`],
              ["ram", `${(w.ram / 1024).toFixed(w.ram % 1024 ? 1 : 0)} GiB`],
              ["cpu", `${w.cpus} vcpu`],
              ["ssh", `:${w.port}`],
              ["login", w.login.trim() || "derola"],
              ["root", w.root ? "allowed (passwordless sudo)" : "denied"],
            ].map(([k, v]) => (
              <div key={k} className="flex items-center justify-between gap-3 text-[13px]">
                <span className="text-muted-foreground">{k}</span>
                <b className="max-w-[70%] truncate font-semibold">{v}</b>
              </div>
            ))}
          </div>
          <div className="mt-2.5 text-[12px] leading-relaxed text-muted-foreground">
            {["debian-trixie", "debian-trixie-std", "ubuntu-24.04", "ubuntu-24.04-std"].includes(w.kind)
              ? "downloads the official cloud image and generates a cloud-init seed (your login + a generated ssh key); the first boot sets the user up and the card shows when ssh is ready, then you can open a terminal, install the agent inside, or attach the chat to this VM"
              : "boots from your file as-is; the VM opens its own window, guest ssh is forwarded to the host port above"}
          </div>
        </>
      )}

      <div className="mt-4 flex items-center gap-2">
        {w.step > 1 && (
          <Button variant="ghost" size="sm" onClick={wizardBack}>
            back
          </Button>
        )}
        <span className="flex-1" />
        {w.step < 3 ? (
          <Button size="sm" onClick={wizardNext}>
            next
          </Button>
        ) : (
          <Button size="sm" onClick={wizardCreate}>
            <Icon name="download" /> create sandbox
          </Button>
        )}
      </div>
    </div>
  );
}

export function SandboxView() {
  const sbx = useStore((s) => s.sbx);
  const wizard = useStore((s) => s.sbxWizard);
  const refreshSandbox = useStore((s) => s.refreshSandbox);
  const openWizard = useStore((s) => s.openWizard);
  const pollStart = useStore((s) => s.pollStart);
  const pollStop = useStore((s) => s.pollStop);

  useEffect(() => {
    pollStart("sandbox");
    return () => pollStop("sandbox");
  }, [pollStart, pollStop]);

  const q = sbx.qemu;
  return (
    <PanelShell
      icon="box"
      title="sandbox"
      sub="disposable QEMU machines: the agent and the code stay off your host"
      action={
        <Button
          size="sm"
          onClick={() => {
            openWizard();
          }}
        >
          <Icon name="plus" /> new sandbox
        </Button>
      }
      msg={
        q ? (
          <div
            className={cn(
              "mb-3 flex items-center gap-2 rounded-lg border px-3 py-2 font-mono text-[12px]",
              q.system_path && q.img_path ? "border-border text-muted-foreground" : "border-warning/50 bg-warning/5 text-warning"
            )}
          >
            {q.system_path && q.img_path ? (
              <>
                <Icon name="check" className="text-success" />
                qemu {(q.system_version || "qemu found").replace(/^QEMU emulator version\s*/i, "v").trim()} · accel: {q.accel || "tcg"} · qemu-img ok
              </>
            ) : (
              <>
                <Icon name="warn" className="flex-none" />
                <span>
                  QEMU not found: install it (<b>winget install Software.QEMU</b> or <b>choco install qemu</b>, then restart the app) and the sandbox tab comes alive.
                </span>
              </>
            )}
          </div>
        ) : null
      }
    >
      {wizard && <Wizard />}
      {sbx.list.length === 0 && !wizard ? (
        <EmptyHint>
          no sandboxes yet: create one and keep the host clean.
          <br />
          the agent works inside the VM while your files stay untouched.
        </EmptyHint>
      ) : (
        <div className="flex flex-col gap-2.5">
          {sbx.list.map((s) => (
            <SbxCard key={s.spec.id || s.spec.name} s={s} />
          ))}
        </div>
      )}
      <div className="mt-2">
        <Button variant="ghost" size="sm" className="text-muted-foreground" onClick={() => refreshSandbox()}>
          <Icon name="check" /> rescan
        </Button>
      </div>
    </PanelShell>
  );
}
