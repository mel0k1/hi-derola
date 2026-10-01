mod agent;
mod app;
mod chat;
mod config;
mod diff;
mod files;
mod gui;
mod md;
mod mcp;
mod provider;
mod search;
mod snapshot;
mod tools;
mod ui;

use anyhow::{Context, Result};

fn main() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let tui = std::env::args().any(|a| a == "--tui");
    let cfg = config::Config::load()?;
    let key = cfg
        .api_key()
        .context("no api key: set api_key in config or HI_DEROLA_API_KEY")?;
    let provider = provider::build(&cfg.provider.kind, cfg.provider.base_url.clone(), key)?;
    let has_display = cfg!(windows)
        || std::env::var_os("DISPLAY").is_some()
        || std::env::var_os("WAYLAND_DISPLAY").is_some();
    if tui || !has_display {
        runtime.block_on(async move {
            let mut terminal = ratatui::init();
            let res = app::run(&mut terminal, cfg, provider).await;
            ratatui::restore();
            res
        })
    } else {
        gui::run(cfg, provider, &runtime)
    }
}
