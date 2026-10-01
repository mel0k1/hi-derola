use anyhow::{anyhow, Result};
use hi_derola::agent;
use hi_derola::chat::{Role, Session};
use hi_derola::config::Config;
use hi_derola::mcp::{self, McpClient};
use hi_derola::provider::{self, ApiEvent, ChatRequest, Provider};
use hi_derola::sessions::{self, SessionMeta, StoredSession};
use hi_derola::{models, snapshot, tools};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{mpsc, oneshot};

pub struct Shared {
    cfg: Mutex<Config>,
    provider: Mutex<Option<Arc<dyn Provider>>>,
    session: Mutex<Session>,
    sid: Mutex<String>,
    title: Mutex<String>,
    created: Mutex<u64>,
    confirm: Mutex<Option<oneshot::Sender<bool>>>,
    inflight: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    tokens: Mutex<(u64, u64)>,
    cost: Mutex<f64>,
    attachments: Mutex<Vec<(String, String)>>,
    mcp: Mutex<Option<Arc<McpClient>>>,
    allow_all: Arc<AtomicBool>,
    queue: Arc<Mutex<Vec<String>>>,
    titled: AtomicBool,
    tx: mpsc::UnboundedSender<ApiEvent>,
}

fn system_prompt() -> String {
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let mut system = format!(
        "You are hi-derola, a coding assistant running on the user's machine.\n\
         Working directory: {cwd}\n\
         Be concise and practical. Use markdown for formatting.\n\n\
         Use the provided tools (read_file, write_file, edit, glob, grep, list_files, bash) \
         to work with files and run commands instead of printing code fences with file \
         contents. Use glob and grep to locate code before reading. \
         Prefer read_file before modifying a file. \
         write_file writes the complete file content."
    );
    let agents = hi_derola::agents_md();
    if !agents.is_empty() {
        system.push_str("\n\n");
        system.push_str(&agents);
    }
    system
}

fn diff_json(rows: Vec<hi_derola::diff::Row>) -> Value {
    Value::Array(
        rows.into_iter()
            .map(|r| json!({"tag": r.tag, "text": r.text}))
            .collect(),
    )
}

fn confirm_payload(name: &str, args: &str) -> Value {
    let head = tools::detail(name, args);
    json!({
        "t": "confirm",
        "name": name,
        "detail": head,
        "diff": diff_json(tools::preview(name, args)),
    })
}

fn persist(sh: &Shared) {
    let ses = sh.session.lock().unwrap();
    if ses.messages.is_empty() {
        return;
    }
    let st = StoredSession {
        id: sh.sid.lock().unwrap().clone(),
        title: sh.title.lock().unwrap().clone(),
        created: *sh.created.lock().unwrap(),
        updated: 0,
        system: ses.system.clone(),
        messages: ses.messages.clone(),
        tokens_in: sh.tokens.lock().unwrap().0,
        tokens_out: sh.tokens.lock().unwrap().1,
        cost: *sh.cost.lock().unwrap(),
    };
    drop(ses);
    let _ = sessions::save(&st);
}

fn emit_sessions(app: &AppHandle, sh: &Shared) {
    let sid = sh.sid.lock().unwrap().clone();
    let _ = app.emit("ev", json!({"t": "sessions", "list": sessions::list(), "sid": sid}));
}

fn emit_attachments(sh: &Shared, app: &AppHandle) {
    let list: Vec<String> = sh.attachments.lock().unwrap().iter().map(|(p, _)| p.clone()).collect();
    let _ = app.emit("ev", json!({"t": "attachments", "list": list}));
}

fn clip_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn launch(sh: &Arc<Shared>) -> Result<(), String> {
    let Some(provider) = sh.provider.lock().unwrap().clone() else {
        return Err("no api key: open settings and add one".into());
    };
    let cfg = sh.cfg.lock().unwrap().clone();
    let (system, messages) = {
        let ses = sh.session.lock().unwrap();
        (ses.system.clone(), ses.messages.clone())
    };
    let req = ChatRequest {
        system,
        messages,
        model: cfg.provider.model.clone(),
        max_tokens: cfg.provider.max_tokens,
        temperature: cfg.provider.temperature,
        top_p: cfg.provider.top_p,
        stream: cfg.provider.stream,
        tools: Vec::new(),
    };
    let mcp = sh.mcp.lock().unwrap().clone();
    let agent_cfg = hi_derola::agent::AgentCfg {
        context_limit: cfg.agent.context_limit,
        max_rounds: cfg.agent.max_rounds,
        output_budget: cfg.agent.output_budget,
        perm: cfg.permissions.clone(),
    };
    let sh2 = sh.clone();
    let handle = tauri::async_runtime::spawn(async move {
        if let Err(e) = agent::run(
            provider,
            req,
            sh2.tx.clone(),
            sh2.allow_all.clone(),
            mcp,
            sh2.queue.clone(),
            agent_cfg,
        )
        .await
        {
            let _ = sh2.tx.send(ApiEvent::Failed(format!("{e:#}")));
        }
    });
    *sh.inflight.lock().unwrap() = Some(handle);
    Ok(())
}

