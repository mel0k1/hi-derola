use anyhow::{Context, Result};
use hi_derola::{app, config, fmt, lsp, provider, tools};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    // headless subcommands that must not require a config/api key
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("hi-derola {}", hi_derola::update::current_version());
        return Ok(());
    }
    let want_update = args.iter().any(|a| a == "update" || a == "self-update");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    if want_update {
        return runtime.block_on(async {
            match hi_derola::update::run(|line| println!("{line}")).await {
                Ok(msg) => {
                    println!("{msg}");
                    Ok(())
                }
                Err(e) => Err(anyhow::anyhow!("update failed: {e:#}")),
            }
        });
    }
    let tui = args.iter().any(|a| a == "--tui");
    let cfg = config::Config::load()?;
    lsp::set_enabled(cfg.lsp.enabled);
    fmt::set_enabled(cfg.formatters.enabled);
    tools::set_shell(cfg.agent.shell.clone());
    let key = cfg
        .api_key()
        .context("no api key: set api_key in config or HI_DEROLA_API_KEY")?;
    let eff = cfg.effective_provider();
    let provider = provider::build(&eff.kind, eff.base_url.clone(), key)?;
    let has_display = cfg!(windows)
        || std::env::var_os("DISPLAY").is_some()
        || std::env::var_os("WAYLAND_DISPLAY").is_some();
    if has_display && !tui {
        eprintln!(
            "desktop gui lives in hi-derola-gui: cargo run -p hi-derola-gui (starting tui now)"
        );
    }
    runtime.block_on(async move {
        let mut terminal = ratatui::init();
        let res = app::run(&mut terminal, cfg, provider).await;
        ratatui::restore();
        res
    })
}
