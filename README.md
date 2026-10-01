# hi-derola

A high-performance, lightweight chat and autonomous coding assistant written in pure Rust.

## features

- minimalist dark GUI (Slint) by default, full TUI (ratatui) with `--tui` or when no display is available
- windows, linux, macos — shell commands run via `cmd /C` on windows, `sh -c` elsewhere
- multi-provider: any OpenAI-compatible endpoint (OpenAI, OpenRouter, ...) + native Anthropic
- streaming responses, reasoning deltas, token usage counters
- automatic retry with backoff on 429/5xx, respects Retry-After
- tool calling: read_file, write_file, edit, list_files, glob, grep, bash
- diff preview before edit/write approval, colored in GUI and TUI, mutations require confirmation (y/n/a)
- snapshots: every turn is snapshotted, `/undo` / `/redo` reverts file changes
- mcp servers over stdio and streamable http: tools exposed as `mcp__<name>__<tool>`
- markdown rendering in answers
- input history with up/down arrows (TUI)
- config: sampling params (temperature, top_p), max_tokens, model switching via /model

## run

```
cargo run        # GUI (Slint)
cargo run -- --tui  # terminal UI
```

config: `~/.config/hi-derola/config.toml`, created on first start.

## config

```toml
[provider]
type = "openai"                                  # openai | anthropic
model = "anthropic/claude-sonnet-4.5"
base_url = "https://openrouter.ai/api/v1"        # any openai-compatible endpoint
api_key = ""                                     # or HI_DEROLA_API_KEY / OPENAI_API_KEY / ANTHROPIC_API_KEY

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
```

## commands

```
/file <path>   attach file to next message
/model <name>  switch model, saved to config
/undo          revert file changes of the last turn
/redo          reapply undone changes
/clear         start new session
/quit          exit
```
