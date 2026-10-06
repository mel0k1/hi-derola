use std::path::PathBuf;

#[derive(Clone, Debug, Default)]
pub struct AgentDecl {
    pub name: String,
    pub description: String,
    pub model: Option<String>,
    pub temperature: Option<f64>,
    pub read_only: bool,
    pub prompt: String,
}

fn project_dir() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    Some(cwd.join(".hi-derola").join("agents"))
}

fn global_dir() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("hi-derola").join("agents"))
}

fn scan_dir(dir: &PathBuf, out: &mut Vec<AgentDecl>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("md") {
            continue;
        }
        let Some(text) = std::fs::read_to_string(&path).ok() else {
            continue;
        };
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let (front, body) = split_frontmatter(&text);
        out.push(AgentDecl {
            name,
            description: front.0,
            model: front.1,
            temperature: front.2,
            read_only: front.3,
            prompt: body,
        });
    }
}

/// Custom agents from the project dir win over global ones with the same name.
pub fn discover() -> Vec<AgentDecl> {
    let mut out = Vec::new();
    if let Some(d) = project_dir() {
        scan_dir(&d, &mut out);
    }
    if let Some(d) = global_dir() {
        scan_dir(&d, &mut out);
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|a| seen.insert(a.name.clone()));
    out
}

/// Resolve an agent by name: custom declarations first, then built-in profiles.
pub fn get(name: &str) -> Option<AgentDecl> {
    let name = name.trim();
    if let Some(a) = discover().into_iter().find(|a| a.name == name) {
        return Some(a);
    }
    match name {
        "general" => Some(AgentDecl {
            name: "general".into(),
            description: "General-purpose agent for research and multi-step tasks with full tool access".into(),
            ..Default::default()
        }),
        "explore" => Some(AgentDecl {
            name: "explore".into(),
            description: "Fast read-only agent for exploring codebases: find files, search code, answer questions".into(),
            read_only: true,
            ..Default::default()
        }),
        _ => None,
    }
}

/// If the text starts with @<agent> and the agent exists, return (agent, rest).
pub fn split_mention(text: &str) -> Option<(String, String)> {
    let t = text.trim_start();
    let t = t.strip_prefix('@')?;
    let (name, rest) = t.split_once(char::is_whitespace)?;
    let name = name.trim().trim_end_matches(',');
    if name.is_empty() {
        return None;
    }
    get(name)?;
    Some((name.to_string(), rest.trim().to_string()))
}

pub fn list_for_spec() -> String {
    let mut items: Vec<String> = discover()
        .into_iter()
        .map(|a| {
            if a.description.is_empty() {
                a.name
            } else {
                format!("{}: {}", a.name, a.description)
            }
        })
        .collect();
    items.push("general: full tool access for research and multi-step tasks".into());
    items.push("explore: read-only codebase exploration".into());
    items.join("\n")
}

type Front = (String, Option<String>, Option<f64>, bool);

fn split_frontmatter(text: &str) -> (Front, String) {
    let empty = (String::new(), None, None, false);
    let rest = match text.strip_prefix("---") {
        Some(rest) => rest,
        None => return (empty, text.trim_start().to_string()),
    };
    let end = match rest.find("\n---") {
        Some(pos) => pos,
        None => return (empty, text.trim_start().to_string()),
    };
    let (front, body) = rest.split_at(end);
    let body = body[3..].trim_start_matches(['-', '\n']).to_string();
    let mut description = String::new();
    let mut model = None;
    let mut temperature = None;
    let mut read_only = false;
    for line in front.lines() {
        if let Some(v) = line.strip_prefix("description:") {
            description = v.trim().trim_matches('"').to_string();
        } else if let Some(v) = line.strip_prefix("model:") {
            model = Some(v.trim().trim_matches('"').to_string()).filter(|s| !s.is_empty());
        } else if let Some(v) = line.strip_prefix("temperature:") {
            temperature = v.trim().parse::<f64>().ok();
        } else if let Some(v) = line.strip_prefix("read_only:") {
            read_only = v.trim().eq_ignore_ascii_case("true");
        }
    }
    (
        (description, model, temperature, read_only),
        body.trim().to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_decl() {
        let text = "---\ndescription: \"Reviews code\"\nmodel: gpt-5-mini\ntemperature: 0.2\nread_only: true\n---\nYou are a reviewer.\n";
        let (front, body) = split_frontmatter(text);
        assert_eq!(front.0, "Reviews code");
        assert_eq!(front.1.as_deref(), Some("gpt-5-mini"));
        assert_eq!(front.2, Some(0.2));
        assert!(front.3);
        assert_eq!(body, "You are a reviewer.");

        let (front, body) = split_frontmatter("just a prompt");
        assert_eq!(front.0, "");
        assert_eq!(body, "just a prompt");
    }

    #[test]
    fn builtins_and_mentions() {
        assert!(get("explore").unwrap().read_only);
        assert!(!get("general").unwrap().read_only);
        assert!(get("no-such-agent").is_none());

        let (name, rest) = split_mention("@explore find the parser").unwrap();
        assert_eq!(name, "explore");
        assert_eq!(rest, "find the parser");
        assert!(split_mention("@nothere do stuff").is_none());
        assert!(split_mention("plain text @explore").is_none());
    }
}
