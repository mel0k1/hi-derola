pub mod agent;
pub mod agents;
pub mod app;
pub mod bg;
pub mod chat;
pub mod commands;
pub mod config;
pub mod diff;
pub mod files;
pub mod fmt;
pub mod lsp;
pub mod md;
pub mod models;
pub mod mcp;
pub mod perm;
pub mod provider;
pub mod search;
pub mod sessions;
pub mod skills;
pub mod snapshot;
pub mod todo;
pub mod tools;
pub mod ui;
pub mod web;

const MAX_AGENTS_MD: usize = 16 * 1024;

/// AGENTS.md/CLAUDE.md discovered in the directory chain of a read file
/// (up to the working directory), injected once per file per session
pub fn instructions_for_file(path: &str) -> Option<(String, String)> {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static SEEN: OnceLock<Mutex<HashSet<std::path::PathBuf>>> = OnceLock::new();
    const MAX_INLINE: usize = 4 * 1024;
    let cwd = std::env::current_dir().ok()?;
    let p = std::path::Path::new(path);
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    };
    let mut dir = abs.parent()?.to_path_buf();
    for _ in 0..6 {
        // strictly below the working directory: cwd-level AGENTS.md is already in the system prompt
        if !dir.starts_with(&cwd) || dir == cwd {
            break;
        }
        for name in ["AGENTS.md", "CLAUDE.md"] {
            let ip = dir.join(name);
            if !ip.is_file() {
                continue;
            }
            let seen = SEEN.get_or_init(|| Mutex::new(HashSet::new()));
            if !seen.lock().unwrap().insert(ip.clone()) {
                return None;
            }
            let body = std::fs::read_to_string(&ip).ok()?;
            let mut text = body.trim().to_string();
            if text.is_empty() {
                continue;
            }
            if text.len() > MAX_INLINE {
                let mut end = MAX_INLINE;
                while end < text.len() && !text.is_char_boundary(end) {
                    end += 1;
                }
                text.truncate(end);
                text.push_str("\n... (truncated)");
            }
            return Some((ip.display().to_string(), text));
        }
        let parent = dir.parent()?.to_path_buf();
        dir = parent;
    }
    None
}

pub fn agents_md() -> String {
    let cwd = std::env::current_dir().unwrap_or_default();
    agents_md_in(&cwd)
}

pub fn agents_md_in(base: &std::path::Path) -> String {
    let mut dir = Some(base.to_path_buf());
    for _ in 0..8 {
        let Some(d) = dir else { break };
        for name in ["AGENTS.md", "CLAUDE.md"] {
            let p = d.join(name);
            let Ok(meta) = std::fs::metadata(&p) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            if let Ok(s) = std::fs::read_to_string(&p) {
                let s = s.trim();
                if s.is_empty() {
                    continue;
                }
                let mut text = s.to_string();
                if text.len() > MAX_AGENTS_MD {
                    let mut end = MAX_AGENTS_MD;
                    while end < text.len() && !text.is_char_boundary(end) {
                        end += 1;
                    }
                    text.truncate(end);
                    text.push_str("\n... (truncated)");
                }
                return format!(
                    "Project instructions from {} ({}):\n{}",
                    name,
                    d.display(),
                    text
                );
            }
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_md_lookup() {
        let dir = std::env::temp_dir().join(format!("hiderola-agents-{}", std::process::id()));
        let sub = dir.join("deep");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(agents_md_in(&dir), "");
        std::fs::write(dir.join("AGENTS.md"), "# rules\nbe nice\n").unwrap();
        let found = agents_md_in(&dir);
        assert!(found.contains("be nice"));
        assert!(found.starts_with("Project instructions from AGENTS.md"));
        let up = agents_md_in(&sub);
        assert!(up.contains("be nice"));
        std::fs::write(dir.join("CLAUDE.md"), "claude only\n").unwrap();
        std::fs::remove_file(dir.join("AGENTS.md")).unwrap();
        assert!(agents_md_in(&sub).contains("claude only"));
        std::fs::write(dir.join("AGENTS.md"), "x".repeat(MAX_AGENTS_MD + 100)).unwrap();
        let t = agents_md_in(&dir);
        assert!(t.contains("(truncated)"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
