// dsh ui-secret-mask: recognize published credential formats in rendered text,
// draw them masked with an inline reveal control. Masking is display-only —
// the original value stays in the message and is what the model receives.
import { iconSvg } from "./icons";

export function esc(s: string) {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

// character the mask keeps readable: provider prefix + little else
const VISIBLE = 8;

type Shape = "run" | "sk" | "exact" | "bounded" | "jwt";
interface Format {
  literal: string;
  shape: Shape;
  min: number;
  max: number;
}

// dsh credential-formats: every literal occurs inside no other literal;
// min/max apply to the run after the literal
const FORMATS: Format[] = [
  // model services: sk- keys carry a digit, which hyphenated identifiers rarely do
  { literal: "sk-", shape: "sk", min: 20, max: 0 },
  { literal: "AIza", shape: "exact", min: 35, max: 35 },
  // code hosting and package registries
  { literal: "ghp_", shape: "run", min: 36, max: 0 },
  { literal: "gho_", shape: "run", min: 36, max: 0 },
  { literal: "ghu_", shape: "run", min: 36, max: 0 },
  { literal: "ghs_", shape: "run", min: 36, max: 0 },
  { literal: "ghr_", shape: "run", min: 36, max: 0 },
  { literal: "github_pat_", shape: "run", min: 22, max: 0 },
  { literal: "glpat-", shape: "run", min: 20, max: 0 },
  { literal: "npm_", shape: "run", min: 36, max: 0 },
  { literal: "hf_", shape: "run", min: 30, max: 0 },
  // cloud access key ids
  { literal: "AKIA", shape: "bounded", min: 16, max: 16 },
  { literal: "ASIA", shape: "bounded", min: 16, max: 16 },
  { literal: "LTAI", shape: "bounded", min: 12, max: 20 },
  { literal: "AKID", shape: "bounded", min: 28, max: 36 },
  // collaboration and payment services
  { literal: "xoxa-", shape: "run", min: 10, max: 0 },
  { literal: "xoxb-", shape: "run", min: 10, max: 0 },
  { literal: "xoxp-", shape: "run", min: 10, max: 0 },
  { literal: "xoxr-", shape: "run", min: 10, max: 0 },
  { literal: "xoxs-", shape: "run", min: 10, max: 0 },
  { literal: "sk_live_", shape: "run", min: 16, max: 0 },
  { literal: "sk_test_", shape: "run", min: 16, max: 0 },
  { literal: "rk_live_", shape: "run", min: 16, max: 0 },
  { literal: "rk_test_", shape: "run", min: 16, max: 0 },
  { literal: "whsec_", shape: "run", min: 24, max: 0 },
  // json web tokens: three base64url runs joined by .eyJ and .
  { literal: "eyJ", shape: "jwt", min: 8, max: 2048 },
];

const LITERALS = FORMATS.map((f) => f.literal).sort((a, b) => b.length - a.length);
// word boundary before the literal so "task-…" never reads as "sk-…"
const PREFIX_RE = new RegExp(`(?<![\\w/-])(${LITERALS.map(esc).join("|")})`).source;
const RUN = "[A-Za-z0-9_-]*";
const PREFIX_RUN_RE = new RegExp(PREFIX_RE + `(${RUN})`, "g");
// the password of an address that carries credentials; scheme and user stay readable
const ADDRESS_RE = /(?<![\w/-])[a-z][a-z0-9+.-]*:\/\/[^\s/:@?#]+:([^\s/@?#]+)(@)/g;
// a private key block runs from its BEGIN line to its END line
export const KEYBLOCK_RE = /-----BEGIN [A-Z0-9 ]+-----[\s\S]*?-----END [A-Z0-9 ]*-----?/g;

export interface SecretHit {
  start: number;
  end: number;
  masked: string;
}

export function maskedFor(value: string, visible: number, from = 0): string {
  const hidden = Math.max(0, value.length - visible);
  if (hidden <= 0) return "";
  return value.slice(from, from + visible) + "*".repeat(hidden);
}

/** One pass over a plain (already escaped) segment: prefixed formats, jwt, address passwords. */
export function scanSecrets(text: string): SecretHit[] {
  const hits: SecretHit[] = [];
  for (const m of text.matchAll(PREFIX_RUN_RE)) {
    const literal = m[1];
    const run = m[2];
    const fmt = FORMATS.find((f) => f.literal === literal);
    if (!fmt) continue;
    let end = (m.index ?? 0) + literal.length + run.length;
    if (fmt.shape === "jwt") {
      if (run.length < fmt.min || run.length > fmt.max) continue;
      // the greedy run stops at the first dot; the token continues .eyJ<run>.<run>
      const rest = text.slice(end);
      const j = rest.match(/^\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+/);
      if (!j) continue;
      end += j[0].length;
    } else if (fmt.shape === "exact") {
      if (run.length !== fmt.min) continue;
    } else if (fmt.shape === "bounded") {
      if (run.length < fmt.min || run.length > fmt.max) continue;
    } else if (fmt.shape === "sk") {
      if (run.length < fmt.min || !/\d/.test(run.slice(0, 256))) continue;
    } else {
      if (run.length < fmt.min) continue;
    }
    const masked = maskedFor(text.slice(m.index ?? 0, end), VISIBLE);
    if (masked) hits.push({ start: m.index ?? 0, end, masked });
  }
  for (const m of text.matchAll(ADDRESS_RE)) {
    const pw = m[1];
    if (!pw) continue;
    const start = (m.index ?? 0) + m[0].indexOf("://") + 3;
    const user = text.slice(start).split(":")[0];
    const pwStart = start + user.length + 1;
    hits.push({ start: pwStart, end: pwStart + pw.length, masked: "*".repeat(pw.length) });
  }
  hits.sort((a, b) => a.start - b.start);
  return hits.filter((h, i) => i === 0 || h.start >= hits[i - 1].end);
}

/** dsh SecretText mask runs: asterisks keep their advance but paint a star tile. */
function starHtml(masked: string, br: boolean): string {
  return masked
    .split(/(\*+)/)
    .map((part) => {
      if (!part) return "";
      if (part[0] === "*") return `<span class="md-secret-star">${part}</span>`;
      return esc(part).replace(/\n/g, br ? "<br>" : "\n");
    })
    .join("");
}

function secretHtml(raw: string, masked: string, br: boolean): string {
  const stars = starHtml(masked, br);
  return (
    `<span class="md-secret" data-raw="${raw}" data-masked="${masked}" data-masked-html="${esc(stars)}">` +
    `<span class="md-secret-value">${stars}</span>` +
    `<button type="button" class="md-eye" aria-pressed="false" aria-label="reveal value" title="reveal value">` +
    `<span class="md-eye-i md-eye-reveal">${iconSvg("eye")}</span>` +
    `<span class="md-eye-i md-eye-hide">${iconSvg("eyeoff")}</span>` +
    `</button></span>`
  );
}

function maskText(text: string, br: boolean): string {
  let out = "";
  let last = 0;
  for (const hit of scanSecrets(text)) {
    out += text.slice(last, hit.start);
    out += secretHtml(text.slice(hit.start, hit.end), hit.masked, br);
    last = hit.end;
  }
  return out + text.slice(last);
}

/**
 * Tag-aware pass over rendered html: substitutes \x02N\x02 key-block placeholders,
 * then masks text between tags, skipping inline <code> spans.
 */
export function maskSegments(html: string, blocks: string[], br: boolean): string {
  if (blocks.length) html = html.replace(/\x02(\d+)\x02/g, (_m, i) => blocks[+i] ?? "");
  if (!/sk-|AIza|gh[pousr]_|github_pat_|glpat-|npm_|hf_|AKIA|ASIA|LTAI|AKID|xox[abprs]-|_live_|_test_|whsec_|eyJ|:\/\/|\x02/.test(html)) return html;
  let out = "";
  let code = 0;
  for (const part of html.split(/(<[^>]*>)/)) {
    if (part.startsWith("<")) {
      if (/^<code[\s>]/.test(part)) code++;
      else if (/^<\/code>/.test(part)) code = Math.max(0, code - 1);
      out += part;
      continue;
    }
    out += code > 0 || !part ? part : maskText(part, br);
  }
  return out;
}

/** Masked html for plain text (user bubbles): escaped, key blocks + inline formats. */
export function maskUserText(text: string): string {
  const blocks: string[] = [];
  const src = text.replace(KEYBLOCK_RE, (m) => {
    const lines = m.split("\n");
    const body = lines
      .slice(1, -1)
      .map((l) => (l ? "*".repeat(l.length) : l))
      .join("\n");
    const masked = lines.length > 2 ? lines[0] + "\n" + body + "\n" + lines[lines.length - 1] : maskedFor(m, VISIBLE);
    blocks.push(secretHtml(m, masked, false));
    return "\x02" + (blocks.length - 1) + "\x02";
  });
  return maskSegments(esc(src), blocks, false);
}