fn resume_queue(app: &AppHandle, sh: &Arc<Shared>) {
    let next = {
        let mut q = sh.queue.lock().unwrap();
        if q.is_empty() {
            None
        } else {
            Some(q.remove(0))
        }
    };
    let Some(composed) = next else {
        return;
    };
    {
        let mut ses = sh.session.lock().unwrap();
        ses.push(Role::User, composed);
    }
    persist(sh);
    emit_sessions(app, sh);
    let _ = app.emit("ev", json!({"t": "queued"}));
    if let Err(e) = launch(sh) {
        let _ = app.emit("ev", json!({"t": "failed", "s": e}));
    }
}

fn autotitle(app: &AppHandle, sh: &Arc<Shared>, msgs: &[hi_derola::chat::Message]) {
    if msgs.len() < 2 {
        return;
    }
    let Some(provider) = sh.provider.lock().unwrap().clone() else {
        return;
    };
    if sh.titled.swap(true, Ordering::Relaxed) {
        return;
    }
    let mut user_text = String::new();
    let mut bot_text = String::new();
    for m in msgs {
        match m.role {
            Role::User if user_text.is_empty() => user_text = m.content.clone(),
            Role::Assistant if !m.content.trim().is_empty() && bot_text.is_empty() => {
                bot_text = m.content.clone();
            }
            _ => {}
        }
        if !user_text.is_empty() && !bot_text.is_empty() {
            break;
        }
    }
    if user_text.trim().is_empty() {
        return;
    }
    let cfg = sh.cfg.lock().unwrap().clone();
    let req = ChatRequest {
        system: "You generate short chat session titles. Reply with only the title: 2-6 words in the language of the message, no quotes, no trailing punctuation.".into(),
        messages: vec![hi_derola::chat::Message::new(
            Role::User,
            format!(
                "User message:\n{}\n\nAssistant reply:\n{}\n\nThe title is:",
                clip_chars(&user_text, 600),
                clip_chars(&bot_text, 400)
            ),
        )],
        model: cfg.provider.model.clone(),
        max_tokens: cfg.provider.max_tokens,
        temperature: cfg.provider.temperature,
        top_p: cfg.provider.top_p,
        stream: false,
        tools: Vec::new(),
    };
    let app2 = app.clone();
    let sh2 = sh.clone();
    tauri::async_runtime::spawn(async move {
        let (btx, _brx) = mpsc::unbounded_channel();
        let Ok(r) = provider.chat(&req, &btx).await else {
            return;
        };
        let raw = r.text.lines().next().unwrap_or("").trim();
        let raw = raw.trim_matches(|c| c == '"' || c == '\'');
        let t: String = raw.chars().take(48).collect();
        if t.trim().is_empty() {
            return;
        }
        *sh2.title.lock().unwrap() = t;
        persist(&sh2);
        emit_sessions(&app2, &sh2);
    });
}

