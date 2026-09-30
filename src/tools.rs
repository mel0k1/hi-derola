use anyhow::{bail, Result};
use serde_json::{json, Value};

use crate::files::{self, WriteBlock};
use crate::provider::ToolSpec;

const MAX_LIST: usize = 500;
const MAX_CAPTURE: usize = 256 * 1024;
const BASH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "read_file",
            description: "Read a UTF-8 text file. Returns the file content, up to 128KB.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path, relative to the working directory or absolute"}
                },
                "required": ["path"]
            }),
        },
        ToolSpec {
            name: "write_file",
            description: "Create or overwrite a file with the complete content. Parent directories are created automatically.",
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
            name: "edit",
            description: "Perform exact string replacement in an existing file. old_str must match the file content exactly and be unique unless replace_all is true. Read the file first.",
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
            name: "list_files",
            description: "List files and directories recursively, up to 3 levels deep.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Directory path, defaults to the working directory"}
                }
            }),
        },
        ToolSpec {
            name: "bash",
            description: "Run a shell command and return stdout/stderr combined, with the exit code on failure.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "Shell command to execute"}
                },
                "required": ["command"]
            }),
        },
    ]
}

pub fn needs_confirm(name: &str) -> bool {
    matches!(name, "write_file" | "edit" | "bash")
}

pub fn detail(name: &str, args: &str) -> String {
    let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
    let d = match name {
        "read_file" | "write_file" | "edit" => v["path"].as_str().unwrap_or("").to_string(),
        "list_files" => v["path"].as_str().unwrap_or(".").to_string(),
        "bash" => v["command"].as_str().unwrap_or("").to_string(),
        _ => String::new(),
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

pub async fn execute(name: &str, args: &str) -> Result<String> {
    let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
    match name {
        "read_file" => {
            let Some(path) = v["path"].as_str() else {
                bail!("read_file: path required");
            };
            let content = files::read_attach(path)?;
            let n = content.lines().count();
            Ok(format!("{path}: {n} lines\n{content}"))
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
        "bash" => {
            let Some(cmd) = v["command"].as_str() else {
                bail!("bash: command required");
            };
            let out =
                tokio::time::timeout(BASH_TIMEOUT, tokio::process::Command::new("sh").arg("-c").arg(cmd).output())
                    .await;
            match out {
                Err(_) => Ok("command timed out (120s)".into()),
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
                        let mut end = MAX_CAPTURE;
                        while !text.is_char_boundary(end) {
                            end -= 1;
                        }
                        text.truncate(end);
                        text.push_str("\n... truncated");
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
        let display = path.display().to_string();
        if path.is_dir() {
            out.push(format!("{display}/"));
            walk(&display, depth + 1, out);
        } else {
            out.push(display);
        }
    }
}
