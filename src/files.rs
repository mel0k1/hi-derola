use anyhow::{bail, Result};

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

pub fn parse_write_blocks(text: &str) -> Vec<WriteBlock> {
    let mut blocks = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let Some(info) = line.strip_prefix("```") else {
            continue;
        };
        let info = info.trim();
        if !is_path(info) {
            continue;
        }
        let mut content = String::new();
        let mut closed = false;
        for l in lines.by_ref() {
            if l.starts_with("```") {
                closed = true;
                break;
            }
            content.push_str(l);
            content.push('\n');
        }
        if closed {
            blocks.push(WriteBlock {
                path: info.to_string(),
                content,
            });
        }
    }
    blocks
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

fn is_path(s: &str) -> bool {
    !s.is_empty()
        && !s.chars().any(char::is_whitespace)
        && (s.contains('/') || s.contains('.'))
}