fn pump(mut rx: mpsc::UnboundedReceiver<ApiEvent>, app: AppHandle, sh: Arc<Shared>) {
    tauri::async_runtime::spawn(async move {
        while let Some(ev) = rx.recv().await {
            let mut resume = false;
            let payload = match ev {
                ApiEvent::Chunk(s) => json!({"t": "chunk", "s": s}),
                ApiEvent::Reasoning(s) => json!({"t": "reasoning", "s": s}),
                ApiEvent::Note(s) => json!({"t": "note", "s": s}),
                ApiEvent::Tool { name, detail, diff } => json!({
                    "t": "tool", "name": name, "detail": detail, "diff": diff_json(diff),
                }),
                ApiEvent::Confirm { name, args, rx } => {
                    *sh.confirm.lock().unwrap() = Some(rx);
                    confirm_payload(&name, &args)
                }
                ApiEvent::Usage { input, output } => {
                    let model = sh.cfg.lock().unwrap().provider.model.clone();
                    let delta = models::cost(&model, input, output);
                    let mut t = sh.tokens.lock().unwrap();
                    t.0 += input;
                    t.1 += output;
                    drop(t);
                    let mut c = sh.cost.lock().unwrap();
                    *c += delta;
                    let total = *c;
                    drop(c);
                    json!({"t": "usage", "input": input, "output": output, "cost": total})
                }
                ApiEvent::Done { text, messages } => {
                    autotitle(&app, &sh, &messages);
                    sh.session.lock().unwrap().messages = messages;
                    snapshot::end_turn();
                    persist(&sh);
                    emit_sessions(&app, &sh);
                    resume = true;
                    json!({"t": "done", "text": text})
                }
                ApiEvent::Failed(e) => {
                    snapshot::end_turn();
                    resume = true;
                    json!({"t": "failed", "s": e})
                }
            };
            if payload.get("t").and_then(|t| t.as_str()) == Some("note") {
                if payload.get("s").and_then(|s| s.as_str()) == Some("") {
                    continue;
                }
            }
            let _ = app.emit("ev", payload);
            if resume {
                resume_queue(&app, &sh);
            }
        }
    });
}

fn start_new(sh: &Shared, app: &AppHandle) {
    persist(sh);
    sh.session.lock().unwrap().clear();
    sh.attachments.lock().unwrap().clear();
    sh.allow_all.store(false, Ordering::Relaxed);
    sh.titled.store(false, Ordering::Relaxed);
    *sh.sid.lock().unwrap() = sessions::new_id();
    *sh.title.lock().unwrap() = String::new();
    *sh.created.lock().unwrap() = 0;
    *sh.tokens.lock().unwrap() = (0, 0);
    *sh.cost.lock().unwrap() = 0.0;
    let _ = app.emit("ev", json!({"t": "cleared"}));
    emit_sessions(app, sh);
    emit_attachments(sh, app);
}

#[tauri::command]
fn init(sh: State<'_, Arc<Shared>>) -> Value {
    let cfg = sh.cfg.lock().unwrap().clone();
    json!({
        "cfg": cfg,
        "keys": cfg.keys(),
        "cwd": std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default(),
        "config_path": hi_derola::config::config_path().display().to_string(),
        "has_provider": sh.provider.lock().unwrap().is_some(),
        "sessions": sessions::list(),
        "sid": sh.sid.lock().unwrap().clone(),
        "title": sh.title.lock().unwrap().clone(),
        "theme": cfg.ui.theme.clone(),
    })
}

#[tauri::command]
fn save(sh: State<'_, Arc<Shared>>, app: AppHandle, cfg: Config) -> Result<Value, String> {
    let p = match cfg.api_key() {
        Some(k) => match provider::build(&cfg.provider.kind, cfg.provider.base_url.clone(), k) {
            Ok(p) => Some(p),
            Err(e) => return Err(format!("{e:#}")),
        },
        None => None,
    };
    cfg.save().map_err(|e| format!("{e:#}"))?;
    let model = cfg.provider.model.clone();
    let kind = cfg.provider.kind.clone();
    let theme = cfg.ui.theme.clone();
    *sh.cfg.lock().unwrap() = cfg;
    *sh.provider.lock().unwrap() = p;
    let _ = app.emit("ev", json!({"t": "model", "name": model, "kind": kind}));
    if let Some(t) = theme {
        let _ = app.emit("ev", json!({"t": "theme", "name": t}));
    }
    Ok(json!({"ok": true}))
}

#[tauri::command]
fn set_theme(sh: State<'_, Arc<Shared>>, theme: String) -> Result<Value, String> {
    let mut cfg = sh.cfg.lock().unwrap();
    cfg.ui.theme = Some(theme.clone());
    cfg.save().map_err(|e| format!("{e:#}"))?;
    Ok(json!({"ok": true, "theme": theme}))
}

