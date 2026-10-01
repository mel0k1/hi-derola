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
        let field = match key.as_str() {
            "edit" => self.edit.as_deref(),
            "write_file" => self.write_file.as_deref(),
            "bash" => self.bash.as_deref(),
            "mcp" => self.mcp.as_deref(),
            "webfetch" => self.webfetch.as_deref(),
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
        "edit" | "write_file" | "bash" => {
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
        other => (other.to_string(), String::new()),
    }
}

fn default_perm(key: &str) -> Perm {
    match key {
        "write_file" | "edit" | "bash" | "mcp" => Perm::Ask,
        _ => Perm::Allow,
    }
}

fn parse(s: &str) -> Perm {
    match s.trim().to_lowercase().as_str() {
        "allow" => Perm::Allow,
        "deny" => Perm::Deny,
        _ => Perm::Ask,
    }
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
    fn parsing() {
        assert_eq!(parse("allow"), Perm::Allow);
        assert_eq!(parse("DENY"), Perm::Deny);
        assert_eq!(parse("ask"), Perm::Ask);
        assert_eq!(parse("bogus"), Perm::Ask);
    }
}
