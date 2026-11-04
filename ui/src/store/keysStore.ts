export const KEY_ACTIONS: [string, string][] = [
  ["send", "send message"],
  ["newline", "insert newline"],
  ["stop", "stop generation"],
  ["new_session", "new session"],
  ["open_settings", "open settings"],
  ["toggle_sidebar", "toggle sidebar"],
  ["toggle_theme", "toggle theme"],
  ["toggle_plan", "toggle plan mode"],
  ["undo", "undo file changes"],
  ["redo", "redo file changes"],
  ["toggle_thinking", "toggle thinking"],
  ["palette", "command palette"],
  ["shortcuts", "open shortcut reference"],
];

export const DEFAULT_KEYS: Record<string, string> = Object.fromEntries(KEY_ACTIONS.map(([k]) => [k, ""]));
Object.assign(DEFAULT_KEYS, {
  send: "enter",
  newline: "shift+enter",
  stop: "escape",
  new_session: "ctrl+n",
  open_settings: "ctrl+comma",
  toggle_sidebar: "ctrl+b",
  toggle_theme: "ctrl+shift+t",
  toggle_plan: "ctrl+shift+p",
  undo: "ctrl+z",
  redo: "ctrl+shift+z",
  toggle_thinking: "ctrl+t",
  palette: "ctrl+k",
  shortcuts: "ctrl+/",
});

export type EffKeys = Record<string, string>;

export function effectiveKeys(overrides?: Record<string, string>): EffKeys {
  const out = { ...DEFAULT_KEYS };
  for (const [k, v] of Object.entries(overrides || {})) {
    const s = String(v).trim().toLowerCase();
    if (s === "none") delete out[k];
    else if (s) out[k] = s;
  }
  return out;
}