#[tauri::command]
async fn list_models(
    sh: State<'_, Arc<Shared>>,
    kind: Option<String>,
    base_url: Option<String>,
    api_key: Option<String>,
) -> Result<Vec<String>, String> {
    let cfg = sh.cfg.lock().unwrap().clone();
    let kind = kind.filter(|s| !s.trim().is_empty()).unwrap_or(cfg.provider.kind.clone());
    let base = base_url.filter(|s| !s.trim().is_empty()).or(cfg.provider.base_url.clone());
    let key = api_key
        .filter(|s| !s.trim().is_empty())
        .or_else(|| cfg.api_key())
        .unwrap_or_default();
    provider::list_models(&kind, base.as_deref(), &key)
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn confirm(sh: State<'_, Arc<Shared>>, ok: bool) {
    if let Some(c) = sh.confirm.lock().unwrap().take() {
        let _ = c.send(ok);
    }
}

#[tauri::command]
fn allow_all(sh: State<'_, Arc<Shared>>) {
    sh.allow_all.store(true, Ordering::Relaxed);
    if let Some(c) = sh.confirm.lock().unwrap().take() {
        let _ = c.send(true);
    }
}

#[tauri::command]
fn stop(sh: State<'_, Arc<Shared>>, app: AppHandle) {
    if let Some(h) = sh.inflight.lock().unwrap().take() {
        h.abort();
    }
    if let Some(c) = sh.confirm.lock().unwrap().take() {
        let _ = c.send(false);
    }
    snapshot::end_turn();
    let _ = app.emit("ev", json!({"t": "note", "s": "cancelled"}));
    let empty = sh.queue.lock().unwrap().is_empty();
    if empty {
        let _ = app.emit("ev", json!({"t": "idle"}));
    } else {
        resume_queue(&app, &sh);
    }
}

#[tauri::command]
async fn mcp_reconnect(sh: State<'_, Arc<Shared>>) -> Result<Vec<String>, String> {
    let cfgs = sh.cfg.lock().unwrap().mcp.clone();
    let (client, logs) = mcp::connect_all(&cfgs).await;
    *sh.mcp.lock().unwrap() = client;
    Ok(logs)
}

#[tauri::command]
fn undo(sh: State<'_, Arc<Shared>>) -> Option<String> {
    let _ = &sh;
    snapshot::undo()
}

#[tauri::command]
fn redo(sh: State<'_, Arc<Shared>>) -> Option<String> {
    let _ = &sh;
    snapshot::redo()
}

fn transcript(msgs: &[hi_derola::chat::Message]) -> Vec<Value> {
    let mut out = Vec::new();
    for m in msgs {
        match m.role {
            Role::User => out.push(json!({"k": "user", "s": m.content})),
            Role::Assistant => {
                if !m.content.trim().is_empty() {
                    out.push(json!({"k": "bot", "s": m.content}));
                }
                for c in &m.tool_calls {
                    out.push(json!({"k": "tool", "s": format!("{} {}", c.name, c.args)}));
                }
            }
            Role::Tool => {
                if !m.content.trim().is_empty() {
                    let head: String = m.content.lines().take(6).collect::<Vec<_>>().join("\n");
                    out.push(json!({"k": "toolout", "s": head}));
                }
            }
        }
    }
    out
}

#[tauri::command]
fn list_sessions() -> Vec<SessionMeta> {
    sessions::list()
}

#[tauri::command]
fn new_session(sh: State<'_, Arc<Shared>>, app: AppHandle) {
    start_new(&sh, &app);
}

#[tauri::command]
fn open_session(sh: State<'_, Arc<Shared>>, app: AppHandle, id: String) -> Result<Value, String> {
    if *sh.sid.lock().unwrap() != id {
        persist(&sh);
    }
    let st = sessions::load(&id).map_err(|e| format!("{e:#}"))?;
    *sh.session.lock().unwrap() = Session {
        system: if st.system.trim().is_empty() {
            system_prompt()
        } else {
            st.system.clone()
        },
        messages: st.messages.clone(),
    };
    *sh.sid.lock().unwrap() = st.id.clone();
    *sh.title.lock().unwrap() = st.title.clone();
    *sh.created.lock().unwrap() = st.created;
    *sh.tokens.lock().unwrap() = (st.tokens_in, st.tokens_out);
    *sh.cost.lock().unwrap() = st.cost;
    sh.titled.store(true, Ordering::Relaxed);
    sh.attachments.lock().unwrap().clear();
    sh.confirm.lock().unwrap().take();
    emit_attachments(&sh, &app);
    Ok(json!({
        "id": st.id,
        "title": st.title,
        "created": st.created,
        "updated": st.updated,
        "tokens_in": st.tokens_in,
        "tokens_out": st.tokens_out,
        "cost": st.cost,
        "transcript": transcript(&st.messages),
    }))
}
#[tauri::command]
fn delete_session(sh: State<'_, Arc<Shared>>, app: AppHandle, id: String) -> Result<Value, String> {
    sessions::delete(&id).map_err(|e| format!("{e:#}"))?;
    let current = { *sh.sid.lock().unwrap() == id };
    if current {
        sh.session.lock().unwrap().clear();
        sh.attachments.lock().unwrap().clear();
        sh.allow_all.store(false, Ordering::Relaxed);
        sh.titled.store(false, Ordering::Relaxed);
        *sh.sid.lock().unwrap() = sessions::new_id();
        *sh.title.lock().unwrap() = String::new();
        *sh.created.lock().unwrap() = 0;
        *sh.tokens.lock().unwrap() = (0, 0);
        *sh.cost.lock().unwrap() = 0.0;
        let _ = app.emit("ev", json!({"t": "cleared"}));
    }
    emit_sessions(&app, &sh);
    Ok(json!({"ok": true, "current": current}))
}

fn expand(p: &str) -> std::path::PathBuf {
    if p == "~" {
        return dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    }
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(h) = dirs::home_dir() {
            return h.join(rest);
        }
    }
    std::path::PathBuf::from(p)
}

