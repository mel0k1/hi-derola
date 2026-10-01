use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

const EXAMPLE: &str = include_str!("../config.example.toml");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub provider: ProviderConfig,
    #[serde(default)]
    pub mcp: Vec<McpConfig>,
    #[serde(default)]
    pub keys: BTreeMap<String, String>,
}

pub const DEFAULT_KEYS: &[(&str, &str)] = &[
    ("send", "enter"),
    ("newline", "shift+enter"),
    ("stop", "escape"),
    ("new_session", "ctrl+n"),
    ("open_settings", "ctrl+comma"),
    ("undo", "ctrl+z"),
    ("redo", "ctrl+shift+z"),
    ("toggle_thinking", "ctrl+t"),
];

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
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub top_p: Option<f64>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConfig {
    pub name: String,
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("hi-derola")
        .join("config.toml")
}

impl Config {
    pub fn load_or_default() -> Result<(Self, bool)> {
        let path = config_path();
        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).context("create config dir")?;
            }
            std::fs::write(&path, EXAMPLE).context("write config")?;
            let cfg: Config = toml::from_str(EXAMPLE).context("parse config")?;
            return Ok((cfg, true));
        }
        let raw = std::fs::read_to_string(&path).context("read config")?;
        let cfg: Config = toml::from_str(&raw).context("parse config")?;
        Ok((cfg, false))
    }

    pub fn load() -> Result<Self> {
        let (cfg, created) = Self::load_or_default()?;
        if created {
            bail!(
                "config created at {}, fill api_key and restart",
                config_path().display()
            );
        }
        Ok(cfg)
    }

    pub fn save(&self) -> Result<()> {
        let raw = toml::to_string_pretty(self).context("serialize config")?;
        std::fs::write(config_path(), raw)?;
        Ok(())
    }

    pub fn keys(&self) -> BTreeMap<String, String> {
        let mut out: BTreeMap<String, String> = DEFAULT_KEYS
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        for (k, v) in &self.keys {
            let v = v.trim().to_lowercase();
            if v == "none" {
                out.remove(k);
            } else if !v.is_empty() {
                out.insert(k.clone(), v);
            }
        }
        out
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
