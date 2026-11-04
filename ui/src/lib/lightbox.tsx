import { useEffect, useRef } from "react";
import { create } from "zustand";
import { Icon } from "@/lib/icons";

// dsh ImageLightbox: body-level full-screen viewer for the original image;
// escape / backdrop / button close, focus moves to the close button on open
// and returns to the opener on unmount
interface Box {
  src: string;
  alt: string;
}

export const useLightbox = create<{
  box: Box | null;
  open: (src: string, alt?: string) => void;
  close: () => void;
}>((set) => ({
  box: null,
  open: (src, alt = "") => set({ box: src ? { src, alt } : null }),
  close: () => set({ box: null }),
}));

export function openLightbox(src: string, alt = "") {
  useLightbox.getState().open(src, alt);
}

export function LightboxHost() {
  const box = useLightbox((s) => s.box);
  const close = useLightbox((s) => s.close);
  const btnRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!box) return;
    const prev = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    btnRef.current?.focus();
    // capture phase: the lightbox escape beats every app-level handler
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopPropagation();
      useLightbox.getState().close();
    };
    document.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("keydown", onKey, true);
      prev?.focus();
    };
  }, [box]);

  if (!box) return null;
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={box.alt || "image preview"}
      className="msg-in fixed inset-0 z-[90] flex items-center justify-center bg-black/70 p-8"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) close();
      }}
    >
      <button
        ref={btnRef}
        onClick={close}
        aria-label="close preview"
        className="absolute top-4 right-4 flex size-9 items-center justify-center rounded-full bg-white/10 text-white/85 transition-colors duration-150 hover:bg-white/20 hover:text-white active:scale-95"
      >
        <Icon name="x" className="[&>svg]:size-4" />
      </button>
      <img
        src={box.src}
        alt={box.alt}
        draggable={false}
        className="max-h-[88vh] max-w-[92vw] rounded-[14px] object-contain shadow-[0_12px_48px_rgba(0,0,0,0.5)]"
      />
    </div>
  );
}
