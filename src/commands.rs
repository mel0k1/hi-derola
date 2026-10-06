use std::path::PathBuf;

use crate::chat::{Message, Role};

/// Prompt for /init: inspect the project and write or improve AGENTS.md.
pub fn init_prompt(cwd: &str) -> String {
    format!(
        "Initialize or improve the project instructions for this repository ({cwd}).\n\n\
         1. Explore the codebase: README, manifests (Cargo.toml, package.json, pyproject.toml, go.mod, ...), \
         directory layout, build/test commands, code style signals.\n\
         2. If AGENTS.md does not exist, create it in the project root. If it exists, improve it in place: \
         fix what is outdated, add what is missing, keep it concise.\n\
         3. Cover: project overview (2-3 sentences), commands to build/lint/test, code style rules, \
         directory structure, anything a coding agent must know. Plain markdown, no fluff."
    )
}

/// Render a session as markdown for /export.
pub fn export_markdown(title: &str, msgs: &[Message]) -> String {
    let mut out = format!(
        "# {}\n",
        if title.trim().is_empty() {
            "hi-derola session"
        } else {
            title
        }
    );
    for m in msgs {
        match m.role {
            Role::User => out.push_str(&format!("\n## user\n\n{}\n", m.content.trim())),
            Role::Assistant => {
                let body = m.content.trim();
                if !body.is_empty() {
                    out.push_str(&format!("\n## assistant\n\n{body}\n"));
                }
                for c in &m.tool_calls {
                    let args: String = c
                        .args
                        .lines()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .take(80)
                        .collect();
                    out.push_str(&format!("\n*tool call: `{}` — {}*\n", c.name, args));
                }
            }
            Role::Tool => {}
        }
    }
    out
}

#[derive(Clone, Debug)]
pub struct Cmd {
    pub name: String,
    pub description: String,
    pub template: String,
}

fn project_dir() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    Some(cwd.join(".hi-derola").join("commands"))
}

fn global_dir() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("hi-derola").join("commands"))
}

fn scan_dir(dir: &PathBuf, out: &mut Vec<Cmd>) {
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
        let (description, template) = split(&text);
        out.push(Cmd {
            name,
            description,
            template,
        });
    }
}

/// Project commands win over global ones with the same name.
pub fn discover() -> Vec<Cmd> {
    let mut out = Vec::new();
    if let Some(d) = project_dir() {
        scan_dir(&d, &mut out);
    }
    if let Some(d) = global_dir() {
        scan_dir(&d, &mut out);
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|c| seen.insert(c.name.clone()));
    out
}

pub fn get(name: &str) -> Option<Cmd> {
    discover().into_iter().find(|c| c.name == name)
}

fn split(text: &str) -> (String, String) {
    let rest = match text.strip_prefix("---") {
        Some(rest) => match rest.find("\n---") {
            Some(pos) => {
                let (front, body) = rest.split_at(pos + 1);
                let body = body.trim_start_matches(['-', '-']).trim_start_matches('\n');
                let mut description = String::new();
                for line in front.lines() {
                    if let Some(v) = line.strip_prefix("description:") {
                        description = v.trim().trim_matches('"').to_string();
                    }
                }
                return (description, body.to_string());
            }
            None => text,
        },
        None => text,
    };
    (String::new(), rest.trim_start().to_string())
}

/// Replace $ARGUMENTS with the full argument string and $1..$9 with positional parts.
pub fn render(template: &str, args: &str) -> String {
    let args = args.trim();
    let positional: Vec<&str> = args.split_whitespace().collect();
    let mut out = template.to_string();
    if out.contains("$ARGUMENTS") {
        out = out.replace("$ARGUMENTS", args);
    }
    for (i, p) in positional.iter().enumerate().take(9) {
        out = out.replace(&format!("${}", i + 1), p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_placeholders() {
        let t = "fix issue $1 in $2, details: $ARGUMENTS";
        assert_eq!(
            render(t, "42 backend"),
            "fix issue 42 in backend, details: 42 backend"
        );
        assert_eq!(render("no placeholders", "x"), "no placeholders");
    }

    #[test]
    fn split_frontmatter() {
        let (d, b) = split("---\ndescription: \"Do stuff\"\n---\nBody $ARGUMENTS\n");
        assert_eq!(d, "Do stuff");
        assert_eq!(b, "Body $ARGUMENTS\n");
        let (d, b) = split("just body");
        assert_eq!(d, "");
        assert_eq!(b, "just body");
    }
}
