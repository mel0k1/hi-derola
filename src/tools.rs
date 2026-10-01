use anyhow::{bail, Result};
use serde_json::{json, Value};

use crate::files::{self, WriteBlock};
use crate::mcp::McpClient;
use crate::provider::ToolSpec;

const MAX_LIST: usize = 500;
const MAX_CAPTURE: usize = 256 * 1024;
const READ_LIMIT: usize = 2000;

pub fn budget(out: String, max: usize) -> String {
    if max == 0 || out.len() <= max {
        return out;
    }
    tail(&out, max)
}

fn tail(s: &str, max: usize) -> String {
    let mut start = s.len() - max;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    let dropped: usize = s[..start].chars().count();
    format!("... ({} chars truncated from the head)\n{}", dropped, &s[start..])
}

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "read_file".into(),
            description: "Read a UTF-8 text file with line numbers (1-based, cat -n style). Returns up to limit lines starting at offset. Binary files are detected and not dumped.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path, relative to the working directory or absolute"},
                    "offset": {"type": "integer", "description": "First line to read, 1-based, default 1"},
                    "limit": {"type": "integer", "description": "Max lines to return, default 2000"}
                },
                "required": ["path"]
            }),
        },
        ToolSpec {
            name: "write_file".into(),
            description: "Create or overwrite a file with the complete content. Parent directories are created automatically.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path"},
                    "content": {"type": "string", "description": "Complete file content"}
                },
                "required": ["path", "content"]
            }),
        },
        ToolSpec {
            name: "edit".into(),
            description: "Perform exact string replacement in an existing file. old_str must match the file content exactly and be unique unless replace_all is true. Read the file first.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path"},
                    "old_str": {"type": "string", "description": "Exact text to replace"},
                    "new_str": {"type": "string", "description": "Replacement text"},
                    "replace_all": {"type": "boolean", "description": "Replace every occurrence, default false"}
                },
                "required": ["path", "old_str", "new_str"]
            }),
        },
        ToolSpec {
            name: "list_files".into(),
            description: "List files and directories recursively, up to 3 levels deep.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Directory path, defaults to the working directory"}
                }
            }),
        },
        ToolSpec {
            name: "glob".into(),
            description: "Find files by glob pattern. Returns up to 100 paths.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Glob pattern, e.g. **/*.rs, supports **, *, ?, [class], {a,b}"},
                    "path": {"type": "string", "description": "Directory to search, defaults to the working directory"}
                },
                "required": ["pattern"]
            }),
        },
        ToolSpec {
            name: "grep".into(),
            description: "Search file contents with a regular expression. Returns up to 100 matches grouped by file.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Regular expression to search for"},
                    "path": {"type": "string", "description": "Directory to search, defaults to the working directory"},
                    "include": {"type": "string", "description": "Optional glob filter for file names, e.g. *.ts or *.{h,cpp}"}
                },
                "required": ["pattern"]
            }),
        },
        ToolSpec {
            name: "bash".into(),
            description: "Run a shell command (sh on unix, cmd on Windows) and return stdout/stderr combined. Long output keeps only the tail. Exit code is added on failure.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "Shell command to execute"},
                    "workdir": {"type": "string", "description": "Optional working directory for the command"},
                    "timeout": {"type": "integer", "description": "Timeout in seconds, default 120, max 600"}
                },
                "required": ["command"]
            }),
        },
    ]
}

pub fn needs_confirm(name: &str) -> bool {
    matches!(name, "write_file" | "edit" | "bash") || name.starts_with("mcp__")
}

pub fn detail(name: &str, args: &str) -> String {
    let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
    let d = match name {
        "read_file" | "write_file" | "edit" => v["path"].as_str().unwrap_or("").to_string(),
        "list_files" => v["path"].as_str().unwrap_or(".").to_string(),
        "glob" | "grep" => {
            let p = v["pattern"].as_str().unwrap_or("").to_string();
            match v["include"].as_str() {
                Some(i) => format!("{p} ({i})"),
                None => p,
            }
        }
        "bash" => v["command"].as_str().unwrap_or("").to_string(),
        _ => {
            let d = args.lines().next().unwrap_or("").to_string();
            if d.chars().count() > 60 {
                let t: String = d.chars().take(57).collect();
                format!("{t}...")
            } else {
                d
            }
        }
    };
    let d = d.lines().next().unwrap_or("").to_string();
    if d.chars().count() > 80 {
        let t: String = d.chars().take(77).collect();
        format!("{t}...")
    } else {
        d
    }
}

