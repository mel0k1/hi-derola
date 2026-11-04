// dsh ui-context-menu + ui-open-in-app, adapted: one shared popup, explicit
// targets raise it with openContextMenu; pointer position is clamped to the
// viewport and a repeated gesture elsewhere just re-opens.
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Icon } from "@/lib/icons";
import { cn } from "@/lib/utils";
import { tr } from "@/lib/i18n";

export interface MenuItem {
  id: string;
  label: string;
  icon?: string;
  danger?: boolean;
  disabled?: boolean;
  /** draw a separator above this item */
  sep?: boolean;
  onSelect?: () => void;
}

interface MenuState {
  x: number;
  y: number;
  items: MenuItem[];
  flipX: boolean;
  flipY: boolean;
}

type Open = (state: MenuState) => void;
let openFn: Open | null = null;

const EST_W = 210;
const EST_H = 34; // per-item estimate

/** Open the shared context menu at the pointer position. */
export function openContextMenu(e: { clientX: number; clientY: number; preventDefault: () => void; stopPropagation: () => void }, items: MenuItem[]) {
  e.preventDefault();
  e.stopPropagation();
  if (!items.length) return;
  const w = Math.min(EST_W, window.innerWidth - 16);
  const h = items.length * EST_H + 10;
  const x = Math.max(8, Math.min(e.clientX, window.innerWidth - w - 8));
  const y = Math.max(8, Math.min(e.clientY, window.innerHeight - h - 8));
  openFn?.({ x, y, items, flipX: e.clientX > window.innerWidth - w - 8, flipY: e.clientY > window.innerHeight - h - 8 });
}

/** "Show in Explorer" / "Show in Finder" / "Show in Folder" per platform. */
export function fileManagerLabel() {
  const ua = navigator.userAgent;
  if (ua.includes("Mac")) return tr("fm.finder");
  if (ua.includes("Win")) return tr("fm.explorer");
  return tr("fm.folder");
}

export function ContextMenuHost() {
  const [state, setState] = useState<MenuState | null>(null);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    openFn = setState;
    return () => {
      openFn = null;
    };
  }, []);

  useEffect(() => {
    if (!state) return;
    const close = () => setState(null);
    const onPointer = (e: PointerEvent) => {
      if (ref.current && e.target instanceof Node && ref.current.contains(e.target)) return;
      close();
    };
    // capture: escape and scrolls close the menu before anything else reacts
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        close();
      }
    };
    window.addEventListener("pointerdown", onPointer, true);
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("scroll", close, true);
    window.addEventListener("blur", close);
    window.addEventListener("resize", close);
    return () => {
      window.removeEventListener("pointerdown", onPointer, true);
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("scroll", close, true);
      window.removeEventListener("blur", close);
      window.removeEventListener("resize", close);
    };
  }, [state]);

  if (!state) return null;
  return createPortal(
    <div
      ref={ref}
      role="menu"
      className={cn(
        "animate-in fade-in zoom-in-[0.98] fixed z-[90] min-w-[190px] max-w-[300px] rounded-[12px] border-[0.5px] border-hairline bg-popover p-[5px] shadow-panel duration-100",
        state.flipX ? "origin-top-right" : "origin-top-left",
      )}
      style={{ left: state.x, top: state.y }}
      onContextMenu={(e) => e.preventDefault()}
    >
      {state.items.map((item) => (
        <div key={item.id}>
          {item.sep && <div className="my-1 h-px bg-border" />}
          <button
            type="button"
            role="menuitem"
            disabled={item.disabled}
            className={cn(
              "flex h-[30px] w-full items-center gap-2 rounded-[8px] px-2.5 text-left text-[13px] transition-colors duration-100",
              item.danger ? "text-destructive hover:bg-destructive/10" : "text-foreground hover:bg-accent",
              item.disabled && "pointer-events-none opacity-40",
            )}
            onClick={() => {
              setState(null);
              item.onSelect?.();
            }}
          >
            {item.icon && <Icon name={item.icon} className="flex-none text-caption [&>svg]:size-3.5" />}
            <span className="truncate">{item.label}</span>
          </button>
        </div>
      ))}
    </div>,
    document.body,
  );
}
