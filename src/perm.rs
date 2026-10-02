use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Perm {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermRule {
    pub tool: String,
    #[serde(default)]
    pub pattern: Option<String>,
    pub permission: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PermCfg {
    #[serde(default)]
    pub edit: Option<String>,
    #[serde(default)]
    pub write_file: Option<String>,
    #[serde(default)]
    pub bash: Option<String>,
    #[serde(default)]
    pub mcp: Option<String>,
    #[serde(default)]
    pub webfetch: Option<String>,
    #[serde(default)]
    pub websearch: Option<String>,
    #[serde(default)]
    pub subagent: Option<String>,
    #[serde(default)]
    pub rules: Vec<PermRule>,
}

impl PermCfg {
    pub fn check(&self, tool: &str, args: &str) -> Perm {
        let (key, subject) = split_tool(tool, args);
        for r in &self.rules {
            if !wc(&r.tool, &key) {
                continue;
            }
            match &r.pattern {
                Some(p) if !wc(p, &subject) => continue,
                _ => return parse(&r.permission),
            }
        }
        // builtin protections: explicit user rules above can override these
        if matches!(
            key.as_str(),
            "read_file" | "list_files" | "glob" | "grep" | "edit" | "write_file"
        ) {
            if key == "read_file" && env_protected(&subject) {
                return Perm::Ask;
            }
            if external_dir(&subject).is_some() {
                return Perm::Ask;
            }
        }
        let field = match key.as_str() {
            "edit" => self.edit.as_deref(),
            "write_file" => self.write_file.as_deref(),
            "bash" => self.bash.as_deref(),
            "mcp" => self.mcp.as_deref(),
            "webfetch" => self.webfetch.as_deref(),
            "websearch" => self.websearch.as_deref(),
            "subagent" => self.subagent.as_deref(),
            _ => None,
        };
        match field {
            Some(s) => parse(s),
            None => default_perm(&key),
        }
    }
}

fn split_tool(tool: &str, args: &str) -> (String, String) {
    if let Some(rest) = tool.strip_prefix("mcp__") {
        return ("mcp".to_string(), rest.to_string());
    }
    match tool {
        "edit" | "write_file" | "read_file" | "list_files" | "glob" | "grep" | "bash" => {
            let v: serde_json::Value =
                serde_json::from_str(args).unwrap_or(serde_json::Value::Null);
            let subject = v["path"]
                .as_str()
                .or_else(|| v["command"].as_str())
                .unwrap_or("")
                .to_string();
            (tool.to_string(), subject)
        }
        "webfetch" => {
            let v: serde_json::Value =
                serde_json::from_str(args).unwrap_or(serde_json::Value::Null);
            let subject = v["url"].as_str().unwrap_or("").to_string();
            (tool.to_string(), subject)
        }
        "websearch" => {
            let v: serde_json::Value =
                serde_json::from_str(args).unwrap_or(serde_json::Value::Null);
            let subject = v["query"].as_str().unwrap_or("").to_string();
            (tool.to_string(), subject)
        }
        other => (other.to_string(), String::new()),
    }
}

fn default_perm(key: &str) -> Perm {
    match key {
        "write_file" | "edit" | "bash" | "mcp" => Perm::Ask,
        _ => Perm::Allow,
    }
}

/// secrets files (.env, .env.local, prod.env, ...); examples stay readable
fn env_protected(path: &str) -> bool {
    let name = std::path::Path::new(path.trim())
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if name.ends_with(".env.example")
        || name.ends_with(".env.sample")
        || name.ends_with(".env.template")
    {
        return false;
    }
    name == ".env" || name.starts_with(".env.") || name.ends_with(".env")
}

/// returns Some(resolved path) when the target lies outside the working directory
fn external_dir(path: &str) -> Option<String> {
    let p = path.trim();
    if p.is_empty() {
        return None;
    }
    let cwd = std::env::current_dir().ok()?;
    let base = std::path::Path::new(p);
    let abs = if base.is_absolute() {
        base.to_path_buf()
    } else {
        cwd.join(base)
    };
    let mut probe = abs.as_path();
    loop {
        match std::fs::canonicalize(probe) {
            Ok(c) => {
                return if c.starts_with(&cwd) {
                    None
                } else {
                    Some(c.display().to_string())
                };
            }
            Err(_) => probe = probe.parent()?,
        }
    }
}

fn parse(s: &str) -> Perm {
    match s.trim().to_lowercase().as_str() {
        "allow" => Perm::Allow,
        "deny" => Perm::Deny,
        _ => Perm::Ask,
    }
}

/// build an allow-rule to persist when the user answers "always allow";
/// bash -> "<first words> *", files -> "*.<ext>", urls -> "scheme://host/*"
pub fn derive_rule(tool: &str, args: &str) -> Option<PermRule> {
    let v: serde_json::Value = serde_json::from_str(args).unwrap_or(serde_json::Value::Null);
    let (key, pattern) = match tool {
        "bash" => {
            let cmd = v["command"].as_str()?.trim();
            let words: Vec<&str> = cmd.split_whitespace().take(2).collect();
            if words.is_empty() {
                return None;
            }
            (tool.to_string(), format!("{} *", words.join(" ")))
        }
        "edit" | "write_file" => {
            let path = v["path"].as_str().unwrap_or("");
            let ext = std::path::Path::new(path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            let p = if ext.is_empty() {
                "*".to_string()
            } else {
                format!("*.{ext}")
            };
            (tool.to_string(), p)
        }
        "webfetch" => {
            let url = v["url"].as_str().unwrap_or("");
            let (scheme, rest) = url.split_once("://")?;
            let host = rest.split(['/', '?', '#']).next().unwrap_or("");
            if host.is_empty() {
                return None;
            }
            ("webfetch".to_string(), format!("{scheme}://{host}/*"))
        }
        t if t.starts_with("mcp__") => {
            let srv = t.strip_prefix("mcp__")?.split("__").next()?;
            if srv.is_empty() {
                return None;
            }
            ("mcp".to_string(), format!("{srv}__*"))
        }
        _ => (tool.to_string(), "*".to_string()),
    };
    Some(PermRule {
        tool: key,
        pattern: Some(pattern),
        permission: "allow".into(),
    })
}

/// wildcard match: * = any run, ? = any single char
pub fn wc(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    wc_rec(&p, 0, &t, 0)
}

fn wc_rec(p: &[char], mut pi: usize, t: &[char], mut ti: usize) -> bool {
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard() {
        assert!(wc("*", "anything"));
        assert!(wc("rm *", "rm -rf /tmp/x"));
        assert!(wc("git status*", "git status --short"));
        assert!(wc("git *", "git push"));
        assert!(!wc("git *", "hg push"));
        assert!(wc("*.rs", "src/main.rs"));
        assert!(!wc("*.rs", "src/main.rs.bak"));
        assert!(wc("a?c", "abc"));
        assert!(!wc("a?c", "ac"));
        assert!(wc("", ""));
        assert!(!wc("", "x"));
        assert!(wc("**", "a/b/c"));
        assert!(wc("cargo build", "cargo build"));
    }

    #[test]
    fn rules_and_defaults() {
        let cfg = PermCfg {
            rules: vec![PermRule {
                tool: "bash".into(),
                pattern: Some("git *".into()),
                permission: "allow".into(),
            }],
            bash: Some("ask".into()),
            ..Default::default()
        };
        assert_eq!(cfg.check("bash", r#"{"command":"git status"}"#), Perm::Allow);
        assert_eq!(cfg.check("bash", r#"{"command":"ls -la"}"#), Perm::Ask);
        assert_eq!(cfg.check("read_file", r#"{"path":"x"}"#), Perm::Allow);
        assert_eq!(cfg.check("write_file", r#"{"path":"x"}"#), Perm::Ask);
        assert_eq!(cfg.check("edit", r#"{"path":"x"}"#), Perm::Ask);

        let deny = PermCfg {
            rules: vec![PermRule {
                tool: "bash".into(),
                pattern: Some("rm *".into()),
                permission: "deny".into(),
            }],
            ..Default::default()
        };
        assert_eq!(deny.check("bash", r#"{"command":"rm -rf /"}"#), Perm::Deny);
        assert_eq!(deny.check("bash", r#"{"command":"echo hi"}"#), Perm::Ask);

        let mcp = PermCfg {
            mcp: Some("allow".into()),
            ..Default::default()
        };
        assert_eq!(mcp.check("mcp__fs__read", "{}"), Perm::Allow);
        assert_eq!(mcp.check("mcp__fs__write", "{}"), Perm::Allow);

        let mcp_rule = PermCfg {
            rules: vec![PermRule {
                tool: "mcp".into(),
                pattern: Some("fs__*".into()),
                permission: "deny".into(),
            }],
            ..Default::default()
        };
        assert_eq!(mcp_rule.check("mcp__fs__write", "{}"), Perm::Deny);
        assert_eq!(mcp_rule.check("mcp__web__get", "{}"), Perm::Ask);

        let wf = PermCfg {
            webfetch: Some("deny".into()),
            ..Default::default()
        };
        assert_eq!(wf.check("webfetch", r#"{"url":"https://x"}"#), Perm::Deny);
        assert_eq!(PermCfg::default().check("webfetch", r#"{"url":"https://x"}"#), Perm::Allow);
        assert_eq!(
            PermCfg::default().check("webfetch", r#"{"url":"https://evil.com"}"#),
            Perm::Allow
        );

        let wf_rule = PermCfg {
            rules: vec![PermRule {
                tool: "webfetch".into(),
                pattern: Some("https://evil.com/*".into()),
                permission: "deny".into(),
            }],
            ..Default::default()
        };
        assert_eq!(
            wf_rule.check("webfetch", r#"{"url":"https://evil.com/x"}"#),
            Perm::Deny
        );

        let sa = PermCfg {
            subagent: Some("ask".into()),
            ..Default::default()
        };
        assert_eq!(sa.check("subagent", r#"{"description":"d"}"#), Perm::Ask);
        assert_eq!(PermCfg::default().check("subagent", "{}"), Perm::Allow);
    }

    #[test]
    fn always_rules() {
        let r = derive_rule("bash", r#"{"command":"git push origin main"}"#).unwrap();
        assert_eq!(r.tool, "bash");
        assert_eq!(r.pattern.as_deref(), Some("git push *"));
        assert_eq!(r.permission, "allow");
        let r = derive_rule("edit", r#"{"path":"src/app.rs"}"#).unwrap();
        assert_eq!(r.pattern.as_deref(), Some("*.rs"));
        let cfg = PermCfg {
            rules: vec![r],
            ..Default::default()
        };
        assert_eq!(cfg.check("edit", r#"{"path":"src/other.rs"}"#), Perm::Allow);
        let r = derive_rule("webfetch", r#"{"url":"https://example.com/a?b"}"#).unwrap();
        assert_eq!(r.pattern.as_deref(), Some("https://example.com/*"));
        let r = derive_rule("mcp__fs__read", "{}").unwrap();
        assert_eq!(r.tool, "mcp");
        assert_eq!(r.pattern.as_deref(), Some("fs__*"));
        assert!(derive_rule("bash", r#"{"command":""}"#).is_none());
    }

    #[test]
    fn parsing() {
        assert_eq!(parse("allow"), Perm::Allow);
        assert_eq!(parse("DENY"), Perm::Deny);
        assert_eq!(parse("ask"), Perm::Ask);
        assert_eq!(parse("bogus"), Perm::Ask);
    }

    #[test]
    fn env_and_external_protection() {
        let cfg = PermCfg::default();
        assert_eq!(cfg.check("read_file", r#"{"path":".env"}"#), Perm::Ask);
        assert_eq!(cfg.check("read_file", r#"{"path":"config/.env.local"}"#), Perm::Ask);
        assert_eq!(cfg.check("read_file", r#"{"path":"prod.env"}"#), Perm::Ask);
        assert_eq!(
            cfg.check("read_file", r#"{"path":"config/.env.example"}"#),
            Perm::Allow
        );
        assert_eq!(cfg.check("read_file", r#"{"path":"src/main.rs"}"#), Perm::Allow);

        // an explicit user rule overrides the builtin
        let allow_env = PermCfg {
            rules: vec![PermRule {
                tool: "read_file".into(),
                pattern: Some("*.env*".into()),
                permission: "allow".into(),
            }],
            ..Default::default()
        };
        assert_eq!(allow_env.check("read_file", r#"{"path":".env"}"#), Perm::Allow);

        // outside the working directory -> ask, even for allowed tools
        let ext = std::env::temp_dir().join("hi-derola-perm-test.txt");
        let ext_json = format!(r#"{{"path":"{}"}}"#, ext.display());
        assert_eq!(cfg.check("read_file", &ext_json), Perm::Ask);
        let edit_json = format!(r#"{{"path":"{}"}}"#, ext.display());
        let allowed_edit = PermCfg {
            write_file: Some("allow".into()),
            edit: Some("allow".into()),
            ..Default::default()
        };
        assert_eq!(allowed_edit.check("edit", &edit_json), Perm::Ask);
        // internal paths stay allowed
        assert_eq!(allowed_edit.check("edit", r#"{"path":"src/lib.rs"}"#), Perm::Allow);
        // relative reads stay internal
        assert_eq!(cfg.check("list_files", r#"{"path":"."}"#), Perm::Allow);
    }
}
