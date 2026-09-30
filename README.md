# hi-derola

A high-performance, lightweight terminal chat and autonomous coding assistant written in pure Rust.

## features

- minimal dark TUI (ratatui), no neon, no animations
- multi-provider: any OpenAI-compatible endpoint (OpenAI, OpenRouter, ...) + native Anthropic
- streaming responses, reasoning deltas, token usage counters
- automatic retry with backoff on 429/5xx, respects Retry-After
- tool calling: read_file, write_file, edit, list_files, bash — mutations require confirmation (y/n/a)
- diff preview before edit/write approval, colored in TUI
- mcp servers over stdio: tools exposed as `mcp__<name>__<tool>`, configured via `[[mcp]]`
- markdown rendering in answers
- input history with up/down arrows
- config: sampling params (temperature, top_p), max_tokens, model switching via /model

## run

```
cargo run
```

config: `~/.config/hi-derola/config.toml`, created on first start.

## commands

```
/file <path>   attach file to next message
/model <name>  switch model, saved to config
/clear         start new session
/quit          exit
```
