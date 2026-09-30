mod agent;
mod app;
mod chat;
mod config;
mod diff;
mod files;
mod md;
mod mcp;
mod provider;
mod tools;
mod ui;

use anyhow::{Context, Result};

fn main() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(run())
}

async fn run() -> Result<()> {
    let cfg = config::Config::load()?;
    let key = cfg
        .api_key()
        .context("no api key: set api_key in config or HI_DEROLA_API_KEY")?;
    let provider = provider::build(&cfg.provider.kind, cfg.provider.base_url.clone(), key)?;
    let mut terminal = ratatui::init();
    let res = app::run(&mut terminal, cfg, provider).await;
    ratatui::restore();
    res
}
