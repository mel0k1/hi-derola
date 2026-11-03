import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

export function fmtTokens(n: number) {
  return n < 1000 ? String(n) : (n / 1000).toFixed(1) + "k";
}

export function fmtCost(c: number) {
  return "$" + (c >= 1 ? c.toFixed(2) : c.toFixed(4));
}

export function fmtRel(ts: number) {
  if (!ts) return "";
  const d = Date.now() / 1000 - ts;
  if (d < 60) return "now";
  if (d < 3600) return Math.floor(d / 60) + "m";
  if (d < 86400) return Math.floor(d / 3600) + "h";
  if (d < 7 * 86400) return Math.floor(d / 86400) + "d";
  const dt = new Date(ts * 1000);
  return dt.toISOString().slice(0, 10);
}

export function fmtGiB(mib: number) {
  const g = mib / 1024;
  return (Number.isInteger(g) ? g : g.toFixed(1)) + " GiB";
}

export function chipName(p: string) {
  const base = p.replace(/\s*\(\d+ entries?\)\s*$/, "").replace(/[\\/]+$/, "");
  return base.split(/[\\/]/).pop() || p;
}

export function joinPath(dir: string, name: string) {
  if (dir === "~") dir = "~/";
  return dir.endsWith("/") || dir.endsWith("\\") ? dir + name : dir + "/" + name;
}

export function parseKVLines(text: string, sep: string) {
  const out: Record<string, string> = {};
  for (const line of (text || "").split("\n")) {
    const t = line.trim();
    if (!t) continue;
    const i = t.indexOf(sep);
    if (i <= 0) continue;
    out[t.slice(0, i).trim()] = t.slice(i + 1).trim();
  }
  return out;
}