pub fn preview(name: &str, args: &str) -> Vec<crate::diff::Row> {
    let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
    match name {
        "edit" => {
            let (Some(path), Some(old), Some(new)) = (
                v["path"].as_str(),
                v["old_str"].as_str(),
                v["new_str"].as_str(),
            ) else {
                return Vec::new();
            };
            match std::fs::read_to_string(path) {
                Ok(c) => crate::diff::preview_edit(&c, old, new),
                Err(_) => Vec::new(),
            }
        }
        "write_file" => {
            let (Some(path), Some(new)) = (v["path"].as_str(), v["content"].as_str()) else {
                return Vec::new();
            };
            let old = std::fs::read_to_string(path).ok();
            crate::diff::preview_write(old.as_deref(), new)
        }
        _ => Vec::new(),
    }
}

pub async fn execute(name: &str, args: &str, mcp: Option<&McpClient>) -> Result<String> {
    if let Some(rest) = name.strip_prefix("mcp__") {
        let Some(c) = mcp else {
            bail!("mcp is not configured");
        };
        return c.call(rest, args).await;
    }
    let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
    match name {
        "read_file" => {
            let Some(path) = v["path"].as_str() else {
                bail!("read_file: path required");
            };
            let offset = v["offset"].as_u64().unwrap_or(1).max(1) as usize;
            let limit = v["limit"].as_u64().unwrap_or(READ_LIMIT as u64).max(1) as usize;
            read_numbered(path, offset, limit)
        }
        "write_file" => {
            let Some(path) = v["path"].as_str() else {
                bail!("write_file: path required");
            };
            let Some(content) = v["content"].as_str() else {
                bail!("write_file: content required");
            };
            let n = files::apply(&WriteBlock {
                path: path.to_string(),
                content: content.to_string(),
            })?;
            Ok(format!("wrote {path} ({n} lines)"))
        }
        "edit" => {
            let Some(path) = v["path"].as_str() else {
                bail!("edit: path required");
            };
            let Some(old) = v["old_str"].as_str() else {
                bail!("edit: old_str required");
            };
            let Some(new) = v["new_str"].as_str() else {
                bail!("edit: new_str required");
            };
            let replace_all = v["replace_all"].as_bool().unwrap_or(false);
            let content = std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("{path}: {e}"))?;
            let count = content.matches(old).count();
            if count == 0 {
                bail!("edit: old_str not found in {path}");
            }
            if count > 1 && !replace_all {
                bail!("edit: old_str matches {count} times in {path}, add context or set replace_all");
            }
            let updated = if replace_all {
                content.replace(old, new)
            } else {
                content.replacen(old, new, 1)
            };
            std::fs::write(path, updated).map_err(|e| anyhow::anyhow!("{path}: {e}"))?;
            Ok(format!("edited {path} ({count} replacement{})", if count == 1 { "" } else { "s" }))
        }
        "list_files" => {
            let dir = v["path"].as_str().unwrap_or(".");
            let mut out = Vec::new();
            walk(dir, 0, &mut out);
            if out.is_empty() {
                return Ok(format!("{dir}: empty"));
            }
            out.sort();
            let mut s = out.join("\n");
            if out.len() >= MAX_LIST {
                s.push_str(&format!("\n... truncated at {MAX_LIST} entries"));
            }
            Ok(s)
        }
        "glob" => {
            let Some(pattern) = v["pattern"].as_str() else {
                bail!("glob: pattern required");
            };
            let dir = v["path"].as_str().unwrap_or(".");
            let files = crate::search::glob(dir, pattern)?;
            if files.is_empty() {
                return Ok(format!("{pattern}: no files found"));
            }
            let mut s = files.join("\n");
            if files.len() >= crate::search::MAX_RESULTS {
                s.push_str(&format!("\n... truncated at {} results", crate::search::MAX_RESULTS));
            }
            Ok(s)
        }
        "grep" => {
            let Some(pattern) = v["pattern"].as_str() else {
                bail!("grep: pattern required");
            };
            let dir = v["path"].as_str().unwrap_or(".");
            let include = v["include"].as_str();
            let hits = crate::search::grep(dir, pattern, include)?;
            if hits.is_empty() {
                return Ok(format!("{pattern}: no matches"));
            }
            let mut s = format!("Found {} matches", hits.len());
            if hits.len() >= crate::search::MAX_RESULTS {
                s.push_str(" (more matches available)");
            }
            let mut cur = String::new();
            for h in hits {
                if h.path != cur {
                    cur = h.path.clone();
                    s.push_str(&format!("\n{cur}:"));
                }
                s.push_str(&format!("\n  Line {}: {}", h.line, h.text));
            }
            Ok(s)
        }
        "bash" => {
            let Some(cmd) = v["command"].as_str() else {
                bail!("bash: command required");
            };
            let timeout = v["timeout"].as_u64().unwrap_or(120).clamp(1, 600);
            let (prog, flag) = shell();
            let mut command = tokio::process::Command::new(prog);
            command.arg(flag).arg(cmd);
            if let Some(w) = v["workdir"].as_str().filter(|s| !s.trim().is_empty()) {
                if !std::path::Path::new(w).is_dir() {
                    bail!("bash: workdir not found: {w}");
                }
                command.current_dir(w);
            }
            let out = tokio::time::timeout(
                std::time::Duration::from_secs(timeout),
                command.output(),
            )
            .await;
            match out {
                Err(_) => Ok(format!("command timed out ({timeout}s)")),
                Ok(res) => {
                    let res = res?;
                    let mut text = String::from_utf8_lossy(&res.stdout).to_string();
                    let err = String::from_utf8_lossy(&res.stderr);
                    if !err.trim().is_empty() {
                        if !text.is_empty() && !text.ends_with('\n') {
                            text.push('\n');
                        }
                        text.push_str(&err);
                    }
                    if text.len() > MAX_CAPTURE {
                        text = tail(&text, MAX_CAPTURE);
                    }
                    if text.trim().is_empty() {
                        text.push_str("(no output)");
                    }
                    if !res.status.success() {
                        text.push_str(&format!(
                            "\nexit code: {}",
                            res.status.code().unwrap_or(-1)
                        ));
                    }
                    Ok(text)
                }
            }
        }
        _ => bail!("unknown tool: {name}"),
    }
}

