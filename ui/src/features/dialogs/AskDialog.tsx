import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { useStore } from "@/store/app";
import { cn } from "@/lib/utils";
import { useT } from "@/lib/i18n";
import { useEffect, useRef } from "react";

export function AskDialog() {
  const ask = useStore((s) => s.ask);
  const submitAsk = useStore((s) => s.submitAsk);
  const skipAsk = useStore((s) => s.skipAsk);
  const firstCtl = useRef<HTMLDivElement>(null);
  const t = useT();

  useEffect(() => {
    if (ask?.open) {
      const el = firstCtl.current?.querySelector("input, select");
      if (el) (el as HTMLElement).focus();
    }
  }, [ask?.open]);

  if (!ask) return null;

  const compose = (): string | null => {
    const lines: string[] = [];
    const missing: string[] = [];
    for (let i = 0; i < ask.qs.length; i++) {
      const form = ask.forms[i];
      if (form) {
        for (const name of form.names) {
          const v = form.vals[name];
          const isMissing = form.required.includes(name) && (v === undefined || v === "" || v === null);
          if (isMissing) missing.push(name);
        }
      }
    }
    if (missing.length) {
      const row = document.querySelector(`[data-field-name="${missing[0]}"]`);
      row?.classList.add("ring-2", "ring-destructive");
      setTimeout(() => row?.classList.remove("ring-2", "ring-destructive"), 1500);
      return null;
    }
    for (let i = 0; i < ask.qs.length; i++) {
      const q = ask.qs[i];
      const form = ask.forms[i];
      if (form) {
        const obj: Record<string, unknown> = {};
        for (const name of form.names) {
          const v = form.vals[name];
          if (v === undefined || v === "" || v === null) continue;
          if (Array.isArray(v) && !v.length) continue;
          obj[name] = v;
        }
        if (Object.keys(obj).length) lines.push(JSON.stringify(obj));
        continue;
      }
      const labels = [...ask.sel[i]].map((j) => (q.options || [])[j]?.label).filter(Boolean);
      const customText = (ask.custom[i] || "").trim();
      const ans = customText || labels.join(", ");
      if (!ans) continue;
      const header = (q.header || "").trim();
      lines.push(header ? header + ": " + ans : ans);
    }
    return lines.join("\n");
  };

  const setVal = (qi: number, name: string, v: unknown) => {
    useStore.setState((s) => {
      if (!s.ask) return s;
      const forms = [...s.ask.forms];
      const f = forms[qi];
      if (f) {
        forms[qi] = { ...f, vals: { ...f.vals, [name]: v } };
      }
      return { ask: { ...s.ask, forms } };
    });
  };

  const setCustom = (qi: number, v: string) => {
    useStore.setState((s) => {
      if (!s.ask) return s;
      const custom = [...s.ask.custom];
      custom[qi] = v;
      return { ask: { ...s.ask, custom } };
    });
  };

  const toggleSel = (qi: number, j: number, multiple: boolean) => {
    useStore.setState((s) => {
      if (!s.ask) return s;
      const sel = [...s.ask.sel.map((x) => new Set(x))];
      if (multiple) {
        if (sel[qi].has(j)) sel[qi].delete(j);
        else sel[qi].add(j);
      } else {
        sel[qi] = new Set([j]);
      }
      return { ask: { ...s.ask, sel } };
    });
  };

  return (
    <Dialog open={ask.open}>
      <DialogContent className="max-h-[85vh] w-[min(680px,92vw)] overflow-y-auto" showCloseButton={false} onEscapeKeyDown={(e) => { e.preventDefault(); skipAsk(); }}>
        <DialogHeader>
          <DialogTitle className="text-primary">{t("ak.title")}</DialogTitle>
        </DialogHeader>
        <div ref={firstCtl} className="flex flex-col gap-5">
          {ask.qs.map((q, i) => {
            const form = ask.forms[i];
            const header = (q.header || "").trim();
            return (
              <div key={i} className="flex flex-col gap-2">
                {header && <div className="text-[12.5px] font-semibold text-primary">{header}</div>}
                {q.question && <div className="text-[13.5px]">{q.question}</div>}
                {form ? (
                  <div className="flex flex-col gap-2.5 rounded-xl border border-border bg-secondary/40 p-3">
                    {form.names.map((name) => {
                      const def = form.props?.[name] || {};
                      const desc = (def.description || "").trim();
                      const req = form.required.includes(name);
                      const val = form.vals[name];
                      return (
                        <div key={name} data-field-name={name} className="flex flex-col gap-1 rounded-md">
                          <div className="flex items-baseline gap-2">
                            <span className="font-mono text-[12px] font-medium text-primary">
                              {name}
                              {req && <span className="text-destructive"> *</span>}
                            </span>
                            {desc && <span className="text-[11.5px] text-muted-foreground">{desc}</span>}
                          </div>
                          {Array.isArray(def.enum) && def.enum.length ? (
                            <select
                              className="h-9 rounded-lg border border-input bg-background px-2.5 font-mono text-[12.5px] outline-none focus:border-ring"
                              defaultValue={def.default !== undefined ? String(def.enum.indexOf(def.default)) : ""}
                              onChange={(e) => setVal(i, name, e.target.value === "" ? "" : def.enum![Number(e.target.value)])}
                            >
                              <option value="">{req ? t("ak.pick") : t("ak.none_opt")}</option>
                              {def.enum.map((v, k) => {
                                const nm = Array.isArray(def.enumNames) ? def.enumNames[k] : undefined;
                                return (
                                  <option key={k} value={String(k)}>
                                    {nm !== undefined && nm !== null ? String(nm) : v === null ? "null" : String(v)}
                                  </option>
                                );
                              })}
                            </select>
                          ) : def.type === "array" && Array.isArray(def.items?.enum) && def.items.enum.length ? (
                            <div className="flex flex-wrap gap-x-4 gap-y-1.5">
                              {def.items.enum.map((v, k) => {
                                const nm = Array.isArray(def.items!.enumNames) ? def.items!.enumNames![k] : undefined;
                                const checked = Array.isArray(val) && (val as unknown[]).includes(def.items!.enum![k]);
                                return (
                                  <label key={k} className="flex cursor-pointer items-center gap-1.5 font-mono text-[12px]">
                                    <Checkbox
                                      checked={checked}
                                      onCheckedChange={(c) => {
                                        const cur = new Set(Array.isArray(val) ? (val as unknown[]) : []);
                                        if (c) cur.add(def.items!.enum![k]);
                                        else cur.delete(def.items!.enum![k]);
                                        setVal(i, name, [...cur]);
                                      }}
                                    />
                                    {nm !== undefined && nm !== null ? String(nm) : String(v)}
                                  </label>
                                );
                              })}
                            </div>
                          ) : def.type === "boolean" ? (
                            <label className="flex cursor-pointer items-center gap-2 text-[13px]">
                              <Checkbox checked={!!val} onCheckedChange={(c) => setVal(i, name, !!c)} />
                            </label>
                          ) : def.type === "integer" || def.type === "number" ? (
                            <input
                              type="number"
                              step={def.type === "integer" ? 1 : "any"}
                              min={def.minimum}
                              max={def.maximum}
                              defaultValue={def.default !== undefined ? String(def.default) : ""}
                              placeholder={def.type}
                              className="h-9 rounded-lg border border-input bg-background px-2.5 font-mono text-[12.5px] outline-none focus:border-ring"
                              onInput={(e) => setVal(i, name, e.currentTarget.value === "" ? "" : Number(e.currentTarget.value))}
                            />
                          ) : (
                            <input
                              type="text"
                              maxLength={def.maxLength}
                              placeholder={def.format || t("ak.value")}
                              defaultValue={def.default !== undefined ? String(def.default) : ""}
                              spellCheck={false}
                              className="h-9 rounded-lg border border-input bg-background px-2.5 text-[13px] outline-none focus:border-ring"
                              onInput={(e) => setVal(i, name, e.currentTarget.value)}
                              onKeyDown={(e) => {
                                if (e.key === "Enter") {
                                  e.preventDefault();
                                  submitAsk(compose());
                                }
                              }}
                            />
                          )}
                        </div>
                      );
                    })}
                  </div>
                ) : (
                  <>
                    <div className="flex flex-col gap-1.5">
                      {(q.options || []).map((o, j) => {
                        const sel = ask.sel[i].has(j);
                        return (
                          <button
                            key={j}
                            type="button"
                            onClick={() => toggleSel(i, j, !!q.multiple)}
                            className={cn(
                              "rounded-lg border px-3 py-2 text-left transition-colors",
                              sel ? "border-primary bg-primary/10" : "border-border hover:border-primary/50 hover:bg-accent"
                            )}
                          >
                            <span>{o.label || ""}</span>
                            {o.description && <span className="mt-0.5 block text-[11.5px] text-muted-foreground">{o.description}</span>}
                          </button>
                        );
                      })}
                    </div>
                    <input
                      placeholder={t("ak.own_answer")}
                      spellCheck={false}
                      className="h-9 rounded-lg border border-input bg-background px-2.5 text-[13px] outline-none focus:border-ring"
                      onInput={(e) => setCustom(i, e.currentTarget.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") {
                          e.preventDefault();
                          submitAsk(compose());
                        }
                      }}
                    />
                  </>
                )}
              </div>
            );
          })}
        </div>
        <DialogFooter className="gap-2">
          <Button variant="ghost" size="sm" onClick={skipAsk}>
            {t("ak.skip")} <kbd className="font-mono text-[10px] text-muted-foreground">esc</kbd>
          </Button>
          <Button size="sm" onClick={() => submitAsk(compose())}>
            {t("ak.answer")} <kbd className="font-mono text-[10px] opacity-70">enter</kbd>
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
