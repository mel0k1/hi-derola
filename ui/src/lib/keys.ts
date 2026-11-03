export interface Combo {
  name: string;
  ctrl: boolean;
  shift: boolean;
  alt: boolean;
  meta: boolean;
}

export function comboToText(c: Combo) {
  const parts: string[] = [];
  if (c.ctrl) parts.push("ctrl");
  if (c.shift) parts.push("shift");
  if (c.alt) parts.push("alt");
  if (c.meta) parts.push("meta");
  parts.push(c.name);
  return parts.join("+");
}

export function parseCombo(s?: string | null): Combo | null {
  if (!s || s.trim().toLowerCase() === "none") return null;
  let ctrl = false, shift = false, alt = false, meta = false, name: string | null = null;
  for (const p of s.split("+")) {
    const l = p.trim().toLowerCase();
    if (!l) continue;
    if (l === "ctrl" || l === "control") ctrl = true;
    else if (l === "shift") shift = true;
    else if (l === "alt") alt = true;
    else if (l === "meta" || l === "cmd" || l === "super") meta = true;
    else name = l === "return" ? "enter" : l === "esc" ? "escape" : l === "comma" ? "," : l;
  }
  return name ? { ctrl, shift, alt, meta, name } : null;
}

export function comboFromEvent(e: KeyboardEvent): Combo | null {
  const k = e.key;
  let name: string | null = null;
  if (k === "Enter") name = "enter";
  else if (k === "Escape") name = "escape";
  else if (k === "Tab") name = "tab";
  else if (k === "Backspace") name = "backspace";
  else if (k === " ") name = "space";
  else if (k === "ArrowUp") name = "up";
  else if (k === "ArrowDown") name = "down";
  else if (k === "ArrowLeft") name = "left";
  else if (k === "ArrowRight") name = "right";
  else if (k.length === 1) name = k.toLowerCase();
  else if (/^F\d+$/.test(k)) name = k.toLowerCase();
  else return null;
  return { name, ctrl: e.ctrlKey, shift: e.shiftKey, alt: e.altKey, meta: e.metaKey };
}

export function sameCombo(a: Combo | null, b: Combo | null) {
  return !!a && !!b && a.name === b.name && a.ctrl === b.ctrl && a.shift === b.shift && a.alt === b.alt && a.meta === b.meta;
}
