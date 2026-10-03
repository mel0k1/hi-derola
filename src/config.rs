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
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub lsp: Toggle,
    #[serde(default)]
    pub formatters: Toggle,
    #[serde(default)]
    pub permissions: crate::perm::PermCfg,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Toggle {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for Toggle {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    #[serde(default = "default_context_limit")]
    pub context_limit: u64,
    #[serde(default = "default_max_rounds")]
    pub max_rounds: usize,
    #[serde(default = "default_output_budget")]
    pub output_budget: usize,
    #[serde(default = "default_subagent_depth")]
    pub subagent_depth: usize,
    #[serde(default)]
    pub compaction: CompactionCfg,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            context_limit: default_context_limit(),
            max_rounds: default_max_rounds(),
            output_budget: default_output_budget(),
            subagent_depth: default_subagent_depth(),
            compaction: Default::default(),
        }
    }
}

/// context compaction tuning: [agent.compaction] in config.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionCfg {
    /// proactive compaction when the conversation approaches the context window;
    /// overflow recovery and manual /compact work regardless of this switch
    #[serde(default = "default_true")]
    pub auto: bool,
    /// headroom kept from the context window before proactive compaction
    /// triggers, in tokens; 0 = a quarter of the context window
    #[serde(default)]
    pub buffer: usize,
    /// tokens of the most recent messages kept verbatim when compacting
    #[serde(default = "default_keep_tokens")]
    pub keep: usize,
}

impl Default for CompactionCfg {
    fn default() -> Self {
        Self {
            auto: true,
            buffer: 0,
            keep: default_keep_tokens(),
        }
    }
}

fn default_keep_tokens() -> usize {
    15_000
}

fn default_subagent_depth() -> usize {
    1
}

fn default_context_limit() -> u64 {
    0
}

fn default_max_rounds() -> usize {
    15
}

fn default_output_budget() -> usize {
    32 * 1024
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UiConfig {
    #[serde(default)]
    pub theme: Option<String>,
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
    ("toggle_sidebar", "ctrl+b"),
    ("toggle_theme", "ctrl+shift+t"),
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

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = "[provider]\ntype = \"openai\"\nmodel = \"m\"\n\n";

    #[test]
    fn compaction_defaults_without_section() {
        let cfg: Config = toml::from_str(&format!("{MINIMAL}[agent]\nmax_rounds = 5\n")).unwrap();
        assert_eq!(cfg.agent.max_rounds, 5);
        let c = &cfg.agent.compaction;
        assert!(c.auto);
        assert_eq!(c.buffer, 0);
        assert_eq!(c.keep, 15_000);
    }

    #[test]
    fn compaction_overrides_and_partial_section() {
        let raw = format!(
            "{MINIMAL}[agent.compaction]\nauto = false\nbuffer = 20000\n"
        );
        let cfg: Config = toml::from_str(&raw).unwrap();
        let c = &cfg.agent.compaction;
        assert!(!c.auto);
        assert_eq!(c.buffer, 20_000);
        assert_eq!(c.keep, 15_000, "unset keep keeps the default");
        // round-trips through save/load without losing the section
        let raw = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&raw).unwrap();
        assert!(!back.agent.compaction.auto);
        assert_eq!(back.agent.compaction.buffer, 20_000);
    }
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

    /// persist an "always allow" permission rule into config.toml
    pub fn append_perm_rule(rule: crate::perm::PermRule) -> Result<()> {
        let (mut cfg, created) = Self::load_or_default()?;
        if created {
            bail!("config not initialized");
        }
        cfg.permissions.rules.push(rule);
        cfg.save()
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
