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
    /// named provider presets: [profiles.<name>] tables in config.toml;
    /// /profile <name> switches at runtime, profile fields override the
    /// base [provider] section when present, the rest is inherited
    #[serde(default)]
    pub profiles: BTreeMap<String, ProfileConfig>,
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
    /// shell used by the bash tool: program name or full path
    /// (cmd / powershell / pwsh get their own flag, everything else gets -c)
    #[serde(default)]
    pub shell: Option<String>,
    /// mcp_timeout = <seconds>: global default tools/call deadline for every
    /// mcp server that does not set its own execution_timeout (or the legacy
    /// timeout blanket); mirrors opencode's experimental.mcp_timeout
    #[serde(default)]
    pub mcp_timeout: Option<u64>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            context_limit: default_context_limit(),
            max_rounds: default_max_rounds(),
            output_budget: default_output_budget(),
            subagent_depth: default_subagent_depth(),
            compaction: Default::default(),
            shell: None,
            mcp_timeout: None,
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
    /// prune stale tool outputs between turns to keep the context lean
    #[serde(default = "default_true")]
    pub prune: bool,
    /// recent tool-output tokens protected from pruning
    #[serde(default = "default_prune_protect")]
    pub prune_protect: usize,
    /// pruning applies only when it frees at least this many tokens
    #[serde(default = "default_prune_min")]
    pub prune_min: usize,
}

impl Default for CompactionCfg {
    fn default() -> Self {
        Self {
            auto: true,
            buffer: 0,
            keep: default_keep_tokens(),
            prune: true,
            prune_protect: default_prune_protect(),
            prune_min: default_prune_min(),
        }
    }
}

fn default_keep_tokens() -> usize {
    15_000
}

fn default_prune_protect() -> usize {
    40_000
}

fn default_prune_min() -> usize {
    20_000
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
    /// name of the currently active [profiles.<name>] preset; set by
    /// /profile, an unknown name silently falls back to the base section
    #[serde(default)]
    pub active: Option<String>,
}

