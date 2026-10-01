# hi-derola

A high-performance, lightweight chat and autonomous coding assistant written in pure Rust.

## features

- desktop GUI (tauri, webview) plus a full TUI (ratatui) with `--tui` or when no display is available
- main screen: chat history sidebar on the left, brand in the center, chat input at the bottom, file/folder attach right under it
- chat sessions persist to disk (`~/.local/share/hi-derola/sessions`), sidebar lists past chats, tap to reopen, delete with two clicks
- dark and light themes, toggled from the sidebar, remembered in config
- inline icons, attachment chips, file browser with folder trees, no npm toolchain — static html/css/js
- windows, linux, macos — shell commands run via `cmd /C` on windows, `sh -c` elsewhere
- multi-provider: any OpenAI-compatible endpoint (OpenAI, OpenRouter, ...) + native Anthropic
- streaming responses and reasoning in a separate collapsible thinking block (GUI)
- api key, endpoint and model are editable right in the GUI settings; the model dropdown is filled from the provider's `/models` endpoint
- configurable hotkeys (`[keys]` in config or capture fields in GUI settings)
- tool calling: read_file, write_file, edit, list_files, glob, grep, bash + MCP servers
- diff preview before edit/write approval, colored in GUI and TUI, mutations require confirmation
- snapshots: every turn is snapshotted, `/undo` / `/redo` reverts file changes
- mcp servers over stdio and streamable http: tools exposed as `mcp__<name>__<tool>`
- markdown rendering in answers, token usage counters, automatic retry with backoff on 429/5xx

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

# interface
[ui]
# theme = "light"                                # dark (default) | light
```

## commands (both UIs)

```
/file <path>   attach file to next message
/model <name>  switch model, saved to config
/models        list models available for the api key
/undo /redo    revert or reapply file changes of a turn
/clear         start new session
```
