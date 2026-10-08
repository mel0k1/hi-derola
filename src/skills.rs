//! skills: drop a folder with a SKILL.md into `.hi-derola/skills/` (project)
//! or `~/.config/hi-derola/skills/` (global) and the agent can load it.
//!
//! discovery walks both roots recursively (skips .git/node_modules/target,
//! caps the walk), so a whole skill collection cloned from github registers
//! at once: `skills install <git-url>` does the clone. every skill can be
//! toggled off — the set of disabled names persists in skills.json next to
//! the global root and the disabled ones vanish from the tool spec.

use anyhow::{bail, Context as _, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const MAX_SKILL_BODY: usize = 16 * 1024;
const MAX_WALK_DEPTH: usize = 8;
const MAX_SKILLS: usize = 500;

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

fn toggles_file() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("hi-derola").join("skills.json"))
}

/// the disabled set lives on disk (survives restarts) and in a process-wide
/// mutex (set_enabled keeps both in sync, so toggles act immediately)
fn disabled() -> &'static Mutex<Vec<String>> {
    static D: Mutex<Vec<String>> = Mutex::new(Vec::new());
    static LOADED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    let _ = LOADED.get_or_init(|| {
        let list = toggles_file()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .and_then(|v| {
                v.get("disabled").and_then(|d| d.as_array()).map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
            })
            .unwrap_or_default();
        *D.lock().unwrap() = list;
    });
    &D
}

fn save_disabled(list: &[String]) -> Result<()> {
    let Some(p) = toggles_file() else {
        bail!("no config dir");
    };
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let raw = json!({ "disabled": list });
    std::fs::write(&p, serde_json::to_string_pretty(&raw)?)?;
    Ok(())
}

pub fn is_enabled(name: &str) -> bool {
    !disabled().lock().unwrap().iter().any(|n| n == name)
}

pub fn set_enabled(name: &str, enabled: bool) -> Result<()> {
    let mut list = disabled().lock().unwrap();
    if enabled {
        list.retain(|n| n != name);
    } else if !list.iter().any(|n| n == name) {
        list.push(name.to_string());
    }
    save_disabled(&list)
}

fn ignored_dir(name: &str) -> bool {
    matches!(
        name,
        ".git" | "node_modules" | "target" | "dist" | "build" | "out" | ".hi-derola"
    )
}

/// recursive SKILL.md discovery under one root (depth-capped, ignores
/// vendored dirs, so symlink loops and huge trees cannot wedge the scan)
fn scan_dir(root: &Path, out: &mut Vec<Skill>, depth: usize) {
    if depth > MAX_WALK_DEPTH || out.len() >= MAX_SKILLS {
        return;
    }
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    for e in rd.flatten() {
        if out.len() >= MAX_SKILLS {
            return;
        }
        let path = e.path();
        let Some(name) = e.file_name().to_str().map(String::from) else {
            continue;
        };
        if path.is_dir() {
            if ignored_dir(&name) {
                continue;
            }
            scan_dir(&path, out, depth + 1);
            continue;
        }
        if name != "SKILL.md" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some((sname, description, _)) = parse(&text) else {
            continue;
        };
        let name = if sname.is_empty() {
            path.parent()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        } else {
            sname
        };
        out.push(Skill {
            name,
            description,
            path,
        });
    }
}

/// every discovered skill (enabled and disabled), project wins over global
/// on a name clash
pub fn discover_all() -> Vec<Skill> {
    let mut out = Vec::new();
    if let Some(d) = project_dir() {
        scan_dir(&d, &mut out, 0);
    }
    if let Some(d) = global_dir() {
        scan_dir(&d, &mut out, 0);
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|s| seen.insert(s.name.clone()));
    out
}

