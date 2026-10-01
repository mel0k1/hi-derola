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