/// a named provider preset: every field is optional and overrides the
/// base [provider] section only when present, so a profile can be as
/// small as a model override or as complete as a whole second provider
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfileConfig {
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub stream: Option<bool>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub top_p: Option<f64>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpOAuthCfg {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_uri: Option<String>,
    /// pin the authorization server metadata url directly — the RFC 9728
    /// probe (401 + protected-resource discovery) is skipped entirely
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_server_metadata_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum McpOAuthOpt {
    On(McpOAuthCfg),
    Off(bool),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    #[serde(default)]
    pub oauth: Option<McpOAuthOpt>,
    /// sampling = false refuses sampling/createMessage from this server
    #[serde(default)]
    pub sampling: Option<bool>,
    /// elicitation = false declines elicitation/create from this server
    #[serde(default)]
    pub elicitation: Option<bool>,
    /// logging = false drops notifications/message log entries from this server
    #[serde(default)]
    pub logging: Option<bool>,
    /// keepalive = <seconds>: ping the server on an interval, note
    /// alive/unresponsive transitions in the chat (absent or 0 = off)
    #[serde(default)]
    pub keepalive: Option<u64>,
    /// timeout = <seconds>: legacy blanket override — applies to every phase
    /// (requests, discovery, tool calls, connect) unless the per-phase
    /// startup_timeout/catalog_timeout/execution_timeout fields are set
    #[serde(default)]
    pub timeout: Option<u64>,
    /// startup_timeout = <seconds>: connect + initialize window (default 30)
    #[serde(default)]
    pub startup_timeout: Option<u64>,
    /// catalog_timeout = <seconds>: tools/resources/prompts discovery and
    /// re-lists plus resource reads and prompt fetches (default 30)
    #[serde(default)]
    pub catalog_timeout: Option<u64>,
    /// execution_timeout = <seconds>: tools/call deadline (default 3600;
    /// a call reporting progress keeps sliding via resetTimeoutOnProgress,
    /// a silent one hits the deadline)
    #[serde(default)]
    pub execution_timeout: Option<u64>,
    /// enabled = false skips this server at startup; /mcpconnect refuses it
    /// until it is re-enabled (absent = enabled)
    #[serde(default)]
    pub enabled: Option<bool>,
    /// working directory for a local (stdio) server
    #[serde(default)]
    pub cwd: Option<String>,
    /// protocol version negotiation: "legacy" (default) keeps the classic
    /// per-transport handshake (2024-11-05 stdio / 2025-03-26 http, roots
    /// advertised, server answer trusted); "auto" asks for the newest
    /// revision this client speaks (2026-07-28: roots capability dropped,
    /// server answer validated); any other value pins exactly that version
    /// (roots are dropped when the pin is 2026-07-28 or newer)
    #[serde(default)]
    pub protocol_version: Option<String>,
}

impl McpConfig {
    pub fn oauth_cfg(&self) -> Option<McpOAuthCfg> {
        match &self.oauth {
            Some(McpOAuthOpt::On(c)) => Some(c.clone()),
            Some(McpOAuthOpt::Off(true)) => Some(McpOAuthCfg::default()),
            _ => None,
        }
    }
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
        assert!(c.prune);
        assert_eq!(c.prune_protect, 40_000);
        assert_eq!(c.prune_min, 20_000);
    }

    #[test]
    fn agent_shell_option() {
        let cfg: Config = toml::from_str(&format!(
            "{MINIMAL}[agent]\nshell = \"/opt/homebrew/bin/fish\"\n"
        ))
        .unwrap();
        assert_eq!(cfg.agent.shell.as_deref(), Some("/opt/homebrew/bin/fish"));
        let cfg: Config = toml::from_str(MINIMAL).unwrap();
        assert_eq!(cfg.agent.shell, None);
    }

    #[test]
    fn agent_mcp_timeout_option() {
        let cfg: Config =
            toml::from_str(&format!("{MINIMAL}[agent]\nmcp_timeout = 120\n")).unwrap();
        assert_eq!(cfg.agent.mcp_timeout, Some(120));
        let cfg: Config = toml::from_str(MINIMAL).unwrap();
        assert_eq!(cfg.agent.mcp_timeout, None, "absent keeps the default");
        // round-trips through save/load without losing the value
        let cfg: Config = toml::from_str(&format!("{MINIMAL}[agent]\nmcp_timeout = 45\n")).unwrap();
        let raw = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&raw).unwrap();
        assert_eq!(back.agent.mcp_timeout, Some(45));
    }

    #[test]
    fn compaction_overrides_and_partial_section() {
        let raw = format!(
            "{MINIMAL}[agent.compaction]\nauto = false\nbuffer = 20000\nprune = false\nprune_protect = 10000\nprune_min = 5000\n"
        );
        let cfg: Config = toml::from_str(&raw).unwrap();
        let c = &cfg.agent.compaction;
        assert!(!c.auto);
        assert_eq!(c.buffer, 20_000);
        assert_eq!(c.keep, 15_000, "unset keep keeps the default");
        assert!(!c.prune);
        assert_eq!(c.prune_protect, 10_000);
        assert_eq!(c.prune_min, 5_000);
        // round-trips through save/load without losing the section
        let raw = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&raw).unwrap();
        assert!(!back.agent.compaction.auto);
        assert_eq!(back.agent.compaction.buffer, 20_000);
        assert!(!back.agent.compaction.prune);
        assert_eq!(back.agent.compaction.prune_protect, 10_000);
        assert_eq!(back.agent.compaction.prune_min, 5_000);
    }

    #[test]
    fn mcp_sampling_flag() {
        let raw = format!(
            "{MINIMAL}[[mcp]]\nname = \"a\"\ncommand = \"x\"\nsampling = false\n\n[[mcp]]\nname = \"b\"\ncommand = \"y\"\n"
        );
        let cfg: Config = toml::from_str(&raw).unwrap();
        assert_eq!(cfg.mcp[0].sampling, Some(false));
        assert_eq!(
            cfg.mcp[1].sampling, None,
            "absent keeps the default (enabled)"
        );
    }

    #[test]
    fn mcp_elicitation_flag() {
        let raw = format!(
            "{MINIMAL}[[mcp]]\nname = \"a\"\ncommand = \"x\"\nelicitation = false\n\n[[mcp]]\nname = \"b\"\ncommand = \"y\"\n"
        );
        let cfg: Config = toml::from_str(&raw).unwrap();
        assert_eq!(cfg.mcp[0].elicitation, Some(false));
        assert_eq!(
            cfg.mcp[1].elicitation, None,
            "absent keeps the default (enabled)"
        );
    }

    #[test]
    fn mcp_keepalive_flag() {
        let raw = format!(
            "{MINIMAL}[[mcp]]\nname = \"a\"\ncommand = \"x\"\nkeepalive = 30\n\n[[mcp]]\nname = \"b\"\ncommand = \"y\"\n\n[[mcp]]\nname = \"c\"\ncommand = \"z\"\nkeepalive = 0\n"
        );
        let cfg: Config = toml::from_str(&raw).unwrap();
        assert_eq!(cfg.mcp[0].keepalive, Some(30));
        assert_eq!(cfg.mcp[1].keepalive, None, "absent keeps keepalive off");
        assert_eq!(cfg.mcp[2].keepalive, Some(0), "explicit 0 stays off");
    }

    #[test]
    fn mcp_timeout_option() {
        let raw = format!(
            "{MINIMAL}[[mcp]]\nname = \"a\"\ncommand = \"x\"\ntimeout = 300\n\n[[mcp]]\nname = \"b\"\ncommand = \"y\"\n"
        );
        let cfg: Config = toml::from_str(&raw).unwrap();
        assert_eq!(cfg.mcp[0].timeout, Some(300), "per-server timeout override");
        assert_eq!(cfg.mcp[1].timeout, None, "absent keeps the defaults");
    }

    #[test]
    fn mcp_phase_timeouts() {
        let raw = format!(
            "{MINIMAL}[[mcp]]\nname = \"a\"\ncommand = \"x\"\nstartup_timeout = 10\ncatalog_timeout = 45\nexecution_timeout = 7200\n\n[[mcp]]\nname = \"b\"\ncommand = \"y\"\n"
        );
        let cfg: Config = toml::from_str(&raw).unwrap();
        assert_eq!(cfg.mcp[0].startup_timeout, Some(10));
        assert_eq!(cfg.mcp[0].catalog_timeout, Some(45));
        assert_eq!(cfg.mcp[0].execution_timeout, Some(7200));
        assert_eq!(
            cfg.mcp[1].startup_timeout, None,
            "absent keeps the defaults"
        );
        // round-trip keeps the fields
        let back: Config = toml::from_str(&toml::to_string_pretty(&cfg).unwrap()).unwrap();
        assert_eq!(back.mcp[0].execution_timeout, Some(7200));
    }

    #[test]
    fn mcp_enabled_and_cwd_options() {
        let raw = format!(
            "{MINIMAL}[[mcp]]\nname = \"a\"\ncommand = \"x\"\nenabled = false\ncwd = \"/tmp/ws\"\n\n[[mcp]]\nname = \"b\"\ncommand = \"y\"\n"
        );
        let cfg: Config = toml::from_str(&raw).unwrap();
        assert_eq!(cfg.mcp[0].enabled, Some(false));
        assert_eq!(cfg.mcp[0].cwd.as_deref(), Some("/tmp/ws"));
        assert_eq!(cfg.mcp[1].enabled, None, "absent keeps the server enabled");
        assert_eq!(cfg.mcp[1].cwd, None);
        // round-trip keeps both fields
        let back: Config = toml::from_str(&toml::to_string_pretty(&cfg).unwrap()).unwrap();
        assert_eq!(back.mcp[0].enabled, Some(false));
        assert_eq!(back.mcp[0].cwd.as_deref(), Some("/tmp/ws"));
    }

    #[test]
    fn mcp_protocol_version_roundtrip() {
        let raw = format!(
            "{MINIMAL}[[mcp]]\nname = \"a\"\ncommand = \"x\"\nprotocol_version = \"auto\"\n\n[[mcp]]\nname = \"b\"\ncommand = \"y\"\nprotocol_version = \"2025-06-18\"\n\n[[mcp]]\nname = \"c\"\ncommand = \"z\"\n"
        );
        let cfg: Config = toml::from_str(&raw).unwrap();
        assert_eq!(cfg.mcp[0].protocol_version.as_deref(), Some("auto"));
        assert_eq!(cfg.mcp[1].protocol_version.as_deref(), Some("2025-06-18"));
        assert_eq!(
            cfg.mcp[2].protocol_version, None,
            "absent = legacy handshake"
        );
        // round-trip keeps the field
        let back: Config = toml::from_str(&toml::to_string_pretty(&cfg).unwrap()).unwrap();
        assert_eq!(back.mcp[0].protocol_version.as_deref(), Some("auto"));
        assert_eq!(back.mcp[1].protocol_version.as_deref(), Some("2025-06-18"));
    }

    #[test]
    fn mcp_oauth_config_forms() {
        let raw = format!(
            "{MINIMAL}[[mcp]]\nname = \"a\"\ncommand = \"x\"\n\n[[mcp]]\nname = \"b\"\ntype = \"remote\"\nurl = \"https://h/mcp\"\noauth = false\n\n[[mcp]]\nname = \"c\"\ntype = \"remote\"\nurl = \"https://h2/mcp\"\n\n[mcp.oauth]\nclient_id = \"cid\"\nscope = \"read write\"\n"
        );
        let cfg: Config = toml::from_str(&raw).unwrap();
        assert_eq!(cfg.mcp.len(), 3);
        assert!(cfg.mcp[0].oauth_cfg().is_none(), "stdio without oauth");
        assert!(
            cfg.mcp[1].oauth_cfg().is_none(),
            "oauth = false disables the flow"
        );
        let c = cfg.mcp[2].oauth_cfg().expect("table enables oauth");
        assert_eq!(c.client_id.as_deref(), Some("cid"));
        assert_eq!(c.scope.as_deref(), Some("read write"));
        assert_eq!(c.client_secret, None);
        // oauth = true -> defaults, round-trip keeps the shape
        let raw2 = format!(
            "{MINIMAL}[[mcp]]\nname = \"b\"\ntype = \"remote\"\nurl = \"u\"\noauth = true\n"
        );
        let cfg2: Config = toml::from_str(&raw2).unwrap();
        assert!(cfg2.mcp[0].oauth_cfg().is_some());
        let _ = toml::to_string_pretty(&cfg2).unwrap();
    }

    #[test]
    fn mcp_oauth_pinned_metadata_url() {
        let raw = format!(
            "{MINIMAL}[[mcp]]\nname = \"a\"\ntype = \"remote\"\nurl = \"https://h/mcp\"\n\n[mcp.oauth]\nclient_id = \"cid\"\nauth_server_metadata_url = \"https://as.example.com/.well-known/oauth-authorization-server\"\n"
        );
        let cfg: Config = toml::from_str(&raw).unwrap();
        let c = cfg.mcp[0].oauth_cfg().expect("table enables oauth");
        assert_eq!(
            c.auth_server_metadata_url.as_deref(),
            Some("https://as.example.com/.well-known/oauth-authorization-server")
        );
        // round-trip keeps the field
        let back: Config = toml::from_str(&toml::to_string_pretty(&cfg).unwrap()).unwrap();
        assert_eq!(
            back.mcp[0].oauth_cfg().unwrap().auth_server_metadata_url,
            c.auth_server_metadata_url
        );
    }

    const PROFILED: &str = concat!(
        "[provider]\ntype = \"openai\"\nmodel = \"base-model\"\napi_key = \"base-key\"\ntemperature = 0.5\n",
        "\n[profiles.cheap]\nmodel = \"cheap-model\"\n",
        "\n[profiles.local]\ntype = \"anthropic\"\nmodel = \"claude-x\"\nbase_url = \"http://localhost:8080\"\napi_key = \"local-key\"\nstream = false\nmax_tokens = 512\ntemperature = 0.1\ntop_p = 0.9\n",
    );

    #[test]
    fn profiles_merge_over_base_section() {
        let cfg: Config = toml::from_str(PROFILED).unwrap();
        assert_eq!(cfg.profiles.len(), 2);

        // no active profile: the base section as-is
        let eff = cfg.effective_provider();
        assert_eq!(eff.kind, "openai");
        assert_eq!(eff.model, "base-model");
        assert_eq!(eff.api_key.as_deref(), Some("base-key"));
        assert_eq!(eff.temperature, Some(0.5));

        // a partial profile only overrides what it sets, the rest is inherited
        let mut cfg: Config = toml::from_str(PROFILED).unwrap();
        cfg.provider.active = Some("cheap".into());
        let eff = cfg.effective_provider();
        assert_eq!(eff.kind, "openai", "type inherited from the base section");
        assert_eq!(eff.model, "cheap-model");
        assert_eq!(eff.api_key.as_deref(), Some("base-key"), "key inherited");
        assert_eq!(eff.temperature, Some(0.5), "sampling inherited");

        // a complete profile overrides every field it carries
        cfg.provider.active = Some("local".into());
        let eff = cfg.effective_provider();
        assert_eq!(eff.kind, "anthropic");
        assert_eq!(eff.model, "claude-x");
        assert_eq!(eff.base_url.as_deref(), Some("http://localhost:8080"));
        assert_eq!(eff.api_key.as_deref(), Some("local-key"));
        assert!(!eff.stream);
        assert_eq!(eff.max_tokens, Some(512));
        assert_eq!(eff.temperature, Some(0.1));
        assert_eq!(eff.top_p, Some(0.9));
    }

    #[test]
    fn unknown_active_profile_falls_back_to_base() {
        let mut cfg: Config = toml::from_str(PROFILED).unwrap();
        cfg.provider.active = Some("nope".into());
        let eff = cfg.effective_provider();
        assert_eq!(eff.kind, "openai");
        assert_eq!(eff.model, "base-model");
        // the summary points it out instead of failing silently
        let s = cfg.profiles_summary();
        assert!(s.contains("not defined"), "summary: {s}");
        assert!(!s.contains("<- active"), "no profile is marked active: {s}");
    }

    #[test]
    fn api_key_prefers_the_active_profile() {
        let mut cfg: Config = toml::from_str(PROFILED).unwrap();
        assert_eq!(cfg.api_key().as_deref(), Some("base-key"));
        cfg.provider.active = Some("local".into());
        assert_eq!(cfg.api_key().as_deref(), Some("local-key"));
        // empty profile key falls back to the base one
        cfg.profiles.get_mut("cheap").unwrap().api_key = Some("  ".into());
        cfg.provider.active = Some("cheap".into());
        assert_eq!(cfg.api_key().as_deref(), Some("base-key"));
    }

    #[test]
    fn profiles_roundtrip_through_toml() {
        let mut cfg: Config = toml::from_str(PROFILED).unwrap();
        cfg.provider.active = Some("local".into());
        let back: Config = toml::from_str(&toml::to_string_pretty(&cfg).unwrap()).unwrap();
        assert_eq!(back.profiles.len(), 2);
        assert_eq!(back.provider.active.as_deref(), Some("local"));
        let eff = back.effective_provider();
        assert_eq!(eff.kind, "anthropic");
        assert_eq!(eff.model, "claude-x");
        assert_eq!(eff.max_tokens, Some(512));
    }

    #[test]
    fn set_model_writes_into_the_active_profile() {
        let mut cfg: Config = toml::from_str(PROFILED).unwrap();
        cfg.set_model("switched");
        assert_eq!(cfg.provider.model, "switched");
        assert_eq!(
            cfg.profiles["cheap"].model.as_deref(),
            Some("cheap-model"),
            "profile untouched without an active profile"
        );
        cfg.provider.active = Some("cheap".into());
        cfg.set_model("switched-2");
        assert_eq!(cfg.provider.model, "switched", "base section untouched");
        assert_eq!(cfg.profiles["cheap"].model.as_deref(), Some("switched-2"));
        // and the effective view agrees
        assert_eq!(cfg.effective_provider().model, "switched-2");
    }

    #[test]
    fn profiles_summary_lists_and_marks_active() {
        let mut cfg: Config = toml::from_str(PROFILED).unwrap();
        assert!(cfg.profiles_summary().starts_with("profiles:\n  cheap"));
        cfg.provider.active = Some("local".into());
        let s = cfg.profiles_summary();
        assert!(
            s.contains("  local  anthropic · claude-x  <- active\n"),
            "active line carries the marker: {s}"
        );
        assert!(
            s.contains("  cheap  openai · cheap-model\n"),
            "inactive line carries no marker: {s}"
        );
        let empty: Config = toml::from_str(MINIMAL).unwrap();
        assert!(empty.profiles_summary().contains("none"));
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
        let eff = self.effective_provider();
        if let Some(k) = &eff.api_key {
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
        match eff.kind.as_str() {
            "openai" => std::env::var("OPENAI_API_KEY")
                .ok()
                .filter(|k| !k.trim().is_empty()),
            "anthropic" => std::env::var("ANTHROPIC_API_KEY")
                .ok()
                .filter(|k| !k.trim().is_empty()),
            _ => None,
        }
    }

    /// overlay a single profile on the base [provider] section
    fn merged(&self, prof: &ProfileConfig) -> ProviderConfig {
        let mut p = self.provider.clone();
        if let Some(v) = &prof.kind {
            p.kind = v.clone();
        }
        if let Some(v) = &prof.model {
            p.model = v.clone();
        }
        if let Some(v) = &prof.base_url {
            p.base_url = Some(v.clone());
        }
        // an empty key in a profile means "not set" — inherit the base one
        // (mirrors api_key()'s own trim handling of the base section)
        if let Some(v) = prof
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            p.api_key = Some(v.to_string());
        }
        if let Some(v) = prof.max_tokens {
            p.max_tokens = Some(v);
        }
        if let Some(v) = prof.stream {
            p.stream = v;
        }
        if let Some(v) = prof.temperature {
            p.temperature = Some(v);
        }
        if let Some(v) = prof.top_p {
            p.top_p = Some(v);
        }
        p
    }

    /// the provider section actually in use: the base [provider] with the
    /// active profile's fields merged over it; no active profile or an
    /// unknown name falls back to the base section as-is
    pub fn effective_provider(&self) -> ProviderConfig {
        match self
            .provider
            .active
            .as_deref()
            .and_then(|n| self.profiles.get(n))
        {
            Some(prof) => self.merged(prof),
            None => self.provider.clone(),
        }
    }

    /// resolve the provider config for a named profile with an optional
    /// model override — crew members and subagents use this to mix apis:
    /// profile None means the base [provider], an unknown name falls back
    /// to it too (the api key of the base is inherited unless the profile
    /// carries its own)
    pub fn provider_for(&self, profile: Option<&str>, model: Option<&str>) -> ProviderConfig {
        let mut p = match profile {
            Some(name) => self
                .profiles
                .get(name)
                .map(|prof| self.merged(prof))
                .unwrap_or_else(|| self.provider.clone()),
            None => self.provider.clone(),
        };
        if let Some(m) = model.map(str::trim).filter(|m| !m.is_empty()) {
            p.model = m.to_string();
        }
        p
    }

    /// persist a model switch: into the active profile when one is set
    /// (so /model is not shadowed by the profile on the next load), into
    /// the base section otherwise
    pub fn set_model(&mut self, model: &str) {
        match self
            .provider
            .active
            .clone()
            .and_then(|n| self.profiles.get_mut(&n))
        {
            Some(prof) => prof.model = Some(model.to_string()),
            None => self.provider.model = model.to_string(),
        }
    }

    /// one-line-per-profile listing shared by /profile in the TUI and the
    /// GUI: resolved type + model per profile, active marker, hint line
    pub fn profiles_summary(&self) -> String {
        if self.profiles.is_empty() {
            return "profiles: none — define [profiles.<name>] tables in config.toml".into();
        }
        let mut out = String::from("profiles:");
        for (name, prof) in &self.profiles {
            let eff = self.merged(prof);
            let mark = if self.provider.active.as_deref() == Some(name.as_str()) {
                "  <- active"
            } else {
                ""
            };
            out.push_str(&format!(
                "\n  {}  {} · {}{}",
                name, eff.kind, eff.model, mark
            ));
        }
        if let Some(active) = &self.provider.active {
            if !self.profiles.contains_key(active) {
                out.push_str(&format!(
                    "\n  active profile \"{active}\" is not defined — using the base [provider] section"
                ));
            }
        }
        out.push_str(
            "\nswitch: /profile <name>, /profile none (back to the base [provider] section)",
        );
        out
    }
}
