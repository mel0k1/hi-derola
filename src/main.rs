mod chat;
mod config;
mod provider;

fn main() -> anyhow::Result<()> {
    let cfg = config::Config::load()?;
    let key = cfg
        .api_key()
        .ok_or_else(|| anyhow::anyhow!("no api key: set api_key in config or HI_DEROLA_API_KEY"))?;
    let provider = provider::build(&cfg.provider.kind, cfg.provider.base_url.clone(), key)?;
    println!("hi-derola · {} · {}", provider.name(), cfg.provider.model);
    Ok(())
}
