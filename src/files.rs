use anyhow::{bail, Result};

#[derive(Clone)]
pub struct WriteBlock {
    pub path: String,
    pub content: String,
}

const MAX_ATTACH_BYTES: u64 = 128 * 1024;

pub fn read_attach(path: &str) -> Result<String> {
    let meta = std::fs::metadata(path).map_err(|e| anyhow::anyhow!("{path}: {e}"))?;
    if meta.len() > MAX_ATTACH_BYTES {
        bail!("{path}: too large ({} bytes)", meta.len());
    }
    std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("{path}: {e}"))
}

pub fn apply(block: &WriteBlock) -> Result<usize> {
    let path = std::path::Path::new(&block.path);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow::anyhow!("{}: {e}", block.path))?;
        }
    }
    std::fs::write(path, &block.content).map_err(|e| anyhow::anyhow!("{}: {e}", block.path))?;
    Ok(block.content.lines().count())
}

pub fn norm(p: &str) -> String {
    if std::path::MAIN_SEPARATOR == '/' {
        p.to_string()
    } else {
        p.replace(std::path::MAIN_SEPARATOR, "/")
    }
}

const MAX_WALK: usize = 4000;
const MAX_DEPTH: usize = 8;
const MAX_MENTIONS: usize = 8;

fn token_path(token: &str) -> Option<String> {
    let t = token.trim_end_matches(['.', ',', ';', ':', ')', ']', '!']);
    if t.is_empty() || t.len() > 256 {
        return None;
    }
    Some(t.to_string())
}

pub fn mentions_in(base: &std::path::Path, text: &str) -> (String, Vec<String>, Vec<String>) {
    let mut blocks = String::new();
    let mut attached = Vec::new();
    let mut missing = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'@' || (i > 0 && (bytes[i - 1] as char).is_alphanumeric()) {
            i += 1;
            continue;
        }
        let start = i + 1;
        let mut end = start;
        while end < bytes.len() {
            let c = bytes[end] as char;
            if c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '\\' | '~' | '+') {
                end += 1;
            } else {
                break;
            }
        }
        if let Some(p) = token_path(&text[start..end]) {
            let full = if p.starts_with('~') {
                if p == "~" {
                    dirs::home_dir().unwrap_or_else(|| base.to_path_buf())
                } else if let Some(rest) = p.strip_prefix("~/") {
                    match dirs::home_dir() {
                        Some(h) => h.join(rest),
                        None => base.join(&p),
                    }
                } else {
                    base.join(&p)
                }
            } else if std::path::Path::new(&p).is_absolute() {
                std::path::PathBuf::from(&p)
            } else {
                base.join(&p)
            };
            match read_attach(&full.display().to_string()) {
                Ok(content) if attached.len() < MAX_MENTIONS => {
                    blocks.push_str(&format!("[file: {p}]\n{content}\n\n"));
                    attached.push(p);
                }
                Ok(_) => missing.push(p),
                Err(_) => missing.push(p),
            }
        }
        i = end.max(start);
    }
    (blocks, attached, missing)
}

pub fn mentions(text: &str) -> (String, Vec<String>, Vec<String>) {
    let cwd = std::env::current_dir().unwrap_or_default();
    mentions_in(&cwd, text)
}

pub fn walk_files(dir: &str, depth: usize, out: &mut Vec<String>) {
    if depth > MAX_DEPTH || out.len() >= MAX_WALK {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if out.len() >= MAX_WALK {
            return;
        }
        let name = e.file_name().to_string_lossy().to_string();
        if matches!(name.as_str(), ".git" | "target" | "node_modules") {
            continue;
        }
        let path = e.path();
        if path.is_dir() {
            walk_files(&path.display().to_string(), depth + 1, out);
        } else if path.is_file() {
            out.push(norm(&path.display().to_string()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mention_expansion() {
        let dir = std::env::temp_dir().join(format!("hiderola-mention-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\n").unwrap();

        let (blocks, got, miss) = mentions_in(&dir, "check @src/main.rs and @Cargo.toml please");
        assert_eq!(got, vec!["src/main.rs", "Cargo.toml"]);
        assert!(miss.is_empty());
        assert!(blocks.contains("[file: src/main.rs]\nfn main() {}\n"));
        assert!(blocks.contains("[file: Cargo.toml]\n[package]\n"));

        let (_, got, miss) = mentions_in(&dir, "no mentions here");
        assert!(got.is_empty() && miss.is_empty());

        let (blocks, got, miss) = mentions_in(&dir, "mail me at user@example.com about @nope.txt");
        assert!(got.is_empty());
        assert_eq!(miss, vec!["nope.txt"]);
        assert!(blocks.is_empty());

        let (_, got, _) = mentions_in(&dir, "trailing @Cargo.toml, and @src/main.rs.");
        assert_eq!(got, vec!["Cargo.toml", "src/main.rs"]);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
