use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const EXAMPLE: &str = include_str!("../config.example.toml");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub provider: ProviderConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    #[serde(rename = "type")]
    pub kind: String,
    pub model: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default = "default_true")]
    pub stream: bool,
}

fn default_true() -> bool {
    true
}

pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("hi-derola")
        .join("config.toml")
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = config_path();
        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).context("create config dir")?;
            }
            std::fs::write(&path, EXAMPLE).context("write config")?;
            bail!(
                "config created at {}, fill api_key and restart",
                path.display()
            );
        }
        let raw = std::fs::read_to_string(&path).context("read config")?;
        let cfg: Config = toml::from_str(&raw).context("parse config")?;
        Ok(cfg)
    }

    pub fn save(&self) -> Result<()> {
        let raw = toml::to_string_pretty(self).context("serialize config")?;
        std::fs::write(config_path(), raw)?;
        Ok(())
    }

    pub fn api_key(&self) -> Option<String> {
        if let Some(k) = &self.provider.api_key {
            let k = k.trim();
            if !k.is_empty() {
                return Some(k.to_string());
            }
        }
        if let Ok(k) = std::env::var("HI_DEROLA_API_KEY") {
            let k = k.trim();
            if !k.is_empty() {
                return Some(k.to_string());
            }
        }
        match self.provider.kind.as_str() {
            "openai" => std::env::var("OPENAI_API_KEY").ok().filter(|k| !k.trim().is_empty()),
            "anthropic" => std::env::var("ANTHROPIC_API_KEY").ok().filter(|k| !k.trim().is_empty()),
            _ => None,
        }
    }
}