#[tauri::command]
fn list_dir(path: Option<String>) -> Result<Value, String> {
    let target = match path.as_deref().map(str::trim) {
        None | Some("") => std::env::current_dir().map_err(|e| e.to_string())?,
        Some(p) if p.starts_with('~') => expand(p),
        Some(p) => std::path::PathBuf::from(p),
    };
    let meta = std::fs::metadata(&target).map_err(|e| format!("{e}"))?;
    if meta.is_file() {
        return Err("not a directory".into());
    }
    let rd = std::fs::read_dir(&target).map_err(|e| format!("{e}"))?;
    let mut entries: Vec<(String, bool, u64)> = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Ok(ft) = e.file_type() else { continue };
        let size = e.metadata().map(|m| m.len()).unwrap_or(0);
        entries.push((name, ft.is_dir(), size));
    }
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    entries.truncate(500);
    let cwd = std::env::current_dir().unwrap_or_default();
    let home = dirs::home_dir();
    let mut display = target.display().to_string();
    if let Some(h) = &home {
        if let Some(rest) = display.strip_prefix(&h.display().to_string()) {
            display = format!("~{rest}");
        }
    }
    let mut cwd_display = cwd.display().to_string();
    if let Some(h) = &home {
        if let Some(rest) = cwd_display.strip_prefix(&h.display().to_string()) {
            cwd_display = format!("~{rest}");
        }
    }
    let parent = target.parent().map(|p| p.display().to_string());
    Ok(json!({
        "path": display,
        "cwd": cwd_display,
        "is_cwd": target == cwd,
        "parent": parent,
        "entries": entries.iter().map(|(n, d, s)| json!({"name": n, "dir": d, "size": s})).collect::<Vec<_>>(),
    }))
}

const MAX_DIR_ENTRIES: usize = 400;

#[tauri::command]
fn list_project_files() -> Vec<String> {
    let mut out = Vec::new();
    hi_derola::files::walk_files(".", 0, &mut out);
    out.sort();
    out.into_iter()
        .map(|p| {
            let p = p.trim_start_matches("./");
            hi_derola::files::norm(p)
        })
        .collect()
}

fn dir_tree(root: &std::path::Path, depth: u8, counter: &mut usize) -> String {
    let mut out = String::new();
    let Ok(rd) = std::fs::read_dir(root) else {
        return out;
    };
    let mut items: Vec<_> = rd.flatten().collect();
    items.sort_by_key(|a| a.file_name());
    for e in items {
        if *counter >= MAX_DIR_ENTRIES {
            out.push_str("  ...\n");
            return out;
        }
        *counter += 1;
        let Ok(ft) = e.file_type() else { continue };
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || matches!(name.as_str(), "node_modules" | "target" | ".git") {
            continue;
        }
        if ft.is_dir() {
            if depth == 0 {
                out.push_str(&format!("  {name}/\n"));
            }
            if depth < 2 {
                let sub = dir_tree(&e.path(), depth + 1, counter);
                for line in sub.lines() {
                    out.push_str(&format!("  {line}\n"));
                }
            }
        } else {
            out.push_str(&format!("  {name}\n"));
        }
    }
    out
}

