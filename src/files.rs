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
