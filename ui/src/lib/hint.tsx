import type { ReactNode } from "react";
import { Tooltip as TooltipPrimitive } from "radix-ui";
import { cn } from "@/lib/utils";

const NAME_OVERRIDES: Record<string, string> = {
  escape: "esc",
  enter: "enter",
  comma: ",",
  up: "\u2191",
  down: "\u2193",
  left: "\u2190",
  right: "\u2192",
};

// "ctrl+shift+p" -> ["ctrl", "shift", "p"]
export function keyChips(combo?: string | null): string[] | undefined {
  if (!combo || combo.trim().toLowerCase() === "none") return undefined;
  const parts = combo
    .split("+")
    .map((p) => p.trim().toLowerCase())
    .filter(Boolean);
  if (!parts.length) return undefined;
  return parts.map((p) => NAME_OVERRIDES[p] ?? p);
}

export function Kbd({ k }: { k: string }) {
  return (
    <kbd className="inline-flex h-[17px] min-w-[17px] items-center justify-center rounded-[5px] border border-white/15 bg-white/10 px-1 font-sans text-[10px] leading-none font-medium text-zinc-100">
      {k}
    </kbd>
  );
}

// dsh-style tooltip: dark glass surface that stays readable in both themes,
// label plus <kbd> shortcut chips
export function Hint({
  label,
  keys,
  side = "top",
  children,
  className,
}: {
  label: string;
  keys?: string[];
  side?: "top" | "bottom" | "left" | "right";
  children: ReactNode;
  className?: string;
}) {
  return (
    <TooltipPrimitive.Provider delayDuration={450} skipDelayDuration={80}>
      <TooltipPrimitive.Root>
        <TooltipPrimitive.Trigger asChild>{children}</TooltipPrimitive.Trigger>
        <TooltipPrimitive.Portal>
          <TooltipPrimitive.Content
            side={side}
            sideOffset={6}
            className={cn(
              "z-[80] flex max-w-72 items-center gap-1.5 rounded-[10px] border border-white/10 bg-[#1d1e23]/95 px-2.5 py-1.5 text-[11.5px] text-zinc-100 shadow-[0_8px_24px_rgba(0,0,0,0.35)] backdrop-blur-[24px]",
              "animate-in fade-in-0 zoom-in-95 duration-100",
              "data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=closed]:zoom-out-95",
              className,
            )}
          >
            <span className="whitespace-nowrap">{label}</span>
            {keys && keys.length > 0 && (
              <span className="flex items-center gap-0.5">
                {keys.map((k, i) => (
                  <Kbd key={k + i} k={k} />
                ))}
              </span>
            )}
          </TooltipPrimitive.Content>
        </TooltipPrimitive.Portal>
      </TooltipPrimitive.Root>
    </TooltipPrimitive.Provider>
  );
}