#[tauri::command]
fn attach_path(sh: State<'_, Arc<Shared>>, app: AppHandle, path: String) -> Result<Value, String> {
    let p = expand(path.trim());
    let meta = std::fs::metadata(&p).map_err(|e| format!("{e}"))?;
    if meta.is_dir() {
        let mut counter = 0;
        let mut tree = format!("[folder: {}]\n", p.display());
        tree.push_str(&dir_tree(&p, 0, &mut counter));
        if counter == 0 {
            return Err("empty folder".into());
        }
        let label = format!("{}/ ({} entries)", p.display(), counter);
        sh.attachments.lock().unwrap().push((label, tree));
        emit_attachments(&sh, &app);
        return Ok(json!({"ok": true, "kind": "folder", "entries": counter}));
    }
    let content = hi_derola::files::read_attach(&p.display().to_string()).map_err(|e| format!("{e:#}"))?;
    let size = content.len();
    sh.attachments.lock().unwrap().push((p.display().to_string(), content));
    emit_attachments(&sh, &app);
    Ok(json!({"ok": true, "kind": "file", "size": size}))
}

#[tauri::command]
fn detach(sh: State<'_, Arc<Shared>>, app: AppHandle, index: usize) {
    let mut at = sh.attachments.lock().unwrap();
    if index < at.len() {
        at.remove(index);
    }
    drop(at);
    emit_attachments(&sh, &app);
}

fn note(s: impl Into<String>) -> Value {
    json!({"cmd": true, "note": s.into()})
}

fn command(sh: &Shared, app: &AppHandle, line: &str) -> Value {
    let (cmd, arg) = line
        .split_once(' ')
        .map(|(c, a)| (c, a.trim()))
        .unwrap_or((line, ""));
    match cmd {
        "/help" | "/h" => note(
            "commands: /file <path> · /model <name> · /models · /undo · /redo · /clear · /help\n\
             mutations (write/edit/bash/mcp) ask for confirmation, allow all skips further asks",
        ),
        "/clear" | "/new" => {
            start_new(sh, app);
            note("new session")
        }
        "/model" => {
            if arg.is_empty() {
                let cfg = sh.cfg.lock().unwrap();
                note(format!(
                    "model: {}\nconfig: {}",
                    cfg.provider.model,
                    hi_derola::config::config_path().display()
                ))
            } else {
                {
                    let mut cfg = sh.cfg.lock().unwrap();
                    cfg.provider.model = arg.to_string();
                }
                let saved = sh.cfg.lock().unwrap().save();
                let _ = app.emit("ev", json!({"t": "model", "name": arg}));
                match saved {
                    Ok(_) => note(format!("model: {arg}")),
                    Err(e) => note(format!("model: {arg} (not saved: {e:#})")),
                }
            }
        }
        "/models" => {
            let cfg = sh.cfg.lock().unwrap().clone();
            let key = cfg.api_key().unwrap_or_default();
            let tx = sh.tx.clone();
            tauri::async_runtime::spawn(async move {
                let msg = match provider::list_models(&cfg.provider.kind, cfg.provider.base_url.as_deref(), &key).await {
                    Ok(list) if list.is_empty() => "no models found".into(),
                    Ok(list) => format!("models ({}):\n{}", list.len(), list.join("\n")),
                    Err(e) => format!("error: {e:#}"),
                };
                let _ = tx.send(ApiEvent::Note(msg));
            });
            note("fetching models...")
        }
        "/file" => {
            if arg.is_empty() {
                note("usage: /file <path>")
            } else {
                match hi_derola::files::read_attach(arg) {
                    Ok(content) => {
                        let size = content.len();
                        sh.attachments.lock().unwrap().push((arg.to_string(), content));
                        emit_attachments(sh, app);
                        note(format!("attached {arg} ({size} bytes)"))
                    }
                    Err(e) => note(format!("error: {e:#}")),
                }
            }
        }
        "/undo" | "/u" => note(snapshot::undo().unwrap_or_else(|| "nothing to undo".into())),
        "/redo" => note(snapshot::redo().unwrap_or_else(|| "nothing to redo".into())),
        _ => note(format!("unknown command: {cmd}, try /help")),
    }
}

