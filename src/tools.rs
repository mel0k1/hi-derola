use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::sync::{Mutex, OnceLock};

use crate::files::{self, WriteBlock};
use crate::mcp::McpClient;
use crate::provider::ToolSpec;

const MAX_LIST: usize = 500;
const MAX_CAPTURE: usize = 256 * 1024;
const READ_LIMIT: usize = 2000;
/// how long a timed-out process tree may finish dying after SIGTERM
/// (escalated to SIGKILL) before the shell result is returned
const KILL_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

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
    format!(
        "... ({} chars truncated from the head)\n{}",
        dropped,
        &s[start..]
    )
}

/// tool outputs are also saved to a file (so the model can read back whatever
/// the inline budget cut away) once they reach this many bytes or lines —
/// same two triggers as opencode's Truncate
pub const SPILL_MIN: usize = 50_000;
pub const SPILL_LINES: usize = 2000;
/// spilled files older than this are deleted opportunistically on every spill
const SPILL_RETENTION_SECS: u64 = 7 * 24 * 3600;

static TRUNCATION_OVERRIDE: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

/// tests point the spill store at a temp directory; None clears the override
pub fn set_truncation_dir(dir: Option<std::path::PathBuf>) {
    *TRUNCATION_OVERRIDE.lock().unwrap() = dir;
}

/// where spilled tool outputs live: <data>/hi-derola/truncated
pub fn truncation_dir() -> Option<std::path::PathBuf> {
    if let Some(d) = TRUNCATION_OVERRIDE.lock().unwrap().clone() {
        return Some(d);
    }
    dirs::data_dir().map(|d| d.join("hi-derola").join("truncated"))
}

/// same budgeting as budget(), but oversized outputs additionally get saved
/// verbatim to the truncation directory and the result carries the path, so
/// nothing is truly lost to the inline cap; max == 0 (unlimited) skips spilling
pub fn spill(out: String, tool: &str, max: usize) -> String {
    match truncation_dir() {
        Some(dir) => spill_into(&dir, out, tool, max),
        None => budget(out, max),
    }
}

pub fn spill_into(dir: &std::path::Path, out: String, tool: &str, max: usize) -> String {
    let big = out.len() >= SPILL_MIN || out.lines().count() > SPILL_LINES;
    if !big || max == 0 {
        return budget(out, max);
    }
    let saved = save_spill(dir, tool, &out);
    let kept = budget(out, max);
    match saved {
        Ok(path) => format!(
            "{kept}\n\n[full output saved to {path} — read it with read_file if you need the part cut off here]"
        ),
        Err(_) => kept,
    }
}

fn save_spill(dir: &std::path::Path, tool: &str, text: &str) -> std::io::Result<String> {
    std::fs::create_dir_all(dir)?;
    spill_cleanup(dir);
    let name: String = tool
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    for i in 0u32..1000 {
        let suffix = if i == 0 {
            String::new()
        } else {
            format!("-{i}")
        };
        let file = dir.join(format!("{name}-{stamp}{suffix}.txt"));
        if file.exists() {
            continue;
        }
        std::fs::write(&file, text)?;
        return Ok(file.display().to_string());
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "spill: no free file name",
    ))
}

fn spill_cleanup(dir: &std::path::Path) {
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(SPILL_RETENTION_SECS));
    let Some(cutoff) = cutoff else { return };
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| t < cutoff)
            .unwrap_or(false);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// true when `path` (an existing file) resolves inside `base`; used to let the
