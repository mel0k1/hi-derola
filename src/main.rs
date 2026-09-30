mod config;

fn main() -> anyhow::Result<()> {
    let cfg = config::Config::load()?;
    println!("hi-derola · {} · {}", cfg.provider.kind, cfg.provider.model);
    Ok(())
}
