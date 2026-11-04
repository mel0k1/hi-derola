// dsh ShortcutReference: searchable keyboard shortcut reference dialog,
// opened from settings or with the shortcuts hotkey (mod+/). Application
// rows carry their editable bindings, message input and shared menu keys
// follow; search filters across labels and key names.
import { useEffect, useRef, useState } from "react";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Icon } from "@/lib/icons";
import { useStore } from "@/store/app";
import { KEY_ACTIONS } from "@/store/keysStore";
import { keyChips } from "@/lib/hint";
import { cn } from "@/lib/utils";
import { useT } from "@/lib/i18n";

// dsh fixedCommands: keys shared by the slash menu, mention popup and palette
// (labels are i18n keys)
const MENU_ROWS: [string, string][] = [
  ["hk.menu_move", "↑ ↓"],
  ["hk.menu_select", "enter tab"],
  ["hk.menu_dismiss", "esc"],
];

// core actions first in product order, dsh coreActionOrder
const APP_ORDER = [
  "shortcuts",
  "new_session",
  "toggle_sidebar",
  "open_settings",
  "toggle_theme",
  "toggle_plan",
  "undo",
  "redo",
  "toggle_thinking",
  "palette",
];

const GROUPS: [string, string | null][] = [
  ["app", "hk.g_app"],
  ["input", "hk.g_input"],
  ["menus", "hk.g_menus"],
];

interface Row {
  id: string;
  label: string;
  group: string;
  pills: string[];
}

// the stop shortcut only nudges on the first press — show the double pill
function bindingPills(action: string, combo?: string): string[] {
  if (action === "stop" && combo === "escape") return ["esc", "esc"];
  const caps = keyChips(combo);
  return caps ? [caps.join(" + ")] : ["none"];
}

export function HotkeysDialog() {
  const open = useStore((s) => s.hotkeysOpen);
  const keys = useStore((s) => s.keys);
  const [query, setQuery] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);
  const t = useT();

  useEffect(() => {
    if (open) {
      setQuery("");
      const id = setTimeout(() => inputRef.current?.focus(), 30);
      return () => clearTimeout(id);
    }
  }, [open]);

  const labels = Object.fromEntries(KEY_ACTIONS.map(([k]) => [k, t("hk." + k)]));
  const rows: Row[] = [];
  for (const id of APP_ORDER) {
    rows.push({ id, label: labels[id] ?? id, group: "app", pills: bindingPills(id, keys[id]) });
  }
  for (const [id] of KEY_ACTIONS) {
    if (id === "send" || id === "newline" || id === "stop") {
      rows.push({ id, label: labels[id] ?? id, group: "input", pills: bindingPills(id, keys[id]) });
    }
  }
  for (const [key, keyText] of MENU_ROWS) {
    rows.push({ id: key, label: t(key), group: "menus", pills: [keyText] });
  }

  const q = query.trim().toLowerCase();
  const visible = rows.filter(
    (r) => !q || r.label.toLowerCase().includes(q) || r.pills.join(" ").toLowerCase().includes(q)
  );
  const bound = rows.filter((r) => r.pills[0] !== "none").length;

  return (
    <Dialog open={open} onOpenChange={(o) => !o && useStore.setState({ hotkeysOpen: false })}>
      <DialogContent
        aria-describedby={undefined}
        showCloseButton={false}
        className="flex h-[600px] max-h-[calc(100dvh-108px)] w-[480px] max-w-[calc(100vw-2rem)] flex-col gap-0 overflow-hidden p-0 sm:max-w-[480px]"
      >
        <div className="relative h-[46px] flex-none pt-[22px] pb-0 pl-6 pr-[54px]">
          <h2 className="m-0 text-[16px] font-medium leading-[24px]">{t("hk.title")}</h2>
          <button
            type="button"
            aria-label={t("a.close")}
            onClick={() => useStore.setState({ hotkeysOpen: false })}
            className="absolute top-5 right-3.5 flex size-7 items-center justify-center rounded-full text-foreground transition-colors duration-100 hover:bg-accent"
          >
            <Icon name="x" className="[&>svg]:size-3.5" />
          </button>
        </div>
        <div className="flex flex-none items-center px-5 pt-3.5 pb-2.5">
          <div className="flex h-9 w-full items-center gap-2 rounded-[12px] border-[0.5px] border-hairline bg-code px-3.5">
            <Icon name="search" className="flex-none text-caption [&>svg]:size-3.5" />
            <input
              ref={inputRef}
              value={query}
              placeholder={t("hk.search")}
              spellCheck={false}
              aria-label={t("hk.search")}
              onChange={(e) => setQuery(e.target.value)}
              className="w-full min-w-0 bg-transparent text-[13px] leading-[18px] outline-none placeholder:text-caption"
            />
            {query !== "" && (
              <button
                type="button"
                aria-label={t("hk.clear_search")}
                onClick={() => {
                  setQuery("");
                  inputRef.current?.focus();
                }}
                className="flex size-4 flex-none items-center justify-center rounded-full border-[0.5px] border-caption text-muted-foreground transition-colors duration-100 hover:bg-accent"
              >
                <Icon name="x" className="[&>svg]:size-2" />
              </button>
            )}
          </div>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto px-4 pb-4.5">
          {GROUPS.map(([group, header]) => {
            const rws = visible.filter((r) => r.group === group);
            if (!rws.length) return null;
            return (
              <section key={group} aria-label={header ?? "application"}>
                {header && <h3 className="mx-2 mt-4 mb-1 text-[12px] font-normal leading-[18px] text-caption">{t(header)}</h3>}
                <div>
                  {rws.map((row) => (
                    <div
                      key={row.id}
                      className="flex min-h-[42px] items-center justify-between gap-2 rounded-[12px] px-2 py-2 text-[13px] leading-[18px] text-foreground"
                    >
                      <span className="min-w-0 truncate">{row.label}</span>
                      <span className="flex flex-none items-center gap-1">
                        {row.pills.map((p, i) => (
                          <span
                            key={i}
                            className={cn(
                              "inline-flex h-[26px] items-center justify-center rounded-[10px] px-3 text-[12px] leading-none whitespace-nowrap",
                              p === "none" ? "text-caption" : "bg-secondary text-muted-foreground"
                            )}
                          >
                            {p}
                          </span>
                        ))}
                      </span>
                    </div>
                  ))}
                </div>
              </section>
            );
          })}
          {!visible.length && (
            <p className="m-1.5 text-[12px] leading-[18px] text-caption" role="status">
              {t("hk.no_match")}
            </p>
          )}
        </div>
        <div className="flex min-h-[52px] flex-none items-center justify-between gap-3 border-t-[0.5px] border-hairline px-6 text-[12px] leading-[18px]">
          <button
            type="button"
            onClick={() => useStore.setState({ hotkeysOpen: false, settingsOpen: true })}
            className="text-caption transition-colors duration-100 hover:text-foreground"
          >
            {t("hk.customize")}
          </button>
          <span className="text-caption">{t("hk.bindings", { n: bound })}</span>
        </div>
      </DialogContent>
    </Dialog>
  );
}
