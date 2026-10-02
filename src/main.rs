use anyhow::{Context, Result};
use hi_derola::{app, config, fmt, lsp, provider};

fn main() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let tui = std::env::args().any(|a| a == "--tui");
    let cfg = config::Config::load()?;
    lsp::set_enabled(cfg.lsp.enabled);
    fmt::set_enabled(cfg.formatters.enabled);
    let key = cfg
        .api_key()
        .context("no api key: set api_key in config or HI_DEROLA_API_KEY")?;
    let provider = provider::build(&cfg.provider.kind, cfg.provider.base_url.clone(), key)?;
    let has_display = cfg!(windows)
        || std::env::var_os("DISPLAY").is_some()
        || std::env::var_os("WAYLAND_DISPLAY").is_some();
    if has_display && !tui {
        eprintln!("desktop gui lives in hi-derola-gui: cargo run -p hi-derola-gui (starting tui now)");
    }
    runtime.block_on(async move {
        let mut terminal = ratatui::init();
        let res = app::run(&mut terminal, cfg, provider).await;
        ratatui::restore();
        res
    })
}
