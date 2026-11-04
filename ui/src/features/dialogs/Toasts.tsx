import { useStore } from "@/store/app";
import { useT } from "@/lib/i18n";

// dsh Toast: a dark top-center pill, slides in, auto-fades;
// known keys translate at render, backend errors pass through as-is
export function Toasts() {
  const toasts = useStore((s) => s.toasts);
  const tr = useT();
  if (!toasts.length) return null;
  return (
    <div className="pointer-events-none fixed top-3 left-1/2 z-[80] flex -translate-x-1/2 flex-col items-center gap-1.5">
      {toasts.map((t) => (
        <div
          key={t.id}
          className="msg-in max-w-[70vw] truncate rounded-full bg-[#353638] px-3.5 py-1.5 text-[12.5px] text-white/95 shadow-panel dark:bg-[#43454a]"
          role="alert"
        >
          {tr(t.text)}
        </div>
      ))}
    </div>
  );
}