/// the enabled set only — what the agent sees
pub fn discover() -> Vec<Skill> {
    discover_all()
        .into_iter()
        .filter(|s| is_enabled(&s.name))
        .collect()
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
    if !is_enabled(name) {
        bail!("skill \"{name}\" is disabled — /skills toggle {name}");
    }
    let skill = discover_all()
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

/// shallow-clone a skill collection into the global root; a re-install of
/// the same repo fast-forwards it. any repo layout works — every SKILL.md
/// below the clone is picked up by discovery
pub fn install_from_git(url: &str) -> Result<String> {
    let url = url.trim().trim_end_matches('/');
    if url.is_empty() {
        bail!("usage: /skills install <git-url>");
    }
    let Some(root) = global_dir() else {
        bail!("no config dir");
    };
    std::fs::create_dir_all(&root)?;
    let name = url
        .rsplit(['/', ':'])
        .next()
        .unwrap_or("skills")
        .trim_end_matches(".git")
        .to_string();
    if name.is_empty() || name.contains("..") {
        bail!("bad repo name from \"{url}\"");
    }
    let dest = root.join(&name);
    let out = if dest.join(".git").exists() {
        std::process::Command::new("git")
            .arg("-C")
            .arg(&dest)
            .args(["pull", "--ff-only"])
            .output()
    } else {
        std::process::Command::new("git")
            .args(["clone", "--depth", "1", url])
            .arg(&dest)
            .output()
    }
    .context("run git")?;
    if !out.status.success() {
        bail!(
            "git failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let mut found = Vec::new();
    scan_dir(&dest, &mut found, 0);
    Ok(format!(
        "installed \"{name}\" — {} skill{} discovered:\n{}",
        found.len(),
        if found.len() == 1 { "" } else { "s" },
        found
            .iter()
            .map(|s| format!(
                "  {}{}",
                s.name,
                if s.description.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", s.description)
                }
            ))
            .collect::<Vec<_>>()
            .join("\n")
    ))
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

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_skill(root: &Path, rel: &str, name: &str, desc: &str) {
        let p = root.join(rel).join("SKILL.md");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(
            p,
            format!("---\nname: {name}\ndescription: \"{desc}\"\n---\nBody\n"),
        )
        .unwrap();
    }

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
    fn recursive_discovery_and_toggles() {
        let dir = tmpdir("hiderola-skill");
        write_skill(&dir, ".hi-derola/skills/review", "review", "Review code");
        write_skill(
            &dir,
            ".hi-derola/skills/collection/commit",
            "commit",
            "How to commit",
        );
        write_skill(
            &dir,
            ".hi-derola/skills/collection/deep/nested",
            "nested",
            "Deep one",
        );
        // a skill inside an ignored dir is not discovered
        write_skill(
            &dir,
            ".hi-derola/skills/collection/node_modules/x",
            "x",
            "no",
        );
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();

        let all = discover_all();
        let names: Vec<&str> = all.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"review"), "{names:?}");
        assert!(names.contains(&"commit"));
        assert!(names.contains(&"nested"));
        assert!(!names.contains(&"x"));

        assert!(load("review").unwrap().contains("Body"));
        assert!(load("commit").is_ok());

        // toggle off hides it from discovery and blocks load, back on restores
        set_enabled("commit", false).unwrap();
        assert!(!is_enabled("commit"));
        assert!(!discover().iter().any(|s| s.name == "commit"));
        assert!(load("commit").is_err());
        let raw = std::fs::read_to_string(toggles_file().unwrap()).unwrap();
        assert!(raw.contains("commit"));
        set_enabled("commit", true).unwrap();
        assert!(is_enabled("commit"));
        assert!(load("commit").is_ok());

        std::env::set_current_dir(prev).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_from_local_git_repo() {
        let dir = tmpdir("hiderola-skill-git");
        let repo = dir.join("repo");
        std::fs::create_dir_all(repo.join("alpha")).unwrap();
        std::fs::create_dir_all(repo.join("beta")).unwrap();
        write_skill(&repo, "alpha", "alpha", "First");
        write_skill(&repo, "beta", "beta", "Second");
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        git(&["add", "-A"]);
        git(&["commit", "-qm", "init"]);

        let prev = std::env::current_dir().unwrap();
        let gdir = dir.join("global/hi-derola/skills");
        std::fs::create_dir_all(&gdir).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.join("global"));
        std::env::set_current_dir(&dir).unwrap();
        let msg = install_from_git(repo.to_str().unwrap()).unwrap();
        assert!(msg.contains("2 skills"), "{msg}");
        assert!(gdir.join("repo/alpha/SKILL.md").exists());
        // reinstall fast-forwards instead of failing
        let msg2 = install_from_git(repo.to_str().unwrap()).unwrap();
        assert!(msg2.contains("2 skills"));
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::set_current_dir(prev).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
