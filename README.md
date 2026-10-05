# hi-derola

A high-performance, lightweight chat and autonomous coding assistant written in pure Rust.

## features

- desktop GUI (tauri, webview) plus a full TUI (ratatui) with `--tui` or when no display is available
- main screen: chat history sidebar on the left, brand in the center, chat input at the bottom, file/folder attach right under it
- chat sessions persist to disk (`~/.local/share/hi-derola/sessions`), sidebar lists past chats, tap to reopen, delete with two clicks
- dark and light themes, toggled from the sidebar, remembered in config
- inline icons, attachment chips, file browser with folder trees, no npm toolchain — static html/css/js
- windows, linux, macos — shell commands run via `cmd /C` on windows, `sh -c` elsewhere; the shell is configurable (`[agent] shell`, e.g. `"C:\\Program Files\\Git\\bin\\bash.exe"` — cmd/powershell/pwsh get their own flag, everything else gets `-c`)
- multi-provider: any OpenAI-compatible endpoint (OpenAI, OpenRouter, ...) + native Anthropic
- streaming responses and reasoning in a separate collapsible thinking block (GUI)
- api key, endpoint and model are editable right in the GUI settings; the model dropdown is filled from the provider's `/models` endpoint
- configurable hotkeys (`[keys]` in config or capture fields in GUI settings)
- tool calling: read_file (line numbers, offset/limit, images come back as native image parts), write_file, edit (tolerant to CRLF/LF, BOM, trailing whitespace; a third pass normalizes smart quotes/dashes for unicode-mangled files; a final fuzzy pass salvages near-miss blocks — at least 2 lines, >=85% line similarity — and re-indents the replacement to the matched block), apply_patch (multi-file V4A patches — nothing is written unless every hunk matches), list_files, glob, grep, bash (workdir, timeout, tail output, background tasks), webfetch (http/https, html converted to markdown/text), websearch (DuckDuckGo, no key needed), codesearch (Exa code/docs search, no key needed) + MCP servers
- custom JS tools: drop `.js` files into `.hi-derola/tools/` (project) or `~/.config/hi-derola/tools/` (global) — each exports `{ name, description, parameters, execute(input) }` and runs in the same confined boa sandbox as the code tool (no fs/network/process access, console.log captured, 30s budget); list them with `/jstools`
- bash hygiene: `AGENT=1` and `HI_DEROLA=1` are exported to every shell (scripts can detect the agent); a foreground timeout asks the tree to exit (SIGTERM) and then kills the whole process tree, not just the shell — own process group + `killpg` with a short TERM→SIGKILL grace on unix, a private kill-on-close Job Object on Windows (`kill_on_drop` stays as the last-resort net)
- lsp diagnostics after write_file/edit: edits are pushed to a language server (rust-analyzer, pyright, typescript-language-server, gopls, clangd — auto-detected on PATH) and errors/warnings come back to the model in the tool result, so it fixes its own mistakes immediately
- formatters after write_file/edit: rustfmt, gofmt, prettier (from node_modules/.bin or PATH), ruff/black (py), clang-format (c/c++), shfmt (sh), ktlint (kotlin) run on the touched file automatically
- glob/grep/list_files respect .gitignore and .ignore and skip hidden files
- prompt caching: anthropic requests mark system/tools/last message with cache_control, openai requests get a prompt_cache_key — long agent loops stop re-paying the full prompt
- background bash: `background: true` runs a command as a task (`bg-2`) — useful for dev servers and long builds; the tool returns immediately, the output streams live (GUI card with a kill button) and the full result arrives as a new message when the command finishes, `task_status` lists tasks and shows the running output, `task_kill` stops a task
- todo tools: `todowrite` / `todoread` keep a structured task list (content, status, priority) for multi-step work; the live list renders as a card in the GUI and info lines in the TUI, and is stored with the session
- question tool pauses the run and asks the user multiple-choice questions right in the UI (free-form answer, esc skips)
- subagent tool delegates a task to a fresh-context agent with a trimmed toolset; `background: true` runs it async — the tool returns a task id (`bg-1`) immediately, the result arrives as a new message when done, `task_status` lists tasks and returns finished results, `task_kill` aborts a running one; finished runs are kept as child sessions (↳ in the sidebar) and can be continued by passing `session_id` back to the subagent tool with full context; `subagent_depth` in `[agent]` controls how deep subagents may nest and spawn their own subagents
- custom agents: a markdown file at `.hi-derola/agents/<name>.md` or `~/.config/hi-derola/agents/<name>.md` (frontmatter: `description`, `model`, `temperature`, `read_only`) defines an agent profile; the `subagent` tool accepts `agent: <name>`, built-in profiles are `general` and `explore`; typing `@<agent> <task>` in the input runs it directly (with autocomplete)
- vision: attach images (png/jpeg/gif/webp, up to 5 MB) via `/file <path>` or the attach picker — they are sent to the model as native image parts (OpenAI-compatible and Anthropic)
- reject with feedback: deny a confirmation and type what to change instead — the feedback reaches the model as the tool result and the run continues with the adjusted plan (TUI: `f` at the confirm prompt, GUI: the feedback field in the dialog)
- plan mode: read-only research — write_file/edit are removed from the toolset and denied; the agent saves the plan to `.hi-derola/plan.md` with `plan_write` and calls `plan_exit` to ask you to leave plan mode — approve to switch to building in the same run, keep planning, or reply with feedback to refine the plan (GUI: the `plan` toggle or ctrl+shift+p, TUI and GUI: `/plan`)
- diff preview before edit/write approval, colored in GUI and TUI, mutations require confirmation
- snapshots: every turn is snapshotted to a shadow git repo in the data dir — `/undo` / `/redo` reverts file changes and survives restarts, respects the project's .gitignore, and never touches your own `.git` (in-memory fallback when git is unavailable)
- AGENTS.md / CLAUDE.md project instructions are picked up from the working directory into the system prompt; when the agent reads a file deeper down, an AGENTS.md/CLAUDE.md found between the working directory and that file is attached to the read result; `/init` scans the repo and creates or improves AGENTS.md itself
- the system prompt opens with an env block — working directory, git repo flag, platform/arch, today's date — so the model knows its surroundings without burning a tool call
- prompt variants per model family: the system prompt gets a short behavior addendum tuned to the model in use (gpt-4/o1/o3 → keep-going "beast" style, gpt → verify-before-change, codex → complete code + run checks, gemini → follow conventions/verify libraries, claude → edit over create/skip praise, kimi → bias to action), picked automatically from the model id the same way opencode does; /model rebuilds it, subagents get it too
- skills: drop a `SKILL.md` (frontmatter + instructions) into `.hi-derola/skills/<name>/` or `~/.config/hi-derola/skills/<name>/` and the agent gets a `skill` tool to load it on demand
- custom commands: a markdown file at `.hi-derola/commands/<name>.md` or `~/.config/hi-derola/commands/<name>.md` (optional `description:` frontmatter, `$ARGUMENTS` and `$1..$9` placeholders) becomes a `/name` command in the GUI and TUI
- `/compact` summarizes and shrinks the conversation on demand, `/export [path]` saves the session as markdown
- `@` file mentions in the input attach files automatically (with autocomplete in the GUI), session titles generated by the model (GUI)
- model catalog: context windows and prices for known models, `context_limit = 0` auto-fits the window, session cost is counted from usage and shown in the status bar and sidebar
- steer/queue: messages sent while the agent is busy are queued — they steer the current run between tool rounds, or start the next run right after it finishes (esc stops the current run, a queued message keeps going); the queue is durable — it is persisted in the session file on every change, so queued messages survive a crash or app restart and steer the next run when the session is reopened
- permission rules: `allow | ask | deny` per tool plus wildcard patterns (e.g. allow `git *`, deny `rm *`) in `[permissions]`, shown in GUI settings; built-in protections ask before reading secret files (`.env`, `prod.env`, ...), before touching paths outside the working directory, and before running bash in a `workdir` outside it (explicit rules override); "always allow" (`w` in TUI, button in GUI) saves a wildcard rule to the config (`git push *`, `*.rs`, `https://host/*`, `mcp__srv__*`); for external paths the saved rule is scoped to the granted directory (`/tmp/**`) instead of a global extension wildcard
- lsp navigation: the `lsp` tool drives the auto-detected language server (same set as diagnostics) for hover, definition, references, implementation, document/workspace symbols and call hierarchy (incoming_calls / outgoing_calls — who calls the function, what it calls, with call sites) — coordinates are 1-based, results come back as compact `path:line:col` lists or a symbol tree instead of raw JSON; positions are converted to/from UTF-16 so unicode lines stay accurate; in read-only mode `lsp` stays available
- code mode: the `code` tool (registered when at least one MCP server is connected) runs a confined JavaScript program — the model writes a small script and calls MCP tools as `mcp.<server>.<tool>({...})` with `await`, loops, branching and try/catch inside a boa_engine sandbox that has no filesystem, network or process access; each child call passes through the same permission checks and confirm UI as a direct `mcp__` call (a denied call throws, "always allow" persists the rule mid-run); the script returns a value, `console.log` output comes back as Logs, and MCP text results that parse as JSON are handed over as real objects — so a batch of MCP calls, their filtering and aggregation happens in one turn instead of one round trip per call
- mcp servers over stdio, streamable http and legacy http + sse (the sse transport is tried automatically when the streamable handshake fails on a non-auth error): tools exposed as `mcp__<name>__<tool>` (a result that carries only `structuredContent` and no text blocks is serialized to text); remote servers can authorize through OAuth 2.0 (PKCE, dynamic client registration, resource-metadata discovery) — set `oauth = true` (or a `[mcp.oauth]` table with `client_id`/`client_secret`/`scope`/`redirect_uri`) on a remote server, run `/mcpauth <name>` (TUI) or hit the auth button in GUI settings, tokens are cached in `<data>/hi-derola/mcp-auth.json` with 0600 perms and refreshed automatically; a 401 mid-request forces one token refresh and retry before surfacing; resources and prompts are discovered from the server's advertised capabilities (paginated lists, connect logs show the counts) alongside resource templates from `resources/templates/list` (`/mcpres [server]` shows both — templates are uri schemes with `{placeholders}` to fill before reading) — the `mcp_resource` tool reads any listed resource (binary data is summarized, allowed in read-only mode), `/mcpres [server]` lists resources, `/mcpread <server> <uri>` dumps one into the chat, `/mcpprompt <server> <name> [key=value ...]` fetches a prompt template and runs it as your message (no args lists prompts, `*` marks required args); the GUI settings have a res button per server that browses resources and prompts — clicking one loads its text into the chat input; the client advertises `roots` and `sampling` capabilities and keeps a background listener per server so server-initiated traffic arrives live: server `instructions` from initialize ride the system prompt as an `<mcp_instructions>` block (a server whose tools are all denied stays quiet); `roots/list` is answered with the workspace root (cwd), `sampling/createMessage` runs a one-shot completion on the configured provider (a note in the chat shows what each server asked for; opt out per server with `sampling = false`), and `notifications/tools|resources|prompts/list_changed` invalidate the cached lists so the next use re-fetches them; resources can be subscribed for live updates — `/mcpsub <server> <uri>` (or the sub button next to a resource in the GUI browser) registers via `resources/subscribe` and every `notifications/resources/updated` re-reads the resource into the chat as a note, `/mcpunsub <server> <uri>` stops it, `/mcpres` marks subscribed entries; servers can log through `notifications/message` (syslog-style levels, debug…emergency) — every entry lands in a 500-entry ring buffer, `/mcplog [server]` shows the recent tail, warning and above also pop into the chat as notes, `/mcplog set <server|all> <level>` sends `logging/setLevel`, per-server opt-out with `logging = false`; a stdio server's stderr is drained into the same buffer as info entries with logger `stderr` (visible in the `/mcplog` tail, never pops into the chat) and is dropped too when `logging = false`; servers can ask the user for structured input through `elicitation/create` (flat schemas: string/number/integer/boolean/enum) — the request surfaces through the ask flow (TUI question prompt, GUI dialog) — the GUI renders the requested schema as a form (one control per property: text/number inputs, checkboxes for booleans, dropdowns for enums, required fields marked and enforced before submit), enum fields become pickable options, single-field schemas take the raw answer, multi-field ones expect a json object; esc/skip cancels, a declined or timed-out prompt answers `decline`/`cancel` so the server's flow stays well-defined, per-server opt-out with `elicitation = false`; long-running `tools/call` requests carry a `_meta.progressToken`, so `notifications/progress` from the server (progress/total/message) surface as live notes in the chat while the call runs — `mcp <name>: <tool> 3/5 — fetching`; the client answers server `ping` requests with an empty result per spec, and per-server `keepalive = <seconds>` pings on an interval — the alive/unresponsive transitions surface as chat notes exactly once per flip (`keepalive ping failed — server unresponsive`, `keepalive recovered`), so a hung stdio/http server is visible without spam; per-server `timeout = <seconds>` overrides the default request (15s), tool-call (120s) and connect (30s) timeouts
- agent loop: token-budgeted context compaction (template summary, keeps the last ~15k tokens verbatim, tunable via `[agent.compaction]`, on overflow too), output `max_tokens` shrinks to the remaining window, per-tool output budget, cached-tokens-aware cost, graceful wrap-up at the round limit that keeps the cache prefix; a reply cut off by the output limit continues automatically ("continue from where you left off", up to 3 times — truncated tool calls are dropped instead of corrupting the transcript)
- repeated compaction merges instead of starting over: a fresh `/compact` (or an auto one) passes the previous summary as `<prior-summary>` and folds it into the new one — older decisions and constraints survive every compaction cycle, and the old summary is replaced rather than accumulating
- tool-output spill: a result over 50 KB is saved verbatim to `<data>/hi-derola/truncated/` (kept for 7 days) and the inline reply carries the path, so the budgeted-away part stays reachable with `read_file`; reading these files is exempt from the external-directory approval, writing them is not
- stale tool-output pruning between turns (`[agent.compaction] prune`): walking backwards from the newest exchange, tool outputs beyond the protected ~40k-token window are replaced with short `[pruned tool output (~N tokens)]` placeholders — big one-shot logs and dumps stop riding along in the context forever; pruning only commits when it frees at least ~20k tokens, `skill` outputs stay verbatim, the current and previous exchange are never touched
- markdown rendering in answers, token usage counters, automatic retry with backoff on 429/5xx and empty replies

