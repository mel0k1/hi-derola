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
    let mut specs = vec![
        ToolSpec {
            name: "read_file".into(),
            description: "Read a UTF-8 text file with line numbers (1-based, cat -n style). Returns up to limit lines starting at offset. Binary files are detected and not dumped. Image files (png/jpg/gif/webp/bmp) are returned as attached images the model can see directly.".into(),
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
            description: "Perform string replacement in an existing file. Tries an exact match first, then tolerates line ending (CRLF/LF), BOM and trailing whitespace differences. Must be unique unless replace_all is true. Read the file first.".into(),
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
            description: "Run a shell command (sh on unix, cmd on Windows) and return stdout/stderr combined. Long output keeps only the tail. Exit code is added on failure. Set background=true for dev servers and long-running builds: the tool returns a task id immediately and the output arrives as a new message when the command finishes; do not poll task_status for completion.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "Shell command to execute"},
                    "workdir": {"type": "string", "description": "Optional working directory for the command"},
                    "timeout": {"type": "integer", "description": "Timeout in seconds, default 120, max 600 (foreground)"},
                    "background": {"type": "boolean", "description": "Run in the background and return immediately; you will be notified when it completes. No '&' needed. Do not poll for completion."}
                },
                "required": ["command"]
            }),
        },
        ToolSpec {
            name: "webfetch".into(),
            description: "Fetch content from an HTTP or HTTPS URL. HTML pages are converted to markdown, textual content types are returned as-is. Read-only.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "The HTTP or HTTPS URL to fetch"},
                    "format": {"type": "string", "enum": ["markdown", "text", "html"], "description": "Output format for HTML pages, default markdown"},
                    "timeout": {"type": "integer", "description": "Timeout in seconds, default 30, max 120"}
                },
                "required": ["url"]
            }),
        },
        ToolSpec {
            name: "websearch".into(),
            description: "Search the web for current information: docs, news, releases, error messages. Returns a ranked list of titles, URLs and snippets; use webfetch to read a specific page.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query"},
                    "timeout": {"type": "integer", "description": "Timeout in seconds, default 15, max 60"}
                },
                "required": ["query"]
            }),
        },
        ToolSpec {
            name: "question".into(),
            description: "Ask the user questions during execution: gather preferences, clarify ambiguous instructions, get decisions on implementation choices. A free-form answer is always available. If you recommend an option, put it first and add \"(Recommended)\" at the end of the label.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "description": "Questions to ask",
                        "items": {
                            "type": "object",
                            "properties": {
                                "question": {"type": "string", "description": "The question text"},
                                "header": {"type": "string", "description": "Very short label, a few words"},
                                "options": {
                                    "type": "array",
                                    "description": "Answer choices",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "label": {"type": "string"},
                                            "description": {"type": "string"}
                                        },
                                        "required": ["label"]
                                    }
                                },
                                "multiple": {"type": "boolean", "description": "Allow selecting more than one option"}
                            },
                            "required": ["question"]
                        }
                    }
                },
                "required": ["questions"]
            }),
        },
        ToolSpec {
            name: "subagent".into(),
            description: format!(
                "Spawns a subagent in a fresh context to work on the task and returns its final response. Include all relevant context and instructions in the prompt: the subagent starts with no history. Use for isolated research, exploration or bulk changes. For long tasks set background=true: the tool returns a task id immediately and the result arrives as a new message when done. Cannot ask the user questions.\nAvailable agents:\n{}",
                crate::agents::list_for_spec()
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "description": {"type": "string", "description": "A short 3-5 word label for the task, displayed to the user"},
                    "prompt": {"type": "string", "description": "The task for the subagent to perform"},
                    "agent": {"type": "string", "description": "Agent profile to use, see the list in the description; omit for the default general agent"},
                    "background": {"type": "boolean", "description": "Run in the background: return a task id now, deliver the result later; check progress with task_status"}
                },
                "required": ["description", "prompt"]
            }),
        },
        ToolSpec {
            name: "task_status".into(),
            description: "Check background tasks (subagents and bash commands). Without arguments lists all tasks with their statuses. Pass id to get the full result of a finished task, or the live output of a running one.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Task id, e.g. bg-1; omit to list all tasks"}
                }
            }),
        },
        ToolSpec {
            name: "task_kill".into(),
            description: "Kill a running background task (bash command or subagent) by id. Use to stop a dev server or a job that is no longer needed. Get ids from task_status.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Task id, e.g. bg-1"}
                },
                "required": ["id"]
            }),
        },
        ToolSpec {
            name: "todowrite".into(),
            description: "Create and maintain a structured task list for the current session. Use proactively for multi-step work (3+ steps or multiple tasks): capture new instructions as todos, keep exactly ONE in_progress while working, update statuses in real time and mark completed only after the work is actually done (including verification). Skip for single straightforward tasks or informational requests.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "description": "The full updated todo list; replaces the previous one",
                        "items": {
                            "type": "object",
                            "properties": {
                                "content": {"type": "string", "description": "Brief, actionable task description"},
                                "status": {"type": "string", "enum": ["pending", "in_progress", "completed", "cancelled"], "description": "Current status of the task"},
                                "priority": {"type": "string", "enum": ["high", "medium", "low"], "description": "Priority level of the task"}
                            },
                            "required": ["content"]
                        }
                    }
                },
                "required": ["todos"]
            }),
        },
        ToolSpec {
            name: "todoread".into(),
            description: "Read the current todo list for the session. Use to re-check the plan after a context compaction or before continuing multi-step work.".into(),
            parameters: json!({ "type": "object", "properties": {} }),
        },
    ];
    let skill_desc = crate::skills::spec_description();
    if !skill_desc.is_empty() {
        specs.push(ToolSpec {
            name: "skill".into(),
            description: skill_desc,
            parameters: crate::skills::spec(),
        });
    }
    specs
}

