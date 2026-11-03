type TauriLike = {
  core: { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> };
  event: { listen: (name: string, cb: (e: { payload: unknown }) => void) => Promise<() => void> };
};

declare global {
  interface Window {
    __TAURI__?: TauriLike;
    __TAURI_INTERNALS__?: unknown;
  }
}

export function invoke<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const t = window.__TAURI__;
  if (!t) return Promise.reject("no backend");
  return t.core.invoke(cmd, args) as Promise<T>;
}

export function listen(cb: (payload: unknown) => void): Promise<() => void> {
  const t = window.__TAURI__;
  if (!t) return Promise.reject("no backend");
  return t.event.listen("ev", (e) => cb(e.payload));
}

export function listenRaw(name: string, cb: (payload: unknown) => void): Promise<() => void> {
  const t = window.__TAURI__;
  if (!t) return Promise.reject("no backend");
  return t.event.listen(name, (e) => cb(e.payload));
}