#[tauri::command]
fn send(sh: State<'_, Arc<Shared>>, app: AppHandle, text: String) -> Result<Value, String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Ok(json!({"cmd": true, "note": ""}));
    }
    if text.starts_with('/') {
        return Ok(command(&sh, &app, &text));
    }
    let mut composed = String::new();
    {
        let mut at = sh.attachments.lock().unwrap();
        for (p, c) in at.iter() {
            composed.push_str(&format!("[file: {p}]\n{c}\n\n"));
        }
        at.clear();
    }
    let (mention_blocks, mention_ok, mention_miss) = hi_derola::files::mentions(&text);
    composed.push_str(&mention_blocks);
    composed.push_str(&text);
    if !mention_ok.is_empty() || !mention_miss.is_empty() {
        let mut note_line = String::new();
        if !mention_ok.is_empty() {
            note_line.push_str(&format!("@mentions attached: {}", mention_ok.join(", ")));
        }
        if !mention_miss.is_empty() {
            if !note_line.is_empty() {
                note_line.push_str(" · ");
            }
            note_line.push_str(&format!("not found: {}", mention_miss.join(", ")));
        }
        let _ = sh.tx.send(ApiEvent::Note(note_line));
    }
    {
        let mut title = sh.title.lock().unwrap();
        if title.trim().is_empty() {
            *title = sessions::title_from(&text);
        }
    }
    if *sh.created.lock().unwrap() == 0 {
        *sh.created.lock().unwrap() = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
    }
    if sh.inflight.lock().unwrap().is_some() {
        sh.queue.lock().unwrap().push(composed);
        emit_attachments(&sh, &app);
        let _ = sh.tx.send(ApiEvent::Note("queued: will steer the current run".into()));
        return Ok(json!({"cmd": false, "queued": true}));
    }
    {
        let mut ses = sh.session.lock().unwrap();
        ses.push(Role::User, composed);
    }
    persist(&sh);
    emit_sessions(&app, &sh);
    emit_attachments(&sh, &app);
    launch(&sh)?;
    Ok(json!({"cmd": false}))
}

pub fn run() -> Result<()> {
    let (cfg, _) = Config::load_or_default()?;
    let cfg = Arc::new(cfg);
    tauri::Builder::default()
        .setup(move |app| {
            let (tx, rx) = mpsc::unbounded_channel::<ApiEvent>();
            let cfg = (*cfg).clone();
            let provider = cfg
                .api_key()
                .and_then(|k| provider::build(&cfg.provider.kind, cfg.provider.base_url.clone(), k).ok());
            let restore = sessions::latest();
            let (sid, title, created, session) = match &restore {
                Some(st) => (
                    st.id.clone(),
                    st.title.clone(),
                    st.created,
                    Session {
                        system: if st.system.trim().is_empty() {
                            system_prompt()
                        } else {
                            st.system.clone()
                        },
                        messages: st.messages.clone(),
                    },
                ),
                None => (sessions::new_id(), String::new(), 0, Session::new(system_prompt())),
            };
            let sh = Arc::new(Shared {
                cfg: Mutex::new(cfg),
                provider: Mutex::new(provider),
                session: Mutex::new(session),
                sid: Mutex::new(sid),
                title: Mutex::new(title),
                created: Mutex::new(created),
                confirm: Mutex::new(None),
                inflight: Mutex::new(None),
                tokens: Mutex::new(restore.as_ref().map(|s| (s.tokens_in, s.tokens_out)).unwrap_or((0, 0))),
                cost: Mutex::new(restore.as_ref().map(|s| s.cost).unwrap_or(0.0)),
                attachments: Mutex::new(Vec::new()),
                mcp: Mutex::new(None),
                allow_all: Arc::new(AtomicBool::new(false)),
                queue: Arc::new(Mutex::new(Vec::new())),
                titled: AtomicBool::new(restore.is_some()),
                tx: tx.clone(),
            });
            let logs = tauri::async_runtime::block_on(async {
                let cfgs = sh.cfg.lock().unwrap().mcp.clone();
                let (client, logs) = mcp::connect_all(&cfgs).await;
                *sh.mcp.lock().unwrap() = client;
                logs
            });
            for l in logs {
                let _ = tx.send(ApiEvent::Note(l));
            }
            pump(rx, app.handle().clone(), sh.clone());
            app.manage(sh);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            init, save, send, confirm, allow_all, stop, list_models, mcp_reconnect, undo, redo,
            list_sessions, new_session, open_session, delete_session, list_dir, attach_path,
            detach, set_theme, list_project_files
        ])
        .run(tauri::generate_context!())
        .map_err(|e| anyhow!("{e}"))?;
    Ok(())
}
