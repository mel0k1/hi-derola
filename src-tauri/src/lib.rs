use anyhow::{anyhow, Result};
use hi_derola::agent;
use hi_derola::chat::{Role, Session};
use hi_derola::config::Config;
use hi_derola::mcp::{self, McpClient};
use hi_derola::provider::{self, ApiEvent, ChatRequest, Provider};
use hi_derola::{snapshot, tools};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{mpsc, oneshot};

pub struct Shared {
    cfg: Mutex<Config>,
    provider: Mutex<Option<Arc<dyn Provider>>>,
    session: Mutex<Session>,
    confirm: Mutex<Option<oneshot::Sender<bool>>>,
    inflight: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    tokens: Mutex<(u64, u64)>,
    attachments: Mutex<Vec<(String, String)>>,
    mcp: Mutex<Option<Arc<McpClient>>>,
    allow_all: Arc<AtomicBool>,
    tx: mpsc::UnboundedSender<ApiEvent>,
}

fn system_prompt() -> String {
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    format!(
        "You are hi-derola, a coding assistant running on the user's machine.\n\
         Working directory: {cwd}\n\
         Be concise and practical. Use markdown for formatting.\n\n\
         Use the provided tools (read_file, write_file, edit, glob, grep, list_files, bash) \
         to work with files and run commands instead of printing code fences with file \
         contents. Use glob and grep to locate code before reading. \
         Prefer read_file before modifying a file. \
         write_file writes the complete file content."
    )
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

fn pump(mut rx: mpsc::UnboundedReceiver<ApiEvent>, app: AppHandle, sh: Arc<Shared>) {
    tauri::async_runtime::spawn(async move {
        while let Some(ev) = rx.recv().await {
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
                    let mut t = sh.tokens.lock().unwrap();
                    t.0 += input;
                    t.1 += output;
                    json!({"t": "usage", "input": input, "output": output})
                }
                ApiEvent::Done { text, messages } => {
                    sh.session.lock().unwrap().messages = messages;
                    snapshot::end_turn();
                    json!({"t": "done", "text": text})
                }
                ApiEvent::Failed(e) => {
                    snapshot::end_turn();
                    json!({"t": "failed", "s": e})
                }
            };
            let _ = app.emit("ev", payload);
        }
    });
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
    *sh.cfg.lock().unwrap() = cfg;
    *sh.provider.lock().unwrap() = p;
    let _ = app.emit("ev", json!({"t": "model", "name": model, "kind": kind}));
    Ok(json!({"ok": true}))
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
    let _ = app.emit("ev", json!({"t": "idle"}));
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
            sh.session.lock().unwrap().clear();
            sh.attachments.lock().unwrap().clear();
            sh.allow_all.store(false, Ordering::Relaxed);
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
            let app2 = app.clone();
            tauri::async_runtime::spawn(async move {
                let msg = match provider::list_models(&cfg.provider.kind, cfg.provider.base_url.as_deref(), &key).await {
                    Ok(list) if list.is_empty() => "no models found".into(),
                    Ok(list) => format!("models ({}):\n{}", list.len(), list.join("\n")),
                    Err(e) => format!("error: {e:#}"),
                };
                let _ = tx.send(ApiEvent::Note(msg));
                let _ = app2;
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
    let provider = sh.provider.lock().unwrap().clone();
    let Some(provider) = provider else {
        return Err("no api key: open settings and add one".into());
    };
    let mut composed = String::new();
    {
        let mut at = sh.attachments.lock().unwrap();
        for (p, c) in at.iter() {
            composed.push_str(&format!("[file: {p}]\n{c}\n\n"));
        }
        at.clear();
    }
    composed.push_str(&text);
    let (system, messages) = {
        let mut ses = sh.session.lock().unwrap();
        ses.push(Role::User, composed);
        (ses.system.clone(), ses.messages.clone())
    };
    let cfg = sh.cfg.lock().unwrap().clone();
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
    let sh2: Arc<Shared> = sh.inner().clone();
    let mcp = sh.mcp.lock().unwrap().clone();
    let handle = tauri::async_runtime::spawn(async move {
        if let Err(e) = agent::run(provider, req, sh2.tx.clone(), sh2.allow_all.clone(), mcp).await {
            let _ = sh2.tx.send(ApiEvent::Failed(format!("{e:#}")));
        }
    });
    *sh.inflight.lock().unwrap() = Some(handle);
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
            let sh = Arc::new(Shared {
                cfg: Mutex::new(cfg),
                provider: Mutex::new(provider),
                session: Mutex::new(Session::new(system_prompt())),
                confirm: Mutex::new(None),
                inflight: Mutex::new(None),
                tokens: Mutex::new((0, 0)),
                attachments: Mutex::new(Vec::new()),
                mcp: Mutex::new(None),
                allow_all: Arc::new(AtomicBool::new(false)),
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
            init, save, send, confirm, allow_all, stop, list_models, mcp_reconnect, undo, redo
        ])
        .run(tauri::generate_context!())
        .map_err(|e| anyhow!("{e}"))?;
    Ok(())
}