#[cfg(windows)]
fn shell() -> (&'static str, &'static str) {
    ("cmd", "/C")
}

#[cfg(not(windows))]
fn shell() -> (&'static str, &'static str) {
    ("sh", "-c")
}

fn norm(p: &str) -> String {
    crate::files::norm(p)
}

const MAX_ATTACH_BYTES: u64 = 128 * 1024;

fn read_numbered(path: &str, offset: usize, limit: usize) -> Result<String> {
    let bytes = std::fs::read(path).map_err(|e| anyhow::anyhow!("{path}: {e}"))?;
    if bytes.len() as u64 > MAX_ATTACH_BYTES {
        bail!("{path}: too large ({} bytes)", bytes.len());
    }
    if bytes.contains(&0) {
        return Ok(format!("{path}: binary file ({} bytes)", bytes.len()));
    }
    let text = String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("{path}: not valid utf-8"))?;
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    if total == 0 {
        return Ok(format!("{path}: empty file"));
    }
    let start = (offset - 1).min(total);
    let end = start.saturating_add(limit).min(total);
    let width = end.to_string().len();
    let mut out = format!("{path} ({total} lines)\n");
    for (i, l) in lines[start..end].iter().enumerate() {
        out.push_str(&format!("{:>width$}\u{2192}{}\n", start + i + 1, l));
    }
    if end < total {
        out.push_str(&format!(
            "... (+{} more lines, use offset {})",
            total - end,
            end + 1
        ));
    }
    Ok(out)
}

fn walk(dir: &str, depth: usize, out: &mut Vec<String>) {
    if depth > 3 || out.len() >= MAX_LIST {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if out.len() >= MAX_LIST {
            return;
        }
        let name = e.file_name().to_string_lossy().to_string();
        if matches!(name.as_str(), ".git" | "target" | "node_modules") {
            continue;
        }
        let path = e.path();
        let display = norm(&path.display().to_string());
        if path.is_dir() {
            out.push(format!("{display}/"));
            walk(&display, depth + 1, out);
        } else {
            out.push(display);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_keeps_tail() {
        let out = "a".repeat(100) + "THE_END";
        let b = budget(out.clone(), 16);
        assert!(b.starts_with("... ("));
        assert!(b.ends_with("THE_END"));
        assert!(b.len() < 60);
        assert_eq!(budget("short".into(), 100), "short");
        assert_eq!(budget("short".into(), 0), "short");
        let uni = "ё".repeat(50);
        assert!(budget(uni, 10).contains("ёё"));
    }

    #[test]
    fn read_numbering_offset_limit() {
        let dir = std::env::temp_dir().join(format!("hiderola-read-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.txt");
        std::fs::write(&p, "one\ntwo\nthree\nfour\nfive\n").unwrap();
        let path = p.display().to_string();

        let out = read_numbered(&path, 1, 100).unwrap();
        assert!(out.starts_with(&format!("{path} (5 lines)")));
        assert!(out.contains("1\u{2192}one"));
        assert!(out.contains("5\u{2192}five"));
        assert!(!out.contains("more lines"));

        let out = read_numbered(&path, 2, 2).unwrap();
        assert!(out.contains("2\u{2192}two"));
        assert!(out.contains("3\u{2192}three"));
        assert!(!out.contains("4\u{2192}four"));
        assert!(out.contains("+2 more lines, use offset 4"));

        let bin = dir.join("b.bin");
        std::fs::write(&bin, [0x89, 0x50, 0x00, 0x4e]).unwrap();
        let out = read_numbered(&bin.display().to_string(), 1, 10).unwrap();
        assert!(out.contains("binary file"));

        std::fs::write(&p, b"\xff\xfe\x00bad").unwrap();
        let out = read_numbered(&path, 1, 10).unwrap();
        assert!(out.contains("binary file"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