/// model read our own spilled outputs without an external-directory approval
pub fn path_in_dir(base: &std::path::Path, path: &str) -> bool {
    let Ok(base) = std::fs::canonicalize(base) else {
        return false;
    };
    let p = std::path::Path::new(path.trim());
    let probe = if p.is_absolute() {
        p.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(p),
            Err(_) => return false,
        }
    };
    match std::fs::canonicalize(&probe) {
        Ok(c) => c.starts_with(&base),
        Err(_) => false,
    }
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
            description: "Perform string replacement in an existing file. Tries an exact match first, then tolerates line ending (CRLF/LF), BOM, trailing whitespace and lookalike unicode (smart quotes, nbsp) differences, and as a last resort a fuzzy line match (at least 2 lines, >=85% similarity) whose replacement is re-indented to the matched block. Must be unique unless replace_all is true. Read the file first.".into(),
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
            name: "apply_patch".into(),
            description: "Apply a multi-file patch in the V4A format. Starts with *** Begin Patch, then one or more sections: *** Add File: <path> with every content line prefixed by '+', *** Update File: <path> (optional *** Move to: <newpath>) with hunks of ' ' context, '-' old and '+' new lines (optional '@@' separators), *** Delete File: <path>, and *** End Patch. The ' ' and '-' lines must match the current file contents exactly. Several files can be patched in one call; nothing is written unless every hunk matches. Use it for wide-reaching multi-file changes; prefer edit for small single-file tweaks.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "patch": {"type": "string", "description": "The full patch text"}
                },
                "required": ["patch"]
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
            description: "Run a shell command (sh on unix, cmd on Windows) and return stdout/stderr combined. Long output keeps only the tail. Exit code is added on failure. A foreground timeout asks the tree to exit (SIGTERM on unix), then kills the whole tree (process group on unix, job object on Windows) and returns 'command timed out (Ns)'. Set background=true for dev servers and long-running builds: the tool returns a task id immediately and the output arrives as a new message when the command finishes; do not poll task_status for completion.".into(),
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
            name: "codesearch".into(),
            description: "Search and get relevant context for any programming task using the Exa Code API: high-quality, fresh code examples, documentation and API references for libraries, SDKs and APIs. Use for ANY question about frameworks, libraries, APIs or programming patterns; complements websearch (general web results) and webfetch (read a known URL). Lower tokensNum for focused questions, higher for comprehensive documentation.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query, e.g. 'React useState hook examples', 'Python pandas dataframe filtering', 'Express.js middleware'"},
                    "tokensNum": {"type": "integer", "description": "Context budget in tokens (1000-50000), default 5000"},
                    "timeout": {"type": "integer", "description": "Timeout in seconds, default 30, max 120"}
                },
                "required": ["query"]
            }),
        },
        ToolSpec {
            name: "mcp_resource".into(),
            description: "Read a resource exposed by an MCP server: file contents, database rows, docs, live data — anything the server lists under its resources. Returns the resource text; binary data is summarized. Use /mcpres (or ask the user) to discover available uris.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "server": {"type": "string", "description": "MCP server name; optional when the uri is unique across servers"},
                    "uri": {"type": "string", "description": "Resource uri, e.g. file:///path or mem://key"}
                },
                "required": ["uri"]
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
                "Spawns a subagent in a child context to work on the task and returns its final response. Include all relevant context and instructions in the prompt: a new subagent starts with no history. Use for isolated research, exploration or bulk changes. For long tasks set background=true: the tool returns a task id immediately and the result arrives as a new message when done. Cannot ask the user questions.\nAvailable agents:\n{}",
                crate::agents::list_for_spec()
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "description": {"type": "string", "description": "A short 3-5 word label for the task, displayed to the user"},
                    "prompt": {"type": "string", "description": "The task for the subagent to perform"},
                    "agent": {"type": "string", "description": "Agent profile to use, see the list in the description; omit for the default general agent"},
                    "session_id": {"type": "string", "description": "Continue a previous subagent conversation by passing the id reported with its result; omit to start a new conversation"},
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
            name: "lsp".into(),
            description: "Navigate code with the language server (LSP) when one is installed: hover, definition, references, implementation, document_symbols, workspace_symbols, incoming_calls (who calls the function at the position), outgoing_calls (what that function calls). Line and column are 1-based as shown in editors. document_symbols needs path only; workspace_symbols needs query only; the position operations need path + line + column. Returns nothing useful when no server supports the file type.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "operation": {"type": "string", "enum": ["hover", "definition", "references", "implementation", "document_symbols", "workspace_symbols", "prepare_call_hierarchy", "incoming_calls", "outgoing_calls"], "description": "The LSP operation to perform"},
                    "path": {"type": "string", "description": "File path (required for everything except workspace_symbols)"},
                    "line": {"type": "integer", "description": "Line number, 1-based"},
                    "column": {"type": "integer", "description": "Character offset on the line, 1-based, default 1"},
                    "query": {"type": "string", "description": "Search query for workspace_symbols"},
                    "include_declaration": {"type": "boolean", "description": "references: include the declaration itself, default true"}
                },
                "required": ["operation"]
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

/// plan-mode-only tools: saving the plan file and asking to leave plan mode
pub fn plan_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "plan_write".into(),
            description: "Save or update the plan file (.hi-derola/plan.md). Plan mode only. Rewrite the complete plan on every update: goal, step-by-step changes, files to touch, risks. The user reads this file when approving.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "plan": {"type": "string", "description": "The full plan in markdown"}
                },
                "required": ["plan"]
            }),
        },
        ToolSpec {
            name: "plan_exit".into(),
            description: "Ask the user to approve leaving plan mode and starting implementation. Call after the plan is saved with plan_write; the user can approve, keep planning, or reply with free-form feedback.".into(),
            parameters: json!({"type": "object", "properties": {}}),
        },
    ]
}

