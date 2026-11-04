import type { ChatItem } from "@/store/app";

// dsh session-log-export: transcript → clipboard markdown. Tool output and
// thinking stay out (verbose internals); user/assistant/notes/errors and
// one-line tool calls carry the conversation.
export function sessionToMarkdown(title: string, transcript: ChatItem[]): string {
  const out: string[] = [`# ${title}`, ""];
  for (const it of transcript) {
    switch (it.kind) {
      case "user":
        out.push(`**you:**`, "", it.text.trim(), "");
        break;
      case "bot":
        out.push(`**assistant:**`, "", it.text.trim(), "");
        break;
      case "tool":
        out.push(`> \`${it.name}\`${it.detail ? " — " + it.detail.slice(0, 120) : ""}`, "");
        break;
      case "bg":
        out.push(`> background \`${it.bgId}\`${it.ended ? "" : " (running)"}`, "", "```", it.output.trim(), "```", "");
        break;
      case "note":
        out.push(`> ${it.text.trim()}`, "");
        break;
      case "error":
        out.push(`**error:** ${it.text.trim()}`, "");
        break;
      default:
        break;
    }
  }
  return out.join("\n").replace(/\n{3,}/g, "\n\n").trimEnd() + "\n";
}