### gui extras

- command palette (`ctrl+k`): fuzzy search across slash commands, saved sessions, models and actions (theme, settings, review, sidebar) — keyboard-first, enter runs, esc closes
- review panel (diff button with a per-file badge in the top bar): every file the session changed with cumulative `+adds / −dels`, expandable diff per file; counts survive a reload (stored with the session), diffs stay for the live session
- context pill in the status bar: percent of the context window used by the current conversation (amber above 80%, red above 92%), click runs `/compact`; fed by the real usage numbers of the last request
- session list with model-generated titles, cost and message counts; reopen or delete from the sidebar, undo/redo buttons, plan-mode toggle, attachment chips

## architecture

```
src/
  main.rs          binary entry: starts the TUI by default, --tui forces it
  lib.rs           inline AGENTS.md discovery for read_file
  app.rs           TUI (ratatui): event loop, transcript rendering, /commands, sessions
  agent.rs         the agent loop: rounds, steering, compaction, output fitting,
                   wrap-up, subagent spawning, permission checks
  chat.rs          Message / Session / ToolCall / Image types
  provider/        openai (SSE) + anthropic (messages) clients, ApiEvent stream,
                   list_models, retry with jitter
  tools.rs         tool specs, dispatch, detail/preview/paths, bash, bg tasks
  codemode.rs      code tool: boa_engine JS sandbox orchestrating MCP tools
  jstools.rs       user-defined JS tools (.hi-derola/tools/*.js) in the boa sandbox
  bg.rs            background task registry (bash + subagents), live output
  perm.rs          permission resolution (allow/ask/deny + wildcard rules)
  sessions.rs      persisted sessions (json per session) + ChangeRec for review
  snapshot.rs      per-turn file snapshots for /undo /redo
  diff.rs          LCS line diff used everywhere
  patch.rs         V4A patch parser + applier for apply_patch
  files.rs         read/write, images, @mention scanning, file trees
  lsp.rs           language-server pool, diagnostics, navigation (incl. call hierarchy)
  fmt.rs           auto-formatters per file type
  mcp.rs           stdio + streamable http MCP clients: tools, resources, templates, prompts, roots, sampling, elicitation, subscriptions, logging, live notifications
  mcpauth.rs       mcp OAuth 2.0: PKCE, discovery, token store, refresh
  models.rs        model catalog: context windows, prices, cost with cached discount
  todo.rs          shared todo state for the todo tools
  skills.rs        SKILL.md loader for the skill tool
  commands.rs      custom /commands + /init + /export
  agents.rs        custom agent profiles, @-mentions
  web.rs           webfetch + websearch (duckduckgo)
  exa.rs           codesearch: Exa code/docs search (JSON-RPC over SSE, no key)
  search.rs        glob/grep/list_files with gitignore support
  config.rs        config.toml model + persistence
  md.rs, ui.rs     markdown + shared UI helpers

src-tauri/         desktop shell: Shared state, event pump (ApiEvent -> webview),
                   tauri commands (send/open_session/confirm/ask/...)
ui/                static html/css/js frontend, no build step; mock.js fakes the
                   backend so the GUI runs in a plain browser
scripts/           mock LLM + pty driver for end-to-end tests
```

