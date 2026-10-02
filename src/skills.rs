use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::path::PathBuf;

const MAX_SKILL_BODY: usize = 16 * 1024;

#[derive(Clone, Debug)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

fn project_dir() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    Some(cwd.join(".hi-derola").join("skills"))
}

fn global_dir() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("hi-derola").join("skills"))
}

fn scan_dir(dir: &PathBuf, out: &mut Vec<Skill>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let file = e.path().join("SKILL.md");
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Some((name, description, _)) = parse(&text) else {
            continue;
        };
        let name = if name.is_empty() {
            e.file_name().to_string_lossy().to_string()
        } else {
            name
        };
        out.push(Skill {
            name,
            description,
            path: file,
        });
    }
}

/// Skills from the project dir win over global ones with the same name.
pub fn discover() -> Vec<Skill> {
    let mut out = Vec::new();
    if let Some(d) = project_dir() {
        scan_dir(&d, &mut out);
    }
    if let Some(d) = global_dir() {
        scan_dir(&d, &mut out);
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|s| seen.insert(s.name.clone()));
    out
}

fn parse(text: &str) -> Option<(String, String, String)> {
    let rest = text.strip_prefix("---")?;
    let end = rest.find("\n---")?;
    let (front, body) = rest.split_at(end + 1);
    let body = body.trim_start_matches(['-', '-']).trim_start_matches('\n');
    let mut name = String::new();
    let mut description = String::new();
    for line in front.lines() {
        if let Some(v) = line.strip_prefix("name:") {
            name = v.trim().trim_matches('"').to_string();
        } else if let Some(v) = line.strip_prefix("description:") {
            description = v.trim().trim_matches('"').to_string();
        }
    }
    Some((name, description, body.to_string()))
}

pub fn load(name: &str) -> Result<String> {
    let skill = discover()
        .into_iter()
        .find(|s| s.name == name)
        .ok_or_else(|| anyhow::anyhow!("skill not found: {name}"))?;
    let text = std::fs::read_to_string(&skill.path)
        .map_err(|e| anyhow::anyhow!("{}: {e}", skill.path.display()))?;
    let Some((_, _, body)) = parse(&text) else {
        bail!("skill file has no frontmatter: {}", skill.path.display());
    };
    let mut body = body;
    if body.len() > MAX_SKILL_BODY {
        let mut end = MAX_SKILL_BODY;
        while end < body.len() && !body.is_char_boundary(end) {
            end += 1;
        }
        body.truncate(end);
        body.push_str("\n... (truncated)");
    }
    Ok(body)
}

pub fn spec_description() -> String {
    let skills = discover();
    if skills.is_empty() {
        return String::new();
    }
    let list: Vec<String> = skills
        .iter()
        .map(|s| {
            if s.description.is_empty() {
                s.name.clone()
            } else {
                format!("{}: {}", s.name, s.description)
            }
        })
        .collect();
    format!(
        "Load a skill: a reusable instruction file that teaches a specific workflow. \
         Call it with the skill name right before doing that kind of work. \
         Available skills:\n{}",
        list.join("\n")
    )
}

pub fn detail(args: &str) -> String {
    let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
    v["name"].as_str().unwrap_or("skill").to_string()
}

pub fn spec() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": {"type": "string", "description": "Skill name, see the list in the description"}
        },
        "required": ["name"]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_frontmatter() {
        let text = "---\nname: commit\ndescription: \"How to commit\"\n---\nBody here\n";
        let (name, desc, body) = parse(text).unwrap();
        assert_eq!(name, "commit");
        assert_eq!(desc, "How to commit");
        assert_eq!(body, "Body here\n");
        assert!(parse("no frontmatter").is_none());
    }

    #[test]
    fn discover_and_load() {
        let dir = std::env::temp_dir().join(format!("hiderola-skill-{}", std::process::id()));
        let skill_dir = dir.join(".hi-derola/skills/review");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: review\ndescription: Review code\n---\nCheck tests first.\n",
        )
        .unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();
        let skills = discover();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "review");
        assert_eq!(skills[0].description, "Review code");
        assert_eq!(load("review").unwrap(), "Check tests first.\n");
        assert!(load("nope").is_err());
        std::env::set_current_dir(prev).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