pub fn specs_core() -> Vec<ToolSpec> {
    specs().into_iter().filter(|s| s.name != "subagent").collect()
}

pub fn specs_nested() -> Vec<ToolSpec> {
    specs_core()
        .into_iter()
        .filter(|s| s.name != "question" && s.name != "todowrite" && s.name != "todoread")
        .collect()
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
        "bash" => {
            let mut d = v["command"].as_str().unwrap_or("").to_string();
            if v["background"].as_bool().unwrap_or(false) {
                d.push_str(" (background)");
            }
            d
        }
        "webfetch" => v["url"].as_str().unwrap_or("").to_string(),
        "websearch" => v["query"].as_str().unwrap_or("").to_string(),
        "question" => v["questions"][0]["question"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        "subagent" => v["description"].as_str().unwrap_or("").to_string(),
        "task_status" => v["id"].as_str().unwrap_or("background tasks").to_string(),
        "task_kill" => v["id"].as_str().unwrap_or("background tasks").to_string(),
        "todowrite" => {
            let n = v["todos"].as_array().map(|a| a.len()).unwrap_or(0);
            format!("{n} todos")
        }
        "todoread" => "todo list".to_string(),
        "skill" => crate::skills::detail(args),
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
            let replace_all = v["replace_all"].as_bool().unwrap_or(false);
            match std::fs::read_to_string(path) {
                Ok(c) => match apply_edit(&c, old, new, replace_all) {
                    Ok((updated, _)) => crate::diff::lines_diff(&c, &updated),
                    Err(_) => Vec::new(),
                },
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
            Ok(format!("wrote {path} ({n} lines){}", post_edit(path).await))
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
            if old.is_empty() {
                bail!("edit: old_str is empty");
            }
            let replace_all = v["replace_all"].as_bool().unwrap_or(false);
            let content = std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("{path}: {e}"))?;
            let (updated, count) = apply_edit(&content, old, new, replace_all)
                .map_err(|e| anyhow::anyhow!("edit: {e:#} in {path}"))?;
            std::fs::write(path, updated).map_err(|e| anyhow::anyhow!("{path}: {e}"))?;
            Ok(format!(
                "edited {path} ({count} replacement{}){}",
                if count == 1 { "" } else { "s" },
                post_edit(path).await
            ))
        }
        "list_files" => {
            let dir = v["path"].as_str().unwrap_or(".");
            let out = list_tree(dir);
            if out.is_empty() {
                return Ok(format!("{dir}: empty"));
            }
            Ok(out.join("\n"))
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
        "webfetch" => {
            let Some(url) = v["url"].as_str() else {
                bail!("webfetch: url required");
            };
            let format = v["format"].as_str().unwrap_or("markdown");
            let timeout = v["timeout"].as_u64().unwrap_or(30);
            crate::web::fetch_markdown(url, format, timeout).await
        }
        "websearch" => {
            let Some(query) = v["query"].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
                bail!("websearch: query required");
            };
            let timeout = v["timeout"].as_u64().unwrap_or(15).clamp(5, 60);
            crate::web::websearch(query, timeout).await
        }
        "task_status" => {
            let id = v["id"].as_str().filter(|s| !s.trim().is_empty());
            Ok(crate::bg::status(id))
        }
        "task_kill" => {
            let Some(id) = v["id"].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
                bail!("task_kill: id required");
            };
            Ok(crate::bg::kill(id))
        }
        "todoread" => Ok(crate::todo::read_render()),
        "skill" => {
            let Some(name) = v["name"].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
                bail!("skill: name required");
            };
            crate::skills::load(name)
        }
        "bash" => {
            let Some(cmd) = v["command"].as_str() else {
                bail!("bash: command required");
            };
            let timeout = v["timeout"].as_u64().unwrap_or(120).clamp(1, 600);
            bash_run(cmd, v["workdir"].as_str(), Some(timeout)).await
        }
        _ => bail!("unknown tool: {name}"),
    }
}