a turn flows: input -> command or prompt -> `agent::run` loop (chat -> stream events -> tool calls -> confirm/ask -> tool results) -> `Done`; every event crosses to the TUI or through the tauri pump into the webview, which renders chunks, diffs, todos and usage live.

## run

```
cargo run                        # terminal UI
cargo run -p hi-derola-gui       # desktop GUI (tauri)
```

config: `~/.config/hi-derola/config.toml`, created on first start. On windows the GUI builds need no extra system deps (WebView2); on linux install `libwebkit2gtk-4.1-dev`.

## config

```toml
[provider]
type = "openai"                                  # openai | anthropic
model = "anthropic/claude-sonnet-4.5"
base_url = "https://openrouter.ai/api/v1"        # any openai-compatible endpoint
api_key = ""                                     # or set it in the GUI settings

# local mcp server over stdio
[[mcp]]
name = "fs"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]

# remote mcp server over streamable http
[[mcp]]
name = "search"
type = "remote"
url = "https://example.com/mcp"
headers = { Authorization = "Bearer ..." }

# gui hotkeys, "none" disables
[keys]
send = "enter"
newline = "shift+enter"
stop = "escape"
new_session = "ctrl+n"
open_settings = "ctrl+comma"
undo = "ctrl+z"
redo = "ctrl+shift+z"
toggle_thinking = "ctrl+t"
toggle_sidebar = "ctrl+b"
toggle_theme = "ctrl+shift+t"
palette = "ctrl+k"

# interface
[ui]
# theme = "light"                                # dark (default) | light

# agent loop
[agent]
context_limit = 0                                # tokens, 0 = auto from the model catalog (90% of the window)
max_rounds = 15                                  # tool rounds per message
output_budget = 32768                            # max chars of one tool result
subagent_depth = 1                               # how deep subagents may spawn subagents

# context compaction tuning
[agent.compaction]
auto = true                                      # proactive compaction near the window; overflow recovery and /compact stay on anyway
buffer = 0                                       # headroom in tokens before compaction, 0 = a quarter of the window (~75% trigger)
keep = 15000                                     # tokens of recent messages kept verbatim when compacting
prune = true                                     # prune stale tool outputs between turns
prune_protect = 40000                            # recent tool-output tokens kept verbatim
prune_min = 20000                                # prune only when it frees at least this many tokens

# tool permissions: allow | ask | deny, first matching rule wins
# unset tools default to: write_file/edit/bash/mcp ask, read-only (incl. webfetch/subagent/todowrite) allow
# background bash asks the same way as a foreground one
[permissions]
bash = "ask"
# webfetch = "allow"
# subagent = "ask"

[[permissions.rules]]
tool = "bash"
pattern = "git *"                                # command for bash, path for edit/write_file, url for webfetch
permission = "allow"
```

## commands (both UIs)

```
/file <path>   attach file to next message
/model <name>  switch model, saved to config
/models        list models available for the api key
/plan          toggle plan mode (read-only research)
/undo /redo    revert or reapply file changes of a turn
/init          create or improve AGENTS.md for this project
/compact       summarize and shrink the conversation context
/export [path] save the session as markdown
/sessions      list saved sessions (TUI; the GUI has the sidebar)
/resume [id]   switch to a saved session, latest by default (TUI)
/clear         start new session
```

gui-only: command palette (`ctrl+k`), review panel, context pill, attachment chips, hotkey capture in settings.