pub fn specs_nested(allow_subagent: bool) -> Vec<ToolSpec> {
    specs()
        .into_iter()
        .filter(|s| match s.name.as_str() {
            "subagent" => allow_subagent,
            "question" | "todowrite" | "todoread" => false,
            _ => true,
        })
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
        "apply_patch" => {
            let s = v["patch"]
                .as_str()
                .map(crate::patch::summarize)
                .unwrap_or_default();
            if s.is_empty() {
                "patch".to_string()
            } else {
                s
            }
        }
        "webfetch" => v["url"].as_str().unwrap_or("").to_string(),
        "websearch" | "codesearch" => v["query"].as_str().unwrap_or("").to_string(),
        "mcp_resource" => {
            let s = v["server"].as_str().unwrap_or("");
            let u = v["uri"].as_str().unwrap_or("");
            if s.is_empty() {
                u.to_string()
            } else {
                format!("{s} {u}")
            }
        }
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
        "lsp" => {
            let op = v["operation"].as_str().unwrap_or("lsp");
            let pos = match v["operation"].as_str() {
                Some("workspace_symbols") => v["query"].as_str().unwrap_or("").to_string(),
                Some("document_symbols") => v["path"].as_str().unwrap_or("").to_string(),
                _ => {
                    let p = v["path"].as_str().unwrap_or("");
                    let l = v["line"].as_u64().unwrap_or(1);
                    let c = v["column"].as_u64().unwrap_or(1);
                    format!("{p}:{l}:{c}")
                }
            };
            format!("{op} {pos}")
        }
        "code" => {
            let c = v["code"].as_str().unwrap_or("").trim();
            let first = c.lines().next().unwrap_or("").trim();
            if first.chars().count() > 60 {
                let t: String = first.chars().take(57).collect();
                format!("{t}...")
            } else if first.is_empty() {
                "script".to_string()
            } else {
                first.to_string()
            }
        }
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

/// file paths a mutation touches; drives the session review panel
pub fn paths(name: &str, args: &str) -> Vec<String> {
    let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
    match name {
        "write_file" | "edit" => v["path"]
            .as_str()
            .map(|p| vec![p.to_string()])
            .unwrap_or_default(),
        "apply_patch" => v["patch"]
            .as_str()
            .and_then(|s| crate::patch::parse(s).ok())
            .map(|ops| {
                ops.into_iter()
                    .map(|o| match o {
                        crate::patch::Op::Add { path, .. }
                        | crate::patch::Op::Update { path, .. }
                        | crate::patch::Op::Delete { path } => path,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
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
        "apply_patch" => {
            let Some(patch) = v["patch"].as_str() else {
                return Vec::new();
            };
            let mut rows: Vec<crate::diff::Row> = Vec::new();
            for pv in crate::patch::preview(patch) {
                if rows.len() > 80 {
                    break;
                }
                rows.push(crate::diff::Row {
                    tag: 0,
                    text: format!("--- {}", pv.label),
                });
                match (pv.old.as_deref(), pv.new.as_deref()) {
                    (Some(o), Some(n)) => rows.extend(crate::diff::lines_diff(o, n)),
                    (None, Some(n)) => rows.extend(crate::diff::preview_write(None, n)),
                    (Some(o), None) => rows.extend(crate::diff::lines_diff(o, "")),
                    _ => {}
                }
            }
            if rows.len() > 80 {
                rows.truncate(80);
                rows.push(crate::diff::Row {
                    tag: 0,
                    text: "...".into(),
                });
            }
            rows
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
        "apply_patch" => {
            let Some(patch) = v["patch"].as_str() else {
                bail!("apply_patch: patch required");
            };
            let ops = crate::patch::parse(patch)?;
            let planned = crate::patch::plan(ops)?;
            let items = planned.items.clone();
            let written = crate::patch::commit(planned)?;
            let mut out = String::from("Success. Updated the following files:");
            for (k, p) in &items {
                out.push_str(&format!("\n{k} {p}"));
            }
            for p in &written {
                out.push_str(&post_edit(p).await);
            }
            Ok(out)
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
            let content =
                std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("{path}: {e}"))?;
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
                s.push_str(&format!(
                    "\n... truncated at {} results",
                    crate::search::MAX_RESULTS
                ));
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
        "codesearch" => {
            let Some(query) = v["query"].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
                bail!("codesearch: query required");
            };
            let tokens = v["tokensNum"].as_u64().unwrap_or(5_000);
            let timeout = v["timeout"].as_u64().unwrap_or(30);
            crate::exa::codesearch(query, tokens, timeout).await
        }
        "mcp_resource" => {
            let Some(c) = mcp else {
                bail!("mcp is not configured");
            };
            let Some(uri) = v["uri"].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
                bail!("mcp_resource: uri required");
            };
            c.read_resource(v["server"].as_str().unwrap_or(""), uri)
                .await
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
        "lsp" => {
            let Some(op) = v["operation"].as_str() else {
                bail!("lsp: operation required");
            };
            let path = v["path"].as_str().unwrap_or("");
            let line = v["line"].as_u64().unwrap_or(1).max(1) as u32;
            let col = v["column"].as_u64().unwrap_or(1).max(1) as u32;
            match op {
                "hover" => crate::lsp::hover(path, line, col).await,
                "definition" => crate::lsp::definition(path, line, col).await,
                "implementation" => crate::lsp::implementation(path, line, col).await,
                "references" => {
                    crate::lsp::references(
                        path,
                        line,
                        col,
                        v["include_declaration"].as_bool().unwrap_or(true),
                    )
                    .await
                }
                "document_symbols" => crate::lsp::document_symbols(path).await,
                "workspace_symbols" => {
                    crate::lsp::workspace_symbols(v["query"].as_str().unwrap_or("")).await
                }
                "prepare_call_hierarchy" => {
                    crate::lsp::prepare_call_hierarchy(path, line, col).await
                }
                "incoming_calls" => crate::lsp::incoming_calls(path, line, col).await,
                "outgoing_calls" => crate::lsp::outgoing_calls(path, line, col).await,
                _ => bail!("lsp: unknown operation: {op}"),
            }
        }
        "bash" => {
            let Some(cmd) = v["command"].as_str() else {
                bail!("bash: command required");
            };
            let timeout = v["timeout"].as_u64().unwrap_or(120).clamp(1, 600);
            if crate::sandbox::shell_route().is_some() {
                // sandbox attached: the command runs inside the VM over ssh
                // (the system ssh client blocks — keep it off the async workers)
                let cmd = cmd.to_string();
                return match tokio::task::spawn_blocking(move || {
                    crate::sandbox::sandbox_bash(&cmd, Some(timeout))
                })
                .await
                {
                    Ok(res) => res,
                    Err(e) => bail!("sandbox bash: {e}"),
                };
            }
            bash_run(cmd, v["workdir"].as_str(), Some(timeout)).await
        }
        _ if crate::jstools::has(name) => crate::jstools::run_tool(name, args).await,
        _ => bail!("unknown tool: {name}"),
    }
}

pub async fn bash_run(cmd: &str, workdir: Option<&str>, timeout: Option<u64>) -> Result<String> {
    let (prog, flag) = shell();
    let mut command = tokio::process::Command::new(prog);
    command.arg(flag).arg(cmd);
    command.kill_on_drop(true);
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    command.env("AGENT", "1");
    command.env("HI_DEROLA", "1");
    if let Some(w) = workdir.map(str::trim).filter(|s| !s.is_empty()) {
        if !std::path::Path::new(w).is_dir() {
            bail!("bash: workdir not found: {w}");
        }
        command.current_dir(w);
    }
    // the shell gets its own process group so a timeout can reap the whole
    // tree (children, servers it spawned), not just the shell itself
    #[cfg(unix)]
    command.process_group(0);

    let child = command.spawn()?;
    #[cfg(unix)]
    let pgid = child.id();
    #[cfg(windows)]
    let job = child.raw_handle().and_then(crate::winjob::Job::attach);

    let wait = child.wait_with_output();
    let res = match timeout {
        Some(t) => match tokio::time::timeout(std::time::Duration::from_secs(t.max(1)), wait).await
        {
            Err(_) => {
                // kill the direct child (kill_on_drop fires when the dropped
                // future unwinds) plus everything it spawned: SIGTERM first
                // so the tree can clean up, SIGKILL after a short grace
                #[cfg(unix)]
                if let Some(pid) = pgid {
                    let pgid = pid as libc::pid_t;
                    unsafe { libc::killpg(pgid, libc::SIGTERM) };
                    let deadline = std::time::Instant::now() + KILL_GRACE;
                    while std::time::Instant::now() < deadline
                        && unsafe { libc::killpg(pgid, 0) } == 0
                    {
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
                    unsafe { libc::killpg(pgid, libc::SIGKILL) };
                }
                #[cfg(windows)]
                if let Some(j) = &job {
                    j.terminate();
                }
                return Ok(format!("command timed out ({t}s)"));
            }
            Ok(res) => res?,
        },
        None => wait.await?,
    };
    #[cfg(windows)]
    drop(job);
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
        text.push_str(&format!("\nexit code: {}", res.status.code().unwrap_or(-1)));
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

static SHELL_OVERRIDE: OnceLock<Option<String>> = OnceLock::new();

/// set the shell used by the bash tool, from [agent] shell in the config;
/// a program name or a full path, empty strings fall back to the default
pub fn set_shell(prog: Option<String>) {
    let _ = SHELL_OVERRIDE.set(prog);
}

/// command-line flag that makes the shell run one command string;
/// derived from the program basename so both "pwsh" and full paths work
fn shell_args(prog: &str) -> &'static str {
    let base = prog
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(prog)
        .trim_end_matches(".exe")
        .to_lowercase();
    match base.as_str() {
        "cmd" => "/C",
        "powershell" | "pwsh" => "-Command",
        _ => "-c",
    }
}

fn shell() -> (String, &'static str) {
    let over = SHELL_OVERRIDE
        .get()
        .and_then(|o| o.as_deref())
        .filter(|s| !s.trim().is_empty());
    match over {
        Some(p) => (p.trim().to_string(), shell_args(p)),
        None => {
            if cfg!(windows) {
                ("cmd".into(), "/C")
            } else {
                ("sh".into(), "-c")
            }
        }
    }
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

const FUZZY_MIN: f64 = 0.85;
const FUZZY_HINT: f64 = 0.6;

/// char-level Levenshtein distance (two-row DP; edit lines are short)
fn lev(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// similarity of two lines ignoring surrounding whitespace, 0.0..=1.0
fn line_sim(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.trim().chars().collect();
    let b: Vec<char> = b.trim().chars().collect();
    if a == b {
        return 1.0;
    }
    let m = a.len().max(b.len());
    if m == 0 {
        return 1.0;
    }
    // the distance is at least the length gap; skip the DP when the
    // threshold is already unreachable
    if (a.len() as f64 - b.len() as f64).abs() / m as f64 >= 1.0 - FUZZY_MIN {
        return 0.0;
    }
    (1.0 - lev(&a, &b) as f64 / m as f64).max(0.0)
}

enum Shift {
    None,
    Add(String),
    Sub(usize),
}

/// the uniform indentation difference between the old block and where it
/// actually matched in the file (the model often writes the block at column
/// 0 while the file has it nested); applied to the replacement lines
fn indent_shift(c_lines: &[&str], at: usize, o_lines: &[&str]) -> Shift {
    for (j, o) in o_lines.iter().enumerate() {
        if o.trim().is_empty() {
            continue;
        }
        let oc = indent_of(c_lines[at + j]);
        let oo = indent_of(o);
        if oc.starts_with(oo) && oc.len() > oo.len() {
            return Shift::Add(oc[oo.len()..].to_string());
        }
        if oo.starts_with(oc) && oo.len() > oc.len() {
            return Shift::Sub(oo.len() - oc.len());
        }
        return Shift::None;
    }
    Shift::None
}

fn indent_of(line: &str) -> &str {
    let t = line.trim_start();
    &line[..line.len() - t.len()]
}

fn apply_shift(line: &str, shift: &Shift) -> String {
    match shift {
        Shift::None => line.to_string(),
        Shift::Add(p) => {
            if line.trim().is_empty() {
                line.to_string()
            } else {
                format!("{p}{line}")
            }
        }
        Shift::Sub(n) => {
            if line.trim().is_empty() {
                return line.to_string();
            }
            let mut removed = 0usize;
            let mut cut = line.len();
            for (idx, c) in line.char_indices() {
                if removed >= *n || !c.is_whitespace() {
                    cut = idx;
                    break;
                }
                removed += 1;
            }
            line[cut..].to_string()
        }
    }
}

/// last-resort match for old_str that no longer matches the file letter for
/// letter (the model misremembers a token or two). Slides the old block over
/// the file line by line and accepts windows whose average per-line
/// similarity (ignoring surrounding whitespace) is >= FUZZY_MIN. Single-line
/// olds are never fuzzy-matched ("x = 1" vs "x = 2" is already 90% similar).
/// The replacement is spliced with the matched block's indentation shift.
fn fuzzy_edit(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> anyhow::Result<Option<(String, usize)>> {
    let c_lines: Vec<&str> = content.lines().collect();
    let o_lines: Vec<&str> = old.lines().collect();
    if o_lines.len() < 2 || c_lines.len() < o_lines.len() {
        return Ok(None);
    }
    if o_lines.iter().all(|l| l.trim().is_empty()) {
        return Ok(None);
    }
    let n_lines: Vec<String> = new.lines().map(|s| s.to_string()).collect();
    let olen = o_lines.len();
    let mut hits: Vec<usize> = Vec::new();
    for i in 0..=c_lines.len() - olen {
        let mut sum = 0.0f64;
        for (j, o) in o_lines.iter().enumerate() {
            sum += line_sim(c_lines[i + j], o);
        }
        if sum / olen as f64 >= FUZZY_MIN {
            hits.push(i);
        }
    }
    if hits.is_empty() {
        return Ok(None);
    }
    if hits.len() > 1 && !replace_all {
        bail!(
            "old_str is close to {} different places, add context or set replace_all",
            hits.len()
        );
    }
    let count = hits.len();
    let mut lines: Vec<String> = c_lines.iter().map(|s| s.to_string()).collect();
    for i in hits.into_iter().rev() {
        let shift = indent_shift(&c_lines, i, &o_lines);
        let repl: Vec<String> = n_lines.iter().map(|l| apply_shift(l, &shift)).collect();
        lines.splice(i..i + olen, repl);
    }
    let mut joined = lines.join("\n");
    if content.ends_with('\n') && !joined.is_empty() {
        joined.push('\n');
    }
    Ok(Some((joined, count)))
}

/// best near-miss block, used only for the "not found" error hint
fn fuzzy_best(content: &str, old: &str) -> Option<(usize, f64)> {
    let c_lines: Vec<&str> = content.lines().collect();
    let o_lines: Vec<&str> = old.lines().collect();
    if o_lines.len() < 2 || c_lines.len() < o_lines.len() {
        return None;
    }
    let olen = o_lines.len();
    let mut best: Option<(usize, f64)> = None;
    for i in 0..=c_lines.len() - olen {
        let mut sum = 0.0f64;
        for (j, o) in o_lines.iter().enumerate() {
            sum += line_sim(c_lines[i + j], o);
        }
        let avg = sum / olen as f64;
        if best.map(|(_, b)| avg > b).unwrap_or(true) {
            best = Some((i + 1, avg));
        }
    }
    best
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

pub fn apply_edit(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Result<(String, usize)> {
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
    match fuzzy_edit(&norm, &old_n, &new_n, replace_all) {
        Ok(Some((out, n))) => return Ok((with_bom(&restore_eol(&out, crlf), bom), n)),
        Ok(None) => {}
        Err(_) => multi = true,
    }
    if multi || (!old_n.is_empty() && norm.matches(&old_n).count() > 1) {
        bail!("old_str matches multiple times, add context or set replace_all");
    }
    if let Some((line_no, score)) = fuzzy_best(&norm, &old_n).filter(|(_, s)| *s >= FUZZY_HINT) {
        bail!(
            "old_str not found; the closest block is at line {line_no} ({:.0}% similar) - read the file again and copy it exactly",
            score * 100.0
        );
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
        let (out, n) =
            apply_edit(content, "const s = \"hello\";", "const s = \"bye\";", false).unwrap();
        assert_eq!(n, 1);
        assert!(out.contains("const s = \"bye\";"));
        assert!(
            out.contains('\u{2013}'),
            "untouched chars must stay original"
        );
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
    fn spill_under_threshold_is_untouched() {
        let tmp = std::env::temp_dir().join("hi-derola-spill-under");
        let _ = std::fs::remove_dir_all(&tmp);
        let out = "x".repeat(SPILL_MIN - 1);
        let res = spill_into(&tmp, out.clone(), "bash", 4096);
        assert_eq!(res, budget(out, 4096));
        assert!(!tmp.exists() || std::fs::read_dir(&tmp).unwrap().next().is_none());
    }

    #[test]
    fn spill_triggers_on_line_count_too() {
        let tmp = std::env::temp_dir().join("hi-derola-spill-lines");
        let _ = std::fs::remove_dir_all(&tmp);
        // ~10KB of many short lines: under SPILL_MIN, over SPILL_LINES
        let out = "line\n".repeat(SPILL_LINES + 1);
        assert!(out.len() < SPILL_MIN);
        let res = spill_into(&tmp, out.clone(), "grep", 4096);
        assert!(res.contains("full output saved to "), "{res}");
        let mut entries: Vec<_> = std::fs::read_dir(&tmp)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            std::fs::read_to_string(entries.pop().unwrap()).unwrap(),
            out
        );
        // exactly at the line threshold (2000 lines) nothing spills
        let tmp2 = std::env::temp_dir().join("hi-derola-spill-lines-edge");
        let _ = std::fs::remove_dir_all(&tmp2);
        let res2 = spill_into(&tmp2, "l\n".repeat(SPILL_LINES), "grep", 4096);
        assert_eq!(res2, budget("l\n".repeat(SPILL_LINES), 4096));
        assert!(!tmp2.exists() || std::fs::read_dir(&tmp2).unwrap().next().is_none());
    }

    #[test]
    fn spill_over_threshold_saves_full_output() {
        let tmp = std::env::temp_dir().join("hi-derola-spill-over");
        let _ = std::fs::remove_dir_all(&tmp);
        let mut out = "HEAD_MARKER\n".to_string();
        out.push_str(&"x".repeat(SPILL_MIN + 100));
        out.push_str("TAIL_MARKER");
        let res = spill_into(&tmp, out.clone(), "bash", 4096);
        assert!(res.contains("full output saved to "), "{res}");
        assert!(res.contains("TAIL_MARKER"), "budgeted tail keeps the end");
        assert!(
            res.ends_with("cut off here]"),
            "the path hint closes the reply"
        );
        // exactly one spill file and it holds the verbatim output
        let mut entries: Vec<_> = std::fs::read_dir(&tmp)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
        assert_eq!(entries.len(), 1, "exactly one spill file");
        let file = entries.pop().unwrap();
        let saved = std::fs::read_to_string(&file).unwrap();
        assert_eq!(saved, out, "spill file must hold the full output");
        assert!(saved.starts_with("HEAD_MARKER\n"));
        assert!(file
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("bash-"));
    }

    #[test]
    fn spill_cleanup_deletes_old_files() {
        let tmp = std::env::temp_dir().join("hi-derola-spill-retention");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let old = tmp.join("bash-old.txt");
        std::fs::write(&old, "stale").unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(&old).unwrap();
        let stale = std::time::SystemTime::now() - std::time::Duration::from_secs(8 * 24 * 3600);
        f.set_times(std::fs::FileTimes::new().set_modified(stale))
            .unwrap();
        drop(f);
        let fresh = tmp.join("bash-fresh.txt");
        std::fs::write(&fresh, "keep me").unwrap();
        let out = "y".repeat(SPILL_MIN + 10);
        spill_into(&tmp, out, "bash", 4096);
        assert!(!old.exists(), "stale spill file must be removed");
        assert!(fresh.exists(), "fresh files survive");
        assert_eq!(std::fs::read_dir(&tmp).unwrap().flatten().count(), 2);
    }

    #[test]
    fn spill_zero_budget_skips_file() {
        let tmp = std::env::temp_dir().join("hi-derola-spill-unlimited");
        let _ = std::fs::remove_dir_all(&tmp);
        let out = "z".repeat(SPILL_MIN + 10);
        let res = spill_into(&tmp, out, "bash", 0);
        assert_eq!(
            res.len(),
            SPILL_MIN + 10,
            "unlimited budget returns everything"
        );
        assert!(!tmp.exists() || std::fs::read_dir(&tmp).unwrap().next().is_none());
    }

    #[test]
    fn path_in_dir_semantics() {
        let tmp = std::env::temp_dir().join("hi-derola-spill-pid");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let inside = tmp.join("bash-1.txt");
        std::fs::write(&inside, "x").unwrap();
        assert!(path_in_dir(&tmp, &inside.display().to_string()));
        assert!(path_in_dir(&tmp, &format!("{}/", tmp.display())));
        let outside = std::env::temp_dir().join("hi-derola-spill-pid-outside.txt");
        let _ = std::fs::remove_file(&outside);
        std::fs::write(&outside, "x").unwrap();
        assert!(!path_in_dir(&tmp, &outside.display().to_string()));
        assert!(
            !path_in_dir(&tmp, "src/main.rs"),
            "relative non-existent paths are not in"
        );
        assert!(!path_in_dir(
            &std::env::temp_dir().join("hi-derola-no-such-base"),
            &inside.display().to_string()
        ));
    }

    #[test]
    fn edit_tolerant_matching() {
        let crlf = "fn main() {\r\n    let x = 1;\r\n    println!(\"{}\", x);\r\n}\r\n";
        let (out, n) = apply_edit(
            crlf,
            "let x = 1;\n    println",
            "let x = 2;\n    println",
            false,
        )
        .unwrap();
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
        let (out, n) = apply_edit(
            ws,
            "fn a() {\n    let y = 2;\n}",
            "fn a() {\n    let y = 3;\n}",
            false,
        )
        .unwrap();
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
    fn fuzzy_last_resort_edit() {
        // one misremembered token in a multi-line block still lands
        let content = "fn main() {\n    let length = 10;\n    let width = 3;\n    let area = length * width;\n}\n";
        let old = "fn main() {\n    let lenght = 10;\n    let width = 3;\n    let area = length * width;\n}";
        let (out, n) = apply_edit(
            content,
            old,
            "fn main() {\n    let length = 10;\n    let width = 3;\n    let area = length * width;\n    println!(\"{area}\");\n}",
            false,
        )
        .unwrap();
        assert_eq!(n, 1);
        assert!(out.contains("println!(\"{area}\");"), "{out}");

        // indentation-only mismatch: the block is nested in the file while
        // old_str sits at column 0; the replacement is re-indented
        let content = "fn f() {\n    if ok {\n        go();\n    }\n}\n";
        let (out, n) = apply_edit(
            content,
            "if ok {\n    go();\n}",
            "if ok {\n    go_fast();\n}",
            false,
        )
        .unwrap();
        assert_eq!(n, 1);
        assert_eq!(out, "fn f() {\n    if ok {\n        go_fast();\n    }\n}\n");

        // too different -> plain not found, no misleading hint
        let err = apply_edit(
            "one\ntwo\nthree\n",
            "one\ntotally different\nfive",
            "x",
            false,
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("old_str not found") && !err.contains("% similar"),
            "{err}"
        );

        // close but under the threshold -> hint pointing at the best block
        let err = apply_edit("alpha one\nbeta two\n", "alpha ona\nbeta xy", "x", false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("% similar"), "{err}");

        // single-line old never fuzzy-matches, even at 90% similarity
        assert!(apply_edit("let x = 1;\n", "let x = 2;", "let x = 3;", false).is_err());

        // ambiguous near-matches error without replace_all ...
        let dup = "alpha one\nbeta two\nalpha one\nbeta two\n";
        let err = apply_edit(dup, "alpha one\nbeta too", "REPL", false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("matches multiple times"), "{err}");
        // ... and with replace_all every near-miss is replaced
        let (out, n) = apply_edit(dup, "alpha one\nbeta too", "ALPHA\nBETA", true).unwrap();
        assert_eq!(n, 2);
        assert_eq!(out, "ALPHA\nBETA\nALPHA\nBETA\n");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bash_timeout_kills_tree() {
        let dir = std::env::temp_dir().join(format!("hiderola-kill-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("alive");
        // the grandchild `sleep 2` would touch the marker at t=2.3s if it
        // survived; the timeout fires at t=1s and must reap the whole tree
        let cmd = format!("sleep 0.3 && sleep 2 && touch {}", marker.display());
        let t0 = std::time::Instant::now();
        let out = bash_run(&cmd, None, Some(1)).await.unwrap();
        assert!(out.contains("timed out"), "{out}");
        assert!(
            t0.elapsed() < std::time::Duration::from_secs(4),
            "timeout must fire early, not wait for the command"
        );
        std::thread::sleep(std::time::Duration::from_secs(2));
        assert!(!marker.exists(), "grandchild survived the timeout kill");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bash_timeout_escalates_to_sigkill() {
        let dir = std::env::temp_dir().join(format!("hiderola-escalate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("alive");
        // the shell ignores SIGTERM, so only the SIGKILL escalation can stop
        // it: without the escalation the marker appears at t=2.3s, while the
        // check happens at t~3.1s
        let cmd = format!(
            "trap '' TERM; sleep 0.3 && sleep 2 && touch {}",
            marker.display()
        );
        let out = bash_run(&cmd, None, Some(1)).await.unwrap();
        assert!(out.contains("timed out"), "{out}");
        std::thread::sleep(std::time::Duration::from_secs(2));
        assert!(!marker.exists(), "TERM-immune shell survived the timeout");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bash_run_captures_output_and_code() {
        let out = bash_run("echo hello >&2; echo hi; exit 3", None, Some(30))
            .await
            .unwrap();
        assert!(out.contains("hi"));
        assert!(out.contains("hello"));
        assert!(out.contains("exit code: 3"), "{out}");
        let out = bash_run("echo ok", None, None).await.unwrap();
        assert_eq!(out.trim(), "ok");
    }

    #[test]
    fn shell_flag_derivation() {
        assert_eq!(shell_args("cmd"), "/C");
        assert_eq!(shell_args("cmd.exe"), "/C");
        assert_eq!(shell_args("pwsh"), "-Command");
        assert_eq!(shell_args("pwsh.exe"), "-Command");
        assert_eq!(
            shell_args("C:\\Program Files\\PowerShell\\7\\pwsh.exe"),
            "-Command"
        );
        assert_eq!(shell_args("bash"), "-c");
        assert_eq!(shell_args("/usr/bin/zsh"), "-c");
        assert_eq!(shell_args("/opt/homebrew/bin/fish"), "-c");
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