pub async fn bash_run(cmd: &str, workdir: Option<&str>, timeout: Option<u64>) -> Result<String> {
    let (prog, flag) = shell();
    let mut command = tokio::process::Command::new(prog);
    command.arg(flag).arg(cmd);
    command.kill_on_drop(true);
    command.env("AGENT", "1");
    command.env("HI_DEROLA", "1");
    if let Some(w) = workdir.map(str::trim).filter(|s| !s.is_empty()) {
        if !std::path::Path::new(w).is_dir() {
            bail!("bash: workdir not found: {w}");
        }
        command.current_dir(w);
    }
    let wait = command.output();
    let res = match timeout {
        Some(t) => match tokio::time::timeout(std::time::Duration::from_secs(t.max(1)), wait).await {
            Err(_) => return Ok(format!("command timed out ({t}s)")),
            Ok(res) => res?,
        },
        None => wait.await?,
    };
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

pub fn spawn_shell(cmd: &str, workdir: Option<&str>) -> Result<tokio::process::Child> {
    let (prog, flag) = shell();
    let mut command = tokio::process::Command::new(prog);
    command.arg(flag).arg(cmd);
    command.env("AGENT", "1");
    command.env("HI_DEROLA", "1");
    if let Some(w) = workdir.map(str::trim).filter(|s| !s.is_empty()) {
        if !std::path::Path::new(w).is_dir() {
            bail!("bash: workdir not found: {w}");
        }
        command.current_dir(w);
    }
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);
    Ok(command.spawn()?)
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

fn normalize_eol(s: &str) -> String {
    s.replace("\r\n", "\n")
}

fn norm_char(c: char) -> char {
    match c {
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
        '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
        '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
        '\u{00A0}' | '\u{2007}' | '\u{202F}' => ' ',
        _ => c,
    }
}

/// match old/new with lookalike unicode (smart quotes, dashes, nbsp) normalized
/// to ascii, splice the replacement into the original text 1:1
fn unicode_edit(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> anyhow::Result<Option<(String, usize)>> {
    let cc: Vec<char> = content.chars().collect();
    let nc: Vec<char> = cc.iter().copied().map(norm_char).collect();
    let on: Vec<char> = old.chars().map(norm_char).collect();
    if on.is_empty() || on.len() > nc.len() {
        return Ok(None);
    }
    let mut hits = Vec::new();
    for i in 0..=nc.len() - on.len() {
        if nc[i..i + on.len()] == on[..] {
            hits.push(i);
        }
    }
    if hits.is_empty() {
        return Ok(None);
    }
    if hits.len() > 1 && !replace_all {
        anyhow::bail!("old_str matches multiple times, add context or set replace_all");
    }
    let newc: Vec<char> = new.chars().collect();
    let mut out = cc;
    let targets: Vec<usize> = if replace_all {
        hits.clone()
    } else {
        vec![hits[0]]
    };
    for lo in targets.into_iter().rev() {
        out.splice(lo..lo + on.len(), newc.iter().copied());
    }
    let n = if replace_all { hits.len() } else { 1 };
    Ok(Some((out.into_iter().collect(), n)))
}

fn try_edit(content: &str, old: &str, new: &str, replace_all: bool) -> Option<(String, usize)> {
    let n = content.matches(old).count();
    if n == 0 || (n > 1 && !replace_all) {
        return None;
    }
    let out = if replace_all {
        content.replace(old, new)
    } else {
        content.replacen(old, new, 1)
    };
    Some((out, n))
}

fn trim_edit(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> anyhow::Result<Option<(String, usize)>> {
    let c_lines: Vec<&str> = content.lines().collect();
    let o_lines: Vec<&str> = old.lines().collect();
    if o_lines.is_empty() {
        return Ok(None);
    }
    let n_lines: Vec<String> = new.lines().map(|s| s.to_string()).collect();
    let c_trim: Vec<&str> = c_lines.iter().map(|l| l.trim_end()).collect();
    let o_trim: Vec<&str> = o_lines.iter().map(|l| l.trim_end()).collect();
    let hits: Vec<usize> = (0..=c_lines.len().saturating_sub(o_lines.len()))
        .filter(|&i| c_trim[i..i + o_lines.len()] == o_trim[..])
        .collect();
    if hits.is_empty() {
        return Ok(None);
    }
    if hits.len() > 1 && !replace_all {
        bail!("old_str matches {} times", hits.len());
    }
    let count = hits.len();
    let mut lines: Vec<String> = c_lines.iter().map(|s| s.to_string()).collect();
    for i in hits.into_iter().rev() {
        lines.splice(i..i + o_lines.len(), n_lines.iter().cloned());
    }
    let mut joined = lines.join("\n");
    if content.ends_with('\n') && !joined.is_empty() {
        joined.push('\n');
    }
    Ok(Some((joined, count)))
}

pub fn apply_edit(content: &str, old: &str, new: &str, replace_all: bool) -> Result<(String, usize)> {
    let (body, bom) = match content.strip_prefix('\u{feff}') {
        Some(rest) => (rest, true),
        None => (content, false),
    };
    let crlf = body.contains("\r\n");
    let norm = normalize_eol(body);
    let old_n = normalize_eol(old);
    let new_n = normalize_eol(new);
    let mut multi = false;
    if let Some((out, n)) = try_edit(body, old, new, replace_all) {
        return Ok((with_bom(&out, bom), n));
    }
    if let Some((out, n)) = try_edit(&norm, &old_n, &new_n, replace_all) {
        return Ok((with_bom(&restore_eol(&out, crlf), bom), n));
    }
    match trim_edit(&norm, &old_n, &new_n, replace_all) {
        Ok(Some((out, n))) => return Ok((with_bom(&restore_eol(&out, crlf), bom), n)),
        Ok(None) => {}
        Err(_) => multi = true,
    }
    match unicode_edit(&norm, &old_n, &new_n, replace_all) {
        Ok(Some((out, n))) => return Ok((with_bom(&restore_eol(&out, crlf), bom), n)),
        Ok(None) => {}
        Err(_) => multi = true,
    }
    if multi || (!old_n.is_empty() && norm.matches(&old_n).count() > 1) {
        bail!("old_str matches multiple times, add context or set replace_all");
    }
    bail!("old_str not found");
}

fn with_bom(s: &str, bom: bool) -> String {
    if bom {
        format!("\u{feff}{s}")
    } else {
        s.to_string()
    }
}

fn restore_eol(text: &str, crlf: bool) -> String {
    if crlf {
        text.replace('\n', "\r\n")
    } else {
        text.to_string()
    }
}

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

/// After write_file/edit: run the formatter, then collect LSP diagnostics.
async fn post_edit(path: &str) -> String {
    let mut out = String::new();
    if let Some(name) = crate::fmt::format_file(path).await {
        out.push_str(&format!("\nformatted with {name}"));
    }
    if let Some(diags) = crate::lsp::diagnose(path).await {
        out.push_str(&format!("\n{diags}"));
    }
    out
}

fn list_tree(dir: &str) -> Vec<String> {
    let walker = ignore::WalkBuilder::new(dir)
        .max_depth(Some(3))
        .require_git(false)
        .build();
    let mut out = Vec::new();
    for e in walker.flatten() {
        if out.len() >= MAX_LIST {
            break;
        }
        if e.depth() == 0 {
            continue;
        }
        let display = norm(&e.path().display().to_string());
        if e.file_type().is_some_and(|t| t.is_dir()) {
            out.push(format!("{display}/"));
        } else {
            out.push(display);
        }
    }
    out.sort();
    if out.len() >= MAX_LIST {
        out.truncate(MAX_LIST);
        out.push(format!("... truncated at {MAX_LIST} entries"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_fallback_edit() {
        // content with smart quotes, old_str typed with ascii quotes
        let content = "const s = \u{201c}hello\u{201d};\nlet d = \u{2013} 1;";
        let (out, n) = apply_edit(content, "const s = \"hello\";", "const s = \"bye\";", false).unwrap();
        assert_eq!(n, 1);
        assert!(out.contains("const s = \"bye\";"));
        assert!(out.contains('\u{2013}'), "untouched chars must stay original");
        // nbsp tolerance
        let (out, _) = apply_edit("a\u{00a0}b", "a b", "ab", false).unwrap();
        assert_eq!(out, "ab");
        // multi without replace_all still errors
        assert!(apply_edit("x\u{2019}y x\u{2019}y", "x'y", "z", false).is_err());
        let (out, n) = apply_edit("x\u{2019}y x\u{2019}y", "x'y", "z", true).unwrap();
        assert_eq!(n, 2);
        assert_eq!(out, "z z");
    }

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
    fn edit_tolerant_matching() {
        let crlf = "fn main() {\r\n    let x = 1;\r\n    println!(\"{}\", x);\r\n}\r\n";
        let (out, n) = apply_edit(crlf, "let x = 1;\n    println", "let x = 2;\n    println", false).unwrap();
        assert_eq!(n, 1);
        assert!(out.contains("let x = 2;"));
        assert!(out.contains("\r\n"));
        assert_eq!(out.matches("\r\n").count(), crlf.matches("\r\n").count());

        let bom_content = "\u{feff}line one\nline two\n";
        let (out, n) = apply_edit(bom_content, "line two", "line TWO", false).unwrap();
        assert_eq!(n, 1);
        assert!(out.starts_with('\u{feff}'));
        assert!(out.contains("line TWO"));

        let ws = "fn a() {   \n    let y = 2;\t\n}\n";
        let (out, n) = apply_edit(ws, "fn a() {\n    let y = 2;\n}", "fn a() {\n    let y = 3;\n}", false).unwrap();
        assert_eq!(n, 1);
        assert!(out.contains("let y = 3;"));
        assert!(out.ends_with('\n'));

        let dup = "a\nfoo\nb\nfoo\n";
        let (out, n) = apply_edit(dup, "foo", "bar", true).unwrap();
        assert_eq!(n, 2);
        assert_eq!(out.matches("bar").count(), 2);
        assert!(apply_edit(dup, "foo", "bar", false).is_err());

        let dup_ws = "a  \nfoo\nb\nfoo  \n";
        assert!(apply_edit(dup_ws, "foo", "x", false).is_err());
        let (out, n) = apply_edit(dup_ws, "foo\nb", "x", false).unwrap();
        assert_eq!(n, 1);
        assert!(out.contains("a  \nx\nfoo  "));

        assert!(apply_edit("hello\n", "missing text", "x", false).is_err());
        assert!(apply_edit("hello\n", "", "x", false).is_err());

        let (out, n) = apply_edit("keep\nold line\nend\n", "old line", "new line", false).unwrap();
        assert_eq!(n, 1);
        assert_eq!(out, "keep\nnew line\nend\n");
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
