import { useStore } from "@/store/app";

// dsh Toast: a dark top-center pill, slides in, auto-fades
export function Toasts() {
  const toasts = useStore((s) => s.toasts);
  if (!toasts.length) return null;
  return (
    <div className="pointer-events-none fixed top-3 left-1/2 z-[80] flex -translate-x-1/2 flex-col items-center gap-1.5">
      {toasts.map((t) => (
        <div
          key={t.id}
          className="msg-in max-w-[70vw] truncate rounded-full bg-[#353638] px-3.5 py-1.5 text-[12.5px] text-white/95 shadow-panel dark:bg-[#43454a]"
          role="alert"
        >
          {t.text}
        </div>
      ))}
    </div>
  );
}
