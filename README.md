# hi-derola

<p align="center"><img src="assets/logo.png" width="140" alt="Hi!Derola" /></p>

[![ci](https://github.com/mel0k1/hi-derola/actions/workflows/ci.yml/badge.svg)](https://github.com/mel0k1/hi-derola/actions/workflows/ci.yml)

a fast coding assistant in pure rust. one codebase, two faces: a desktop gui (tauri) and a terminal ui (ratatui). the frontend is plain html/css/js with no build step.

the agent chats, reads and edits files, runs shell commands, calls mcp tools and keeps going until the task is done. every mutation shows a diff and waits for your ok, and any turn can be rolled back with `/undo`.

## what it does

- **two uis, one core.** desktop gui and terminal ui share the same engine, sessions persist on disk and reopen where you left them.
- **an agent that works.** streaming answers with a collapsible reasoning block, tool rounds, message steering while a run is active, plan mode for read-only research, automatic context compaction, token and cost tracking (`/usage`).
- **a real toolset.** files, bash (foreground and background tasks), glob/grep, subagents, todos, questions, web fetch and search, code search, image input, custom js tools and custom agents defined in markdown.
- **mcp, both flavors.** stdio and remote servers (streamable http, sse), oauth 2.0 with pkce, resources and prompts. servers are added in the gui settings and saved to config.toml.
- **guard rails.** per-tool permissions with wildcard rules, diff preview before every edit, per-turn snapshots for `/undo` and `/redo`, `/doctor` for a one-shot environment check.
- **beyond the host.** multi-agent crew sessions that talk in rounds, remote hosts over ssh, local qemu sandboxes so the agent's code never touches your machine.

## run

```
cargo run                    # terminal ui
cargo run -p hi-derola-gui   # desktop gui (tauri)
```

config lives at `~/.config/hi-derola/config.toml` and is created on first start. the api key, model and mcp servers can be set right in the gui settings. every option is documented in [config.example.toml](config.example.toml).

gui system deps: none on windows (webview2 ships with it), `libwebkit2gtk-4.1-dev` on linux.

## commands

`/file` `/model` `/models` `/profile` `/plan` `/undo` `/redo` `/init` `/compact` `/export` `/usage` `/sessions` `/resume` `/clear` `/update` `/help`

type `/help` in either ui for the full list with arguments.

## development

ci runs on every push: `cargo fmt --check`, `cargo test`, a syntax gate for the ui and a tauri check on windows. pushing a `v*` tag builds release binaries and attaches them to a github release, which is what `hi-derola update` self-update looks for.

the frontend lives in `ui/` and runs in a plain browser with a mocked backend (`mock.js`), so gui work needs no compiled shell.
