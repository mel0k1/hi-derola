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
pub mod patch;
pub mod perm;
pub mod provider;
pub mod search;
pub mod sessions;
pub mod skills;
pub mod snapshot;
pub mod todo;
pub mod tools;
pub mod ui;
#[cfg(windows)]
mod winjob;
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

/// days since the unix epoch -> (year, month, day); Howard Hinnant's civil_from_days
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// today's date as YYYY-MM-DD (UTC)
pub fn today_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// true if base or any of its ancestors contains a .git entry
/// (a repository directory or a worktree file)
pub fn is_git_repo_in(base: &std::path::Path) -> bool {
    let mut dir = Some(base.to_path_buf());
    for _ in 0..32 {
        let Some(d) = dir else { break };
        if d.join(".git").exists() {
            return true;
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
    false
}

/// the shared system prompt for both frontends; venue is
/// "in the user's terminal" (TUI) or "on the user's machine" (GUI)
pub fn base_prompt_in(base: &std::path::Path, venue: &str) -> String {
    let repo = if is_git_repo_in(base) { "yes" } else { "no" };
    let mut system = format!(
        "You are hi-derola, a coding assistant running {venue}.\n\
         <env>\n\
         Working directory: {}\n\
         Is directory a git repo: {repo}\n\
         Platform: {} ({})\n\
         </env>\n\
         Today's date: {}.\n\
         Be concise and practical. Use markdown for formatting.\n\n\
         Use the provided tools to work with files and run commands instead of printing code \
         fences with file contents. Use glob and grep to locate code before reading. \
         Prefer read_file before modifying a file. \
         write_file writes the complete file content.",
        base.display(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        today_iso(),
    );
    let agents = agents_md_in(base);
    if !agents.is_empty() {
        system.push_str("\n\n");
        system.push_str(&agents);
    }
    system
}

pub fn base_prompt(venue: &str) -> String {
    let cwd = std::env::current_dir().unwrap_or_default();
    base_prompt_in(&cwd, venue)
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

    #[test]
    fn civil_date_anchors() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 1970 is not a leap year: day 59 is Mar 1, day 60 is Mar 2
        assert_eq!(civil_from_days(59), (1970, 3, 1));
        assert_eq!(civil_from_days(60), (1970, 3, 2));
        // 2000 IS a leap year: Feb 29 exists
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(20_454), (2026, 1, 1));
        assert_eq!(civil_from_days(20_729), (2026, 10, 3));
        assert_eq!(today_iso().len(), 10);
    }

    #[test]
    fn env_prompt_and_git_detection() {
        let dir = std::env::temp_dir().join(format!("hiderola-env-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = base_prompt_in(&dir, "in the user's terminal");
        assert!(p.contains("running in the user's terminal"));
        assert!(p.contains("Is directory a git repo: no"));
        assert!(p.contains(&format!(
            "Platform: {} ({})",
            std::env::consts::OS,
            std::env::consts::ARCH
        )));
        assert!(p.contains("Today's date: 2"));
        assert!(p.contains("<env>"));
        assert!(!p.contains("Project instructions"));
        // a worktree-style .git file counts as a repo too
        std::fs::write(dir.join(".git"), "gitdir: /elsewhere\n").unwrap();
        assert!(is_git_repo_in(&dir));
        let p = base_prompt_in(&dir, "on the user's machine");
        assert!(p.contains("Is directory a git repo: yes"));
        assert!(p.contains("running on the user's machine"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
