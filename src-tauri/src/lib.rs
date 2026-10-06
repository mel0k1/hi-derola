use anyhow::{anyhow, Result};
use hi_derola::agent;
use hi_derola::chat::{Role, Session};
use hi_derola::config::Config;
use hi_derola::mcp::{self, McpSlot};
use hi_derola::mcpauth;
use hi_derola::provider::{self, ApiEvent, ChatRequest, ConfirmReply, Provider};
use hi_derola::sessions::{self, ChangeRec as SessionChange, SessionMeta, StoredSession};
use hi_derola::todo::Todo;
use hi_derola::{fmt, lsp, models, sandbox, snapshot, tools};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{mpsc, oneshot};

pub struct Shared {
    cfg: Mutex<Config>,
    provider: Mutex<Option<Arc<dyn Provider>>>,
    session: Mutex<Session>,
    sid: Mutex<String>,
    title: Mutex<String>,
    created: Mutex<u64>,
    confirm: Mutex<Option<(oneshot::Sender<ConfirmReply>, String, String)>>,
    ask: Mutex<Option<oneshot::Sender<String>>>,
    inflight: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    tokens: Mutex<(u64, u64)>,
    cost: Mutex<f64>,
    todos: Mutex<Vec<Todo>>,
    attachments: Mutex<Vec<(String, String)>>,
    mcp: McpSlot,
    /// live session id mirrored for mcp tools/call _meta passthrough
    mcp_session: Arc<RwLock<String>>,
    allow_all: Arc<AtomicBool>,
    plan: AtomicBool,
    queue: Arc<Mutex<Vec<String>>>,
    titled: AtomicBool,
    changes: Mutex<Vec<SessionChange>>,
    tx: mpsc::UnboundedSender<ApiEvent>,
}

fn system_prompt(model: &str) -> String {
    let mut s = hi_derola::base_prompt("on the user's machine", model);
    // while a sandbox VM is attached (the sandbox tab attach button), the
    // bash AND file tools run inside it — the model must know that
    if let Some(add) = sandbox::shell_route_addendum() {
        s.push_str("\n\n");
        s.push_str(&add);
    }
    s
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
        todos: sh.todos.lock().unwrap().clone(),
        parent: None,
        changes: sh.changes.lock().unwrap().clone(),
        queue: sh.queue.lock().unwrap().clone(),
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

const PLAN_PROMPT: &str = "PLAN MODE is active: research the codebase (read_file, glob, grep, read-only bash commands) and design an approach. File modifications are disabled. Save the full plan to .hi-derola/plan.md with plan_write (rewrite the whole file on every update), then call plan_exit to ask the user to approve leaving plan mode.";

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
    let mcp = sh.mcp.clone();
    let plan = sh.plan.load(Ordering::Relaxed);
    let mut req = req;
    if plan {
        req.system.push_str("\n\n");
        req.system.push_str(PLAN_PROMPT);
    }
    let agent_cfg = hi_derola::agent::AgentCfg {
        context_limit: cfg.agent.context_limit,
        max_rounds: cfg.agent.max_rounds,
        output_budget: cfg.agent.output_budget,
        perm: cfg.permissions.clone(),
        nested: false,
        plan,
        read_only: false,
        parent_sid: Some(sh.sid.lock().unwrap().clone()),
        depth: 0,
        max_depth: cfg.agent.subagent_depth,
        compaction: cfg.agent.compaction.clone(),
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
                ApiEvent::Tool { name, detail, diff, paths } => {
                    if !paths.is_empty() {
                        let mut adds = 0u64;
                        let mut dels = 0u64;
                        for r in &diff {
                            match r.tag {
                                1 => adds += 1,
                                2 => dels += 1,
                                _ => {}
                            }
                        }
                        if adds > 0 || dels > 0 {
                            let mut ch = sh.changes.lock().unwrap();
                            for p in &paths {
                                if let Some(rec) = ch.iter_mut().find(|r| &r.path == p) {
                                    rec.adds += adds;
                                    rec.dels += dels;
                                } else {
                                    ch.push(SessionChange {
                                        path: p.clone(),
                                        adds,
                                        dels,
                                    });
                                }
                            }
                            persist(&sh);
                        }
                    }
                    json!({
                        "t": "tool", "name": name, "detail": detail, "diff": diff_json(diff),
                        "paths": paths,
                    })
                }
                ApiEvent::Confirm { name, args, rx } => {
                    *sh.confirm.lock().unwrap() = Some((rx, name.clone(), args.clone()));
                    confirm_payload(&name, &args)
                }
                ApiEvent::Ask { name, args, rx } => {
                    *sh.ask.lock().unwrap() = Some(rx);
                    json!({"t": "ask", "name": name, "args": args})
                }
                ApiEvent::Todo(s) => {
                    *sh.todos.lock().unwrap() = hi_derola::todo::get();
                    persist(&sh);
                    json!({"t": "todo", "s": s})
                }
                ApiEvent::BgOut { id, chunk } => json!({"t": "bgout", "id": id, "s": chunk}),
                ApiEvent::Usage { input, output, cached } => {
                    let (model, kind, ctx_limit) = {
                        let cfg = sh.cfg.lock().unwrap();
                        let limit = if cfg.agent.context_limit > 0 {
                            cfg.agent.context_limit
                        } else {
                            let w = models::lookup(&cfg.provider.model).window;
                            if w > 0 { w / 10 * 9 } else { 0 }
                        };
                        (cfg.provider.model.clone(), cfg.provider.kind.clone(), limit)
                    };
                    let disc = if kind == "anthropic" { 0.1 } else { 0.5 };
                    let delta = models::cost_cached(&model, input, output, cached, disc);
                    let mut t = sh.tokens.lock().unwrap();
                    t.0 += input;
                    t.1 += output;
                    drop(t);
                    let mut c = sh.cost.lock().unwrap();
                    *c += delta;
                    let total = *c;
                    drop(c);
                    let ctx_used = input + output;
                    json!({"t": "usage", "input": input, "output": output, "cached": cached, "cost": total,
                           "ctx_used": ctx_used, "ctx_limit": ctx_limit})
                }
                ApiEvent::Done { text, messages } => {
                    *sh.inflight.lock().unwrap() = None;
                    autotitle(&app, &sh, &messages);
                    sh.session.lock().unwrap().messages = messages;
                    snapshot::end_turn();
                    persist(&sh);
                    emit_sessions(&app, &sh);
                    resume = true;
                    json!({"t": "done", "text": text})
                }
                ApiEvent::Plan(on) => {
                    sh.plan.store(on, Ordering::Relaxed);
                    let _ = app.emit("ev", json!({"t": "plan", "on": on}));
                    json!({"t": "plan", "on": on})
                }
                ApiEvent::Failed(e) => {
                    *sh.inflight.lock().unwrap() = None;
                    snapshot::end_turn();
                    resume = true;
                    json!({"t": "failed", "s": e})
                }
                ApiEvent::Wake => {
                    resume = sh.inflight.lock().unwrap().is_none();
                    json!({"t": "wake"})
                }
                ApiEvent::Submit(s) => {
                    dispatch_prompt(&sh, &app, s);
                    json!({"t": "wake"})
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
    *sh.changes.lock().unwrap() = Vec::new();
    *sh.sid.lock().unwrap() = sessions::new_id();
    if let Ok(mut g) = sh.mcp_session.write() {
        *g = sh.sid.lock().unwrap().clone();
    }
    *sh.title.lock().unwrap() = String::new();
    *sh.created.lock().unwrap() = 0;
    *sh.tokens.lock().unwrap() = (0, 0);
    *sh.cost.lock().unwrap() = 0.0;
    *sh.todos.lock().unwrap() = Vec::new();
    sh.queue.lock().unwrap().clear();
    hi_derola::todo::clear();
    let _ = app.emit("ev", json!({"t": "cleared"}));
    emit_sessions(app, sh);
    emit_attachments(sh, app);
}

#[tauri::command]
async fn init(sh: State<'_, Arc<Shared>>) -> Result<Value, String> {
    let cfg = sh.cfg.lock().unwrap().clone();
    Ok(json!({
        "cfg": cfg,
        "keys": cfg.keys(),
        "cwd": std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default(),
        "config_path": hi_derola::config::config_path().display().to_string(),
        "has_provider": sh.provider.lock().unwrap().is_some(),
        "plan": sh.plan.load(Ordering::Relaxed),
        "sessions": sessions::list(),
        "sid": sh.sid.lock().unwrap().clone(),
        "title": sh.title.lock().unwrap().clone(),
        "theme": cfg.ui.theme.clone(),
        "todos": hi_derola::todo::render(&sh.todos.lock().unwrap().clone()),
    }))
}

#[tauri::command]
async fn save(sh: State<'_, Arc<Shared>>, app: AppHandle, cfg: Config) -> Result<Value, String> {
    let p = match cfg.api_key() {
        Some(k) => match provider::build(&cfg.provider.kind, cfg.provider.base_url.clone(), k) {
            Ok(p) => Some(p),
            Err(e) => return Err(format!("{e:#}")),
        },
        None => None,
    };
    cfg.save().map_err(|e| format!("{e:#}"))?;
    lsp::set_enabled(cfg.lsp.enabled);
    fmt::set_enabled(cfg.formatters.enabled);
    hi_derola::tools::set_shell(cfg.agent.shell.clone());
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
async fn set_theme(sh: State<'_, Arc<Shared>>, theme: String) -> Result<Value, String> {
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
async fn confirm(
    sh: State<'_, Arc<Shared>>,
    ok: bool,
    feedback: Option<String>,
    always: Option<bool>,
) -> Result<(), String> {
    if let Some((c, name, args)) = sh.confirm.lock().unwrap().take() {
        let always = always.unwrap_or(false);
        if ok && always {
            if let Some(rule) = hi_derola::perm::derive_rule(&name, &args) {
                if let Err(e) = hi_derola::config::Config::append_perm_rule(rule.clone()) {
                    eprintln!("perm rule not saved: {e:#}");
                }
                sh.cfg.lock().unwrap().permissions.rules.push(rule);
            }
        }
        let _ = c.send(ConfirmReply {
            approved: ok,
            feedback: feedback.unwrap_or_default(),
            always,
        });
    }
    Ok(())
}

#[tauri::command]
async fn answer(sh: State<'_, Arc<Shared>>, text: String) -> Result<(), String> {
    if let Some(a) = sh.ask.lock().unwrap().take() {
        let _ = a.send(text);
    }
    Ok(())
}

#[tauri::command]
async fn allow_all(sh: State<'_, Arc<Shared>>) -> Result<(), String> {
    sh.allow_all.store(true, Ordering::Relaxed);
    if let Some((c, _, _)) = sh.confirm.lock().unwrap().take() {
        let _ = c.send(ConfirmReply { approved: true, feedback: String::new(), always: false });
    }
    Ok(())
}

#[tauri::command]
async fn set_plan(sh: State<'_, Arc<Shared>>, app: AppHandle, on: bool) -> Result<(), String> {
    sh.plan.store(on, Ordering::Relaxed);
    let _ = app.emit("ev", json!({"t": "plan", "on": on}));
    Ok(())
}

#[tauri::command]
async fn task_kill(app: AppHandle, id: String) -> Result<(), String> {
    let msg = hi_derola::bg::kill(id.trim());
    let _ = app.emit("ev", json!({"t": "note", "s": msg}));
    Ok(())
}

#[tauri::command]
async fn stop(sh: State<'_, Arc<Shared>>, app: AppHandle) -> Result<(), String> {
    if let Some(h) = sh.inflight.lock().unwrap().take() {
        h.abort();
    }
    if let Some((c, _, _)) = sh.confirm.lock().unwrap().take() {
        let _ = c.send(ConfirmReply::default());
    }
    if let Some(a) = sh.ask.lock().unwrap().take() {
        let _ = a.send(String::new());
    }
    snapshot::end_turn();
    let _ = app.emit("ev", json!({"t": "note", "s": "cancelled"}));
    let empty = sh.queue.lock().unwrap().is_empty();
    if empty {
        let _ = app.emit("ev", json!({"t": "idle"}));
    } else {
        resume_queue(&app, &sh);
    }
    Ok(())
}

/// client hooks for mcp: workspace root (cwd) + sampling via our provider
fn mcp_hooks(
    cfg: &Config,
    tx: mpsc::UnboundedSender<ApiEvent>,
    session: Arc<RwLock<String>>,
) -> hi_derola::mcp::McpHooks {
    let hooks = hi_derola::mcp::McpHooks::workspace(std::env::current_dir().ok())
        .with_notes(tx.clone())
        .with_session(session)
        .with_eliciter(hi_derola::mcp::default_eliciter(tx.clone()))
        .with_mcp_timeout(cfg.agent.mcp_timeout);
    match cfg.api_key() {
        Some(k) => match provider::build(&cfg.provider.kind, cfg.provider.base_url.clone(), k) {
            Ok(p) => hooks.with_sampler(hi_derola::mcp::default_sampler(
                p,
                cfg.provider.model.clone(),
                cfg.provider.temperature,
                tx,
            )),
            Err(_) => hooks,
        },
        None => hooks,
    }
}

#[tauri::command]
async fn mcp_reconnect(sh: State<'_, Arc<Shared>>) -> Result<Vec<String>, String> {
    let cfg = sh.cfg.lock().unwrap().clone();
    let hooks = mcp_hooks(&cfg, sh.tx.clone(), sh.mcp_session.clone());
    let (client, logs) = mcp::connect_all(&cfg.mcp, &hooks).await;
    *sh.mcp.lock().unwrap() = client;
    Ok(logs)
}

#[tauri::command]
async fn mcp_auth(
    sh: State<'_, Arc<Shared>>,
    name: String,
    code: Option<String>,
) -> Result<Vec<String>, String> {
    let cfg = sh.cfg.lock().unwrap().clone();
    let hooks = mcp_hooks(&cfg, sh.tx.clone(), sh.mcp_session.clone());
    let cfgs = cfg.mcp;
    let mut logs = vec![match code {
        // a pasted authorization code resumes the pending flow
        Some(code) => mcpauth::finish_auth(&name, &cfgs, &code).await,
        None => mcpauth::authorize_flow(&name, &cfgs).await,
    }
    .map_err(|e| format!("{e:#}"))?];
    logs.extend(mcp::reconnect_one(&sh.mcp, &cfgs, &hooks, &name).await);
    Ok(logs)
}

#[tauri::command]
async fn mcp_resources(sh: State<'_, Arc<Shared>>) -> Result<Vec<Value>, String> {
    let Some(c) = sh.mcp.lock().unwrap().clone() else {
        return Ok(vec![]);
    };
    Ok(c.resources()
        .await
        .into_iter()
        .map(|r| {
            json!({"server": r.server, "uri": r.uri, "name": r.name, "description": r.description, "mime": r.mime})
        })
        .collect())
}

#[tauri::command]
async fn mcp_read_resource(sh: State<'_, Arc<Shared>>, server: String, uri: String) -> Result<Value, String> {
    let Some(c) = sh.mcp.lock().unwrap().clone() else {
        return Err("mcp is not configured".into());
    };
    match c.read_resource(&server, &uri).await {
        Ok(text) => Ok(json!({"text": text})),
        Err(e) => Err(format!("{e:#}")),
    }
}

#[tauri::command]
async fn mcp_subscribe(sh: State<'_, Arc<Shared>>, server: String, uri: String) -> Result<(), String> {
    let Some(c) = sh.mcp.lock().unwrap().clone() else {
        return Err("mcp is not configured".into());
    };
    c.subscribe(&server, &uri).await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn mcp_unsubscribe(sh: State<'_, Arc<Shared>>, server: String, uri: String) -> Result<(), String> {
    let Some(c) = sh.mcp.lock().unwrap().clone() else {
        return Err("mcp is not configured".into());
    };
    c.unsubscribe(&server, &uri).await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn mcp_prompts(sh: State<'_, Arc<Shared>>) -> Result<Vec<Value>, String> {
    let Some(c) = sh.mcp.lock().unwrap().clone() else {
        return Ok(vec![]);
    };
    Ok(c.prompts()
        .await
        .into_iter()
        .map(|p| {
            json!({
                "server": p.server, "name": p.name, "description": p.description,
                "arguments": p.arguments.iter().map(|a| json!({
                    "name": a.name, "description": a.description, "required": a.required
                })).collect::<Vec<_>>(),
            })
        })
        .collect())
}

#[tauri::command]
async fn mcp_templates(sh: State<'_, Arc<Shared>>) -> Result<Vec<Value>, String> {
    let Some(c) = sh.mcp.lock().unwrap().clone() else {
        return Ok(vec![]);
    };
    Ok(c.templates()
        .await
        .into_iter()
        .map(|t| {
            json!({"server": t.server, "uri_template": t.uri_template, "name": t.name, "description": t.description, "mime": t.mime})
        })
        .collect())
}

#[tauri::command]
async fn mcp_subscriptions(sh: State<'_, Arc<Shared>>) -> Result<Vec<Value>, String> {
    let Some(c) = sh.mcp.lock().unwrap().clone() else {
        return Ok(vec![]);
    };
    Ok(c.subscriptions()
        .await
        .into_iter()
        .map(|(server, uri)| json!({"server": server, "uri": uri}))
        .collect())
}

#[tauri::command]
async fn mcp_get_prompt(
    sh: State<'_, Arc<Shared>>,
    server: String,
    name: String,
    args: std::collections::HashMap<String, String>,
) -> Result<Vec<Value>, String> {
    let Some(c) = sh.mcp.lock().unwrap().clone() else {
        return Err("mcp is not configured".into());
    };
    let mut a = serde_json::Map::new();
    for (k, v) in args {
        a.insert(k, json!(v));
    }
    match c.get_prompt(&server, &name, &Value::Object(a)).await {
        Ok(msgs) => Ok(msgs
            .into_iter()
            .map(|(role, text)| json!({"role": role, "text": text}))
            .collect()),
        Err(e) => Err(format!("{e:#}")),
    }
}

// ---- sandbox: local QEMU VMs for isolated agent work ----

#[tauri::command]
async fn sandbox_detect() -> Result<sandbox::QemuInfo, String> {
    tokio::task::spawn_blocking(sandbox::detect_qemu)
        .await
        .map_err(|e| format!("{e}"))
}

/// rebuild the live session system prompt after attach/detach so the model
/// immediately sees (or loses) the sandbox addendum — same as the TUI refresh
fn refresh_session_prompt(sh: &Shared) {
    let model = sh.cfg.lock().unwrap().provider.model.clone();
    sh.session.lock().unwrap().system = system_prompt(&model);
}

/// route the bash + file tools of the chat into a running sandbox VM
/// (the gui counterpart of the TUI `/sandbox attach`)
#[tauri::command]
async fn sandbox_attach(sh: State<'_, Arc<Shared>>, id: String) -> Result<String, String> {
    let m = sandbox::SandboxManager::global();
    let st = m
        .list()
        .into_iter()
        .find(|s| s.spec.id == id)
        .ok_or_else(|| format!("no sandbox with id \"{id}\""))?;
    if st.state != sandbox::VmState::Running {
        return Err(format!(
            "sandbox \"{}\" is {} — start it first",
            st.spec.name,
            format!("{:?}", st.state).to_lowercase()
        ));
    }
    let ready = st
        .ssh
        .as_ref()
        .map(|x| x.state == sandbox::SshState::Ready)
        .unwrap_or(false);
    if !ready {
        return Err(
            "ssh is not ready yet — wait for the ready badge on the card, then attach".into(),
        );
    }
    sandbox::set_shell_route(Some(id.clone()));
    refresh_session_prompt(&sh);
    Ok(format!(
        "bash and file tools now run inside \"{}\" — your host files stay out of reach",
        st.spec.name
    ))
}

/// stop routing the chat tools into the VM (back to the host)
#[tauri::command]
async fn sandbox_detach(sh: State<'_, Arc<Shared>>) -> Result<(), String> {
    if sandbox::shell_route().is_none() {
        return Err("no sandbox attached — tools already run on the host".into());
    }
    sandbox::set_shell_route(None);
    refresh_session_prompt(&sh);
    Ok(())
}

#[tauri::command]
async fn sandbox_list() -> Result<Value, String> {
    let m = sandbox::SandboxManager::global();
    Ok(json!({
        "dir": m.dir().display().to_string(),
        "sandboxes": m.list(),
        "attached": sandbox::shell_route(),
    }))
}

#[tauri::command]
async fn sandbox_create(spec: sandbox::NewSandbox) -> Result<sandbox::SandboxStatus, String> {
    let m = sandbox::SandboxManager::global().clone();
    tokio::task::spawn_blocking(move || m.create(&spec))
        .await
        .map_err(|e| format!("{e}"))?
        .map_err(|e| format!("{e:#}"))
}

/// action: start | stop | delete; start/stop return the fresh status,
/// delete returns null (the caller refreshes the list)
#[tauri::command]
async fn sandbox_action(id: String, action: String) -> Result<Value, String> {
    let m = sandbox::SandboxManager::global().clone();
    tokio::task::spawn_blocking(move || match action.as_str() {
        "start" => m
            .start(&id)
            .and_then(|s| Ok(serde_json::to_value(s)?)),
        "stop" => m.stop(&id).and_then(|s| Ok(serde_json::to_value(s)?)),
        "delete" => m.delete(&id).map(|_| Value::Null),
        other => Err(anyhow::anyhow!("unknown sandbox action \"{other}\"")),
    })
    .await
    .map_err(|e| format!("{e}"))?
    .map_err(|e| format!("{e:#}"))
}

/// run one command inside a running sandbox VM over ssh
#[tauri::command]
async fn sandbox_ssh_exec(
    id: String,
    command: String,
    timeout_secs: Option<u64>,
) -> Result<Value, String> {
    let m = sandbox::SandboxManager::global().clone();
    tokio::task::spawn_blocking(move || {
        m.ssh_exec(&id, &command, timeout_secs)
            .map(|o| json!({"code": o.code, "stdout": o.stdout, "stderr": o.stderr}))
    })
    .await
    .map_err(|e| format!("{e}"))?
    .map_err(|e| format!("{e:#}"))
}

/// install (or reinstall) the agent inside the VM: config upload +
/// rustup + cargo install, progress streamed through sandbox_list
#[tauri::command]
async fn sandbox_agent_install(id: String) -> Result<sandbox::SandboxStatus, String> {
    let m = sandbox::SandboxManager::global().clone();
    tokio::task::spawn_blocking(move || m.install_agent(&id))
        .await
        .map_err(|e| format!("{e}"))?
        .map_err(|e| format!("{e:#}"))
}

/// open a new terminal window with an interactive ssh session into the VM
/// (plain shell, or the agent TUI when agent = true)
#[tauri::command]
async fn sandbox_ssh_terminal(id: String, agent: bool) -> Result<(), String> {
    let m = sandbox::SandboxManager::global().clone();
    tokio::task::spawn_blocking(move || m.open_terminal(&id, agent))
        .await
        .map_err(|e| format!("{e}"))?
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn undo(sh: State<'_, Arc<Shared>>) -> Result<Option<String>, String> {
    let _ = &sh;
    Ok(snapshot::undo())
}

#[tauri::command]
async fn redo(sh: State<'_, Arc<Shared>>) -> Result<Option<String>, String> {
    let _ = &sh;
    Ok(snapshot::redo())
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
async fn list_sessions() -> Vec<SessionMeta> {
    sessions::list()
}

#[tauri::command]
async fn new_session(sh: State<'_, Arc<Shared>>, app: AppHandle) -> Result<(), String> {
    start_new(&sh, &app);
    Ok(())
}

#[tauri::command]
async fn open_session(sh: State<'_, Arc<Shared>>, app: AppHandle, id: String) -> Result<Value, String> {
    if *sh.sid.lock().unwrap() != id {
        persist(&sh);
    }
    let st = sessions::load(&id).map_err(|e| format!("{e:#}"))?;
    let model = sh.cfg.lock().unwrap().provider.model.clone();
    *sh.session.lock().unwrap() = Session {
        system: if st.system.trim().is_empty() {
            system_prompt(&model)
        } else {
            st.system.clone()
        },
        messages: st.messages.clone(),
    };
    *sh.sid.lock().unwrap() = st.id.clone();
    if let Ok(mut g) = sh.mcp_session.write() {
        *g = st.id.clone();
    }
    *sh.title.lock().unwrap() = st.title.clone();
    *sh.created.lock().unwrap() = st.created;
    *sh.tokens.lock().unwrap() = (st.tokens_in, st.tokens_out);
    *sh.cost.lock().unwrap() = st.cost;
    *sh.todos.lock().unwrap() = st.todos.clone();
    hi_derola::todo::set_list(st.todos.clone());
    let _ = app.emit(
        "ev",
        json!({"t": "todo", "s": hi_derola::todo::render(&st.todos)}),
    );
    sh.titled.store(true, Ordering::Relaxed);
    sh.attachments.lock().unwrap().clear();
    sh.confirm.lock().unwrap().take();
    sh.ask.lock().unwrap().take();
    *sh.changes.lock().unwrap() = st.changes.clone();
    *sh.queue.lock().unwrap() = st.queue.clone();
    emit_attachments(&sh, &app);
    if !st.queue.is_empty() {
        let _ = sh.tx.send(ApiEvent::Note(format!(
            "restored {} queued message(s) from the previous run, they will steer the next run",
            st.queue.len()
        )));
    }
    Ok(json!({
        "id": st.id,
        "title": st.title,
        "created": st.created,
        "updated": st.updated,
        "tokens_in": st.tokens_in,
        "tokens_out": st.tokens_out,
        "cost": st.cost,
        "transcript": transcript(&st.messages),
        "changes": st.changes,
        "queue": st.queue,
    }))
}
#[tauri::command]
async fn delete_session(sh: State<'_, Arc<Shared>>, app: AppHandle, id: String) -> Result<Value, String> {
    sessions::delete(&id).map_err(|e| format!("{e:#}"))?;
    let current = { *sh.sid.lock().unwrap() == id };
    if current {
        sh.session.lock().unwrap().clear();
        sh.attachments.lock().unwrap().clear();
        sh.allow_all.store(false, Ordering::Relaxed);
        sh.titled.store(false, Ordering::Relaxed);
        *sh.changes.lock().unwrap() = Vec::new();
        *sh.sid.lock().unwrap() = sessions::new_id();
        if let Ok(mut g) = sh.mcp_session.write() {
            *g = sh.sid.lock().unwrap().clone();
        }
        *sh.title.lock().unwrap() = String::new();
        *sh.created.lock().unwrap() = 0;
        *sh.tokens.lock().unwrap() = (0, 0);
        *sh.cost.lock().unwrap() = 0.0;
        *sh.todos.lock().unwrap() = Vec::new();
        sh.queue.lock().unwrap().clear();
        hi_derola::todo::clear();
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
async fn list_agents() -> Value {
    let agents: Vec<Value> = hi_derola::agents::discover()
        .into_iter()
        .map(|a| json!({"name": a.name, "description": a.description, "read_only": a.read_only}))
        .collect();
    json!({"agents": agents})
}

#[tauri::command]
async fn list_dir(path: Option<String>) -> Result<Value, String> {
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
async fn list_project_files() -> Vec<String> {
    let mut out = Vec::new();
    hi_derola::files::walk_files(".", &mut out);
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
async fn attach_path(sh: State<'_, Arc<Shared>>, app: AppHandle, path: String) -> Result<Value, String> {
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
    let content = if hi_derola::files::is_image(&p.display().to_string()) {
        let (mime, data) =
            hi_derola::files::read_image(&p.display().to_string()).map_err(|e| format!("{e:#}"))?;
        format!("data:{mime};base64,{data}")
    } else {
        hi_derola::files::read_attach(&p.display().to_string()).map_err(|e| format!("{e:#}"))?
    };
    let size = content.len();
    sh.attachments.lock().unwrap().push((p.display().to_string(), content));
    emit_attachments(&sh, &app);
    Ok(json!({"ok": true, "kind": "file", "size": size}))
}

#[tauri::command]
async fn detach(sh: State<'_, Arc<Shared>>, app: AppHandle, index: usize) -> Result<(), String> {
    let mut at = sh.attachments.lock().unwrap();
    if index < at.len() {
        at.remove(index);
    }
    drop(at);
    emit_attachments(&sh, &app);
    Ok(())
}

fn note(s: impl Into<String>) -> Value {
    json!({"cmd": true, "note": s.into()})
}

fn command(sh: &Arc<Shared>, app: &AppHandle, line: &str) -> Value {
    let (cmd, arg) = line
        .split_once(' ')
        .map(|(c, a)| (c, a.trim()))
        .unwrap_or((line, ""));
    match cmd {
        "/help" | "/h" => note(
            "commands: /file <path> · /model <name> · /models · /plan · /undo · /redo · /init · /compact · /export [path] · /mcpadd <name> <url|command...> · /mcpconnect <name> · /mcpdisconnect <name> · /mcplogout <name> · /mcpres [server] · /mcpstatus · /mcpread <server> <uri> · /mcpsub <server> <uri> · /mcpunsub <server> <uri> · /mcpprompt [server] <name> [k=v] · /mcplog [server] (/mcplog set <server|all> <level>) · /jstools [reload] · /clear · /help\n\
             mutations (write/edit/bash/mcp) ask for confirmation, allow all skips further asks\n\
             custom commands: .hi-derola/commands/<name>.md or ~/.config/hi-derola/commands/<name>.md ($ARGUMENTS, $1..$9)",
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
        "/mcpadd" => {
            let cfg = match hi_derola::mcp::parse_add(arg) {
                Ok(c) => c,
                Err(e) => return note(format!("error: {e:#}")),
            };
            let name = cfg.name.clone();
            let saved = {
                let mut c = sh.cfg.lock().unwrap();
                c.mcp.retain(|x| x.name != name);
                c.mcp.push(cfg.clone());
                match c.save() {
                    Ok(_) => "saved to config".to_string(),
                    Err(e) => format!("not saved: {e:#}"),
                }
            };
            let hooks = {
                let c = sh.cfg.lock().unwrap().clone();
                mcp_hooks(&c, sh.tx.clone(), sh.mcp_session.clone())
            };
            let slot = sh.mcp.clone();
            let tx = sh.tx.clone();
            tauri::async_runtime::spawn(async move {
                let existing = slot.lock().unwrap().clone();
                let msg = match existing {
                    Some(m) => match m.add(&cfg, &hooks).await {
                        Ok(sum) => format!("mcp {name}: connected ({sum})"),
                        Err(e) => format!("error: {e:#}"),
                    },
                    None => {
                        let (client, mut logs) = hi_derola::mcp::connect_all(std::slice::from_ref(&cfg), &hooks).await;
                        *slot.lock().unwrap() = client;
                        logs.pop().unwrap_or_else(|| format!("mcp {name}: connected"))
                    }
                };
                let _ = tx.send(ApiEvent::Note(msg));
            });
            note(format!("adding mcp {name} ({saved})..."))
        }
        "/mcpconnect" => {
            if arg.is_empty() {
                return note("usage: /mcpconnect <name> — (re)connect a configured server");
            }
            let name = arg.trim().to_string();
            let cfgs = sh.cfg.lock().unwrap().mcp.clone();
            let hooks = {
                let c = sh.cfg.lock().unwrap().clone();
                mcp_hooks(&c, sh.tx.clone(), sh.mcp_session.clone())
            };
            let slot = sh.mcp.clone();
            let tx = sh.tx.clone();
            tauri::async_runtime::spawn(async move {
                for l in mcp::reconnect_one(&slot, &cfgs, &hooks, &name).await {
                    let _ = tx.send(ApiEvent::Note(l));
                }
            });
            note(format!("connecting mcp {name}..."))
        }
        "/mcpdisconnect" => {
            if arg.is_empty() {
                return note("usage: /mcpdisconnect <name> — drop the live connection (config untouched), /mcpconnect brings it back");
            }
            let name = arg.trim().to_string();
            let m = sh.mcp.lock().unwrap().clone();
            let Some(m) = m else {
                return note("mcp is not configured");
            };
            let tx = sh.tx.clone();
            tauri::async_runtime::spawn(async move {
                let msg = m.disconnect(&name).await.unwrap_or_else(|e| format!("error: {e:#}"));
                let _ = tx.send(ApiEvent::Note(msg));
            });
            note(format!("disconnecting mcp {name}..."))
        }
        "/mcplogout" => {
            if arg.is_empty() {
                return note("usage: /mcplogout <name> — drop the stored oauth tokens; the next use starts a fresh auth flow");
            }
            let name = arg.trim().to_string();
            let msg = if mcpauth::logout(&name) {
                format!("mcp {name}: signed out (tokens cleared, a fresh auth flow starts on next use)")
            } else {
                format!("mcp {name}: no stored credentials")
            };
            note(msg)
        }
        "/mcpstatus" => {
            let cfgs = sh.cfg.lock().unwrap().mcp.clone();
            let mcp = sh.mcp.clone();
            let tx = sh.tx.clone();
            tauri::async_runtime::spawn(async move {
                let list = mcp::status(&mcp, &cfgs).await;
                if list.is_empty() {
                    let _ = tx.send(ApiEvent::Note(
                        "no mcp servers configured ([[mcp]] in config.toml)".into(),
                    ));
                    return;
                }
                let mut out = format!("mcp servers ({}):", list.len());
                for e in list {
                    out.push_str(&format!("\n  [{}] {}: {}", e.state, e.name, e.detail));
                }
                let _ = tx.send(ApiEvent::Note(out));
            });
            note("checking mcp servers...")
        }
        "/mcpres" => {
            let mcp = sh.mcp.lock().unwrap().clone();
            let filter = arg.trim().to_string();
            let tx = sh.tx.clone();
            tauri::async_runtime::spawn(async move {
                let Some(c) = mcp else {
                    let _ = tx.send(ApiEvent::Note("mcp is not configured".into()));
                    return;
                };
                let subs: std::collections::HashSet<(String, String)> =
                    c.subscriptions().await.into_iter().collect();
                let list: Vec<_> = c
                    .resources()
                    .await
                    .into_iter()
                    .filter(|r| filter.is_empty() || r.server == filter)
                    .collect();
                let tpls: Vec<_> = c
                    .templates()
                    .await
                    .into_iter()
                    .filter(|t| filter.is_empty() || t.server == filter)
                    .collect();
                let msg = if list.is_empty() && tpls.is_empty() {
                    format!(
                        "no mcp resources{}",
                        if filter.is_empty() { String::new() } else { format!(" on {filter}") }
                    )
                } else {
                    let mut out = String::new();
                    if !list.is_empty() {
                        out.push_str(&format!("mcp resources ({}):", list.len()));
                        for r in list {
                            out.push_str(&format!("\n  {}  {}", r.server, r.uri));
                            if subs.contains(&(r.server.clone(), r.uri.clone())) {
                                out.push_str(" [subscribed]");
                            }
                            if !r.name.is_empty() && r.name != r.uri {
                                out.push_str(&format!(" ({})", r.name));
                            }
                            if !r.description.is_empty() {
                                out.push_str(&format!(" — {}", r.description));
                            }
                        }
                    }
                    if !tpls.is_empty() {
                        if !out.is_empty() {
                            out.push_str("\n\n");
                        }
                        out.push_str(&format!("mcp templates ({}):", tpls.len()));
                        for t in tpls {
                            out.push_str(&format!("\n  {}  {}", t.server, t.uri_template));
                            if !t.name.is_empty() {
                                out.push_str(&format!(" ({})", t.name));
                            }
                            if !t.description.is_empty() {
                                out.push_str(&format!(" — {}", t.description));
                            }
                        }
                        out.push_str("\n\nfill the braces with real values and read via /mcpread <server> <uri>");
                    }
                    out
                };
                let _ = tx.send(ApiEvent::Note(msg));
            });
            note("listing mcp resources...")
        }
        "/mcpread" => {
            let Some((server, uri)) = arg.trim().split_once(char::is_whitespace) else {
                return note("usage: /mcpread <server> <uri> — run /mcpres to list resources");
            };
            let server = server.trim().to_string();
            let uri = uri.trim().to_string();
            let mcp = sh.mcp.lock().unwrap().clone();
            let tx = sh.tx.clone();
            tauri::async_runtime::spawn(async move {
                let Some(c) = mcp else {
                    let _ = tx.send(ApiEvent::Note("mcp is not configured".into()));
                    return;
                };
                let msg = match c.read_resource(&server, &uri).await {
                    Ok(text) => hi_derola::provider::truncate(&text).trim().to_string(),
                    Err(e) => format!("error: {e:#}"),
                };
                let _ = tx.send(ApiEvent::Note(msg));
            });
            note(format!("reading {server} {uri}..."))
        }
        "/mcpprompt" => {
            let parts: Vec<String> = arg.split_whitespace().map(|s| s.to_string()).collect();
            if parts.is_empty() {
                let mcp = sh.mcp.lock().unwrap().clone();
                let tx = sh.tx.clone();
                tauri::async_runtime::spawn(async move {
                    let Some(c) = mcp else {
                        let _ = tx.send(ApiEvent::Note("mcp is not configured".into()));
                        return;
                    };
                    let list = c.prompts().await;
                    if list.is_empty() {
                        let _ = tx.send(ApiEvent::Note("no mcp prompts".into()));
                        return;
                    }
                    let mut out = format!("mcp prompts ({}):", list.len());
                    for p in list {
                        out.push_str(&format!("\n  {}  {}", p.server, p.name));
                        if !p.description.is_empty() {
                            out.push_str(&format!(" — {}", p.description));
                        }
                        if !p.arguments.is_empty() {
                            let names: Vec<String> = p
                                .arguments
                                .iter()
                                .map(|a| if a.required { format!("{}*", a.name) } else { a.name.clone() })
                                .collect();
                            out.push_str(&format!(" (args: {})", names.join(", ")));
                        }
                    }
                    out.push_str("\n\nusage: /mcpprompt <server> <name> [key=value ...]");
                    let _ = tx.send(ApiEvent::Note(out));
                });
                note("listing mcp prompts...")
            } else if parts.len() < 2 {
                note("usage: /mcpprompt <server> <name> [key=value ...]")
            } else {
                let mcp = sh.mcp.lock().unwrap().clone();
                let tx = sh.tx.clone();
                let sh2 = sh.clone();
                let app2 = app.clone();
                let server = parts[0].clone();
                let name = parts[1].clone();
                let mut a = serde_json::Map::new();
                for p in &parts[2..] {
                    if let Some((k, v)) = p.split_once('=') {
                        a.insert(k.to_string(), json!(v));
                    }
                }
                tauri::async_runtime::spawn(async move {
                    let Some(c) = mcp else {
                        let _ = tx.send(ApiEvent::Note("mcp is not configured".into()));
                        return;
                    };
                    match c.get_prompt(&server, &name, &Value::Object(a)).await {
                        Ok(msgs) if msgs.is_empty() => {
                            let _ = tx.send(ApiEvent::Note("prompt returned no messages".into()));
                        }
                        Ok(msgs) => {
                            let mut text = String::new();
                            for (role, t) in &msgs {
                                if role != "user" {
                                    text.push_str(&format!("[{role}]\n"));
                                }
                                text.push_str(t);
                                text.push_str("\n\n");
                            }
                            dispatch_prompt(&sh2, &app2, text.trim().to_string());
                        }
                        Err(e) => {
                            let _ = tx.send(ApiEvent::Note(format!("error: {e:#}")));
                        }
                    }
                });
                note(format!("fetching prompt {server}/{name}..."))
            }
        }
        "/file" => {
            if arg.is_empty() {
                note("usage: /file <path>")
            } else if hi_derola::files::is_image(arg) {
                // images ride as data urls, same as the file-dialog attach
                match hi_derola::files::read_image(arg) {
                    Ok((mime, data)) => {
                        let content = format!("data:{mime};base64,{data}");
                        let size = content.len();
                        sh.attachments.lock().unwrap().push((arg.to_string(), content));
                        emit_attachments(sh, app);
                        note(format!("attached {arg} ({mime} image, {size} bytes)"))
                    }
                    Err(e) => note(format!("error: {e:#}")),
                }
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
        "/mcplog" => {
            let parts: Vec<&str> = arg.split_whitespace().collect();
            if parts.first().copied() == Some("set") {
                if parts.len() < 3 {
                    return note("usage: /mcplog set <server|all> <level> — levels: debug info notice warning error critical alert emergency");
                }
                let server = if parts[1] == "all" {
                    String::new()
                } else {
                    parts[1].to_string()
                };
                let level = parts[2].to_string();
                let mcp = sh.mcp.lock().unwrap().clone();
                let tx = sh.tx.clone();
                tauri::async_runtime::spawn(async move {
                    let Some(c) = mcp else {
                        let _ = tx.send(ApiEvent::Note("mcp is not configured".into()));
                        return;
                    };
                    for l in c.set_log_level(&server, &level).await {
                        let _ = tx.send(ApiEvent::Note(l));
                    }
                });
                note(format!("setting mcp log level {level}..."))
            } else {
                // logs() is a sync snapshot of the ring buffer, no spawn needed
                let filter = parts.first().copied().unwrap_or("");
                let Some(c) = sh.mcp.lock().unwrap().clone() else {
                    return note("mcp is not configured");
                };
                let list: Vec<mcp::McpLogEntry> = c
                    .logs()
                    .into_iter()
                    .filter(|e| filter.is_empty() || e.server == filter)
                    .collect();
                if list.is_empty() {
                    note("no mcp log messages yet — warning and above pop into the chat")
                } else {
                    let tail = &list[list.len().saturating_sub(20)..];
                    let mut out = format!("mcp logs (last {}):", tail.len());
                    for e in tail {
                        let who = if e.logger.is_empty() {
                            e.server.clone()
                        } else {
                            format!("{} {}", e.server, e.logger)
                        };
                        out.push_str(&format!("\n  [{}] {}: {}", e.level, who, e.data));
                    }
                    note(out)
                }
            }
        }
        "/jstools" => {
            if arg.trim() == "reload" {
                hi_derola::jstools::reload();
                note(format!(
                    "JS tools rescanned:\n{}",
                    hi_derola::jstools::summary()
                ))
            } else {
                note(format!("JS tools:\n{}", hi_derola::jstools::summary()))
            }
        }
        "/undo" | "/u" => note(snapshot::undo().unwrap_or_else(|| "nothing to undo".into())),
        "/redo" => note(snapshot::redo().unwrap_or_else(|| "nothing to redo".into())),
        "/plan" => {
            let on = !sh.plan.load(Ordering::Relaxed);
            sh.plan.store(on, Ordering::Relaxed);
            let _ = app.emit("ev", json!({"t": "plan", "on": on}));
            note(if on {
                "plan mode on: read-only research, the agent will propose a plan instead of making changes"
            } else {
                "plan mode off"
            })
        }
        "/init" => {
            if sh.inflight.lock().unwrap().is_some() {
                return note("wait for the current run to finish");
            }
            let cwd = std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            dispatch_prompt(sh, app, hi_derola::commands::init_prompt(&cwd))
        }
        "/compact" => {
            if sh.inflight.lock().unwrap().is_some() {
                return note("wait for the current run to finish");
            }
            let msgs_now = sh.session.lock().unwrap().messages.clone();
            if msgs_now.len() < 6 {
                return note("nothing to compact yet");
            }
            let cfg = sh.cfg.lock().unwrap().clone();
            let Some(provider) = sh.provider.lock().unwrap().clone() else {
                return note("no provider configured");
            };
            let (system, _) = {
                let ses = sh.session.lock().unwrap();
                (ses.system.clone(), ses.messages.clone())
            };
            let req = ChatRequest {
                system,
                messages: msgs_now,
                model: cfg.provider.model.clone(),
                max_tokens: cfg.provider.max_tokens,
                temperature: cfg.provider.temperature,
                top_p: cfg.provider.top_p,
                stream: false,
                tools: Vec::new(),
            };
            let tx = sh.tx.clone();
            let keep = cfg.agent.compaction.keep;
            tauri::async_runtime::spawn(async move {
                let mut msgs = req.messages.clone();
                if agent::compact_session(provider, &req, &mut msgs, &tx, keep).await {
                    let _ = tx.send(ApiEvent::Done {
                        text: "context compacted".into(),
                        messages: msgs,
                    });
                } else {
                    let _ = tx.send(ApiEvent::Note("compaction failed".into()));
                }
            });
            note("compacting context...")
        }
        "/export" => {
            let path = if arg.is_empty() {
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                format!("hiderola-session-{ts}.md")
            } else {
                arg.to_string()
            };
            let (title, msgs) = {
                let title = sh.title.lock().unwrap().clone();
                let msgs = sh.session.lock().unwrap().messages.clone();
                (title, msgs)
            };
            let md = hi_derola::commands::export_markdown(&title, &msgs);
            match std::fs::write(&path, md) {
                Ok(_) => note(format!("exported to {path}")),
                Err(e) => note(format!("error: {e:#}")),
            }
        }
        _ => match hi_derola::commands::get(cmd) {
            Some(c) => {
                if sh.inflight.lock().unwrap().is_some() {
                    return note("wait for the current run to finish");
                }
                let text = hi_derola::commands::render(&c.template, arg);
                dispatch_prompt(sh, app, text)
            }
            None => note(format!("unknown command: {cmd}, try /help")),
        },
    }
}

fn dispatch_prompt(sh: &Arc<Shared>, app: &AppHandle, text: String) -> Value {
    if sh.inflight.lock().unwrap().is_some() {
        sh.queue.lock().unwrap().push(text);
        // the queue is durable: it lives in the session file, so a crash
        // cannot lose messages typed while the agent was busy
        persist(sh);
        let _ = sh.tx.send(ApiEvent::Note("queued: will run after the current task".into()));
        return json!({"cmd": false, "queued": true});
    }
    {
        let mut ses = sh.session.lock().unwrap();
        ses.push(Role::User, text);
    }
    persist(sh);
    emit_sessions(app, sh);
    match launch(sh) {
        Ok(_) => json!({"cmd": false}),
        Err(e) => note(format!("error: {e}")),
    }
}

#[tauri::command]
async fn send(sh: State<'_, Arc<Shared>>, app: AppHandle, text: String) -> Result<Value, String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Ok(json!({"cmd": true, "note": ""}));
    }
    if text.starts_with('/') {
        return Ok(command(&sh, &app, &text));
    }
    // manual subagent invocation: "@explore find the parser"
    if let Some((agent_name, rest)) = hi_derola::agents::split_mention(&text) {
        let Some(provider) = sh.provider.lock().unwrap().clone() else {
            return Err("no api key: open settings and add one".into());
        };
        let cfg = sh.cfg.lock().unwrap().clone();
        let (blocks, _ok, _miss) = hi_derola::files::mentions(&rest);
        let prompt = format!("{blocks}{rest}");
        let parent_sid = sh.sid.lock().unwrap().clone();
        let (sub_req, sid, read_only) = match agent::resolve_sub_req(
            Some(&agent_name),
            &prompt,
            None,
            Some(&parent_sid),
            &cfg.provider.model,
            cfg.provider.max_tokens,
            cfg.provider.temperature,
            cfg.provider.top_p,
        ) {
            Ok(r) => r,
            Err(e) => return Err(e),
        };
        let id = agent::spawn_standalone_subagent(
            provider,
            sub_req,
            sid,
            String::new(),
            read_only,
            hi_derola::agent::AgentCfg {
                context_limit: cfg.agent.context_limit,
                max_rounds: cfg.agent.max_rounds,
                output_budget: cfg.agent.output_budget,
                perm: cfg.permissions.clone(),
                nested: false,
                plan: false,
                read_only: false,
                parent_sid: Some(parent_sid),
                depth: 0,
                max_depth: cfg.agent.subagent_depth,
                compaction: cfg.agent.compaction.clone(),
            },
            sh.allow_all.clone(),
            sh.mcp.clone(),
            sh.queue.clone(),
            sh.tx.clone(),
        );
        return Ok(json!({"cmd": false, "subagent": id}));
    }
    let mut composed = String::new();
    let mut images: Vec<hi_derola::chat::Image> = Vec::new();
    {
        let mut at = sh.attachments.lock().unwrap();
        for (p, c) in at.iter() {
            if let Some((mime, data)) = hi_derola::files::split_data_url(c) {
                images.push(hi_derola::chat::Image { mime, data });
                composed.push_str(&format!("[image: {p}]\n\n"));
            } else {
                composed.push_str(&format!("[file: {p}]\n{c}\n\n"));
            }
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
        persist(&sh);
        let _ = sh.tx.send(ApiEvent::Note("queued: will steer the current run".into()));
        return Ok(json!({"cmd": false, "queued": true}));
    }
    {
        let mut ses = sh.session.lock().unwrap();
        ses.messages
            .push(hi_derola::chat::Message::new(Role::User, composed).with_images(images));
    }
    persist(&sh);
    emit_sessions(&app, &sh);
    emit_attachments(&sh, &app);
    launch(&sh)?;
    Ok(json!({"cmd": false}))
}

pub fn run() -> Result<()> {
    let (cfg, _) = Config::load_or_default()?;
    lsp::set_enabled(cfg.lsp.enabled);
    fmt::set_enabled(cfg.formatters.enabled);
    hi_derola::tools::set_shell(cfg.agent.shell.clone());
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
                            system_prompt(&cfg.provider.model)
                        } else {
                            st.system.clone()
                        },
                        messages: st.messages.clone(),
                    },
                ),
                None => (
                    sessions::new_id(),
                    String::new(),
                    0,
                    Session::new(system_prompt(&cfg.provider.model)),
                ),
            };
            let sh = Arc::new(Shared {
                cfg: Mutex::new(cfg),
                provider: Mutex::new(provider),
                session: Mutex::new(session),
                sid: Mutex::new(sid.clone()),
                mcp_session: Arc::new(RwLock::new(sid)),
                title: Mutex::new(title),
                created: Mutex::new(created),
                confirm: Mutex::new(None),
                ask: Mutex::new(None),
                inflight: Mutex::new(None),
                tokens: Mutex::new(restore.as_ref().map(|s| (s.tokens_in, s.tokens_out)).unwrap_or((0, 0))),
                cost: Mutex::new(restore.as_ref().map(|s| s.cost).unwrap_or(0.0)),
                todos: Mutex::new(restore.as_ref().map(|s| s.todos.clone()).unwrap_or_default()),
                attachments: Mutex::new(Vec::new()),
                mcp: Arc::new(Mutex::new(None)),
                allow_all: Arc::new(AtomicBool::new(false)),
                plan: AtomicBool::new(false),
                queue: Arc::new(Mutex::new(Vec::new())),
                titled: AtomicBool::new(restore.is_some()),
                changes: Mutex::new(restore.as_ref().map(|s| s.changes.clone()).unwrap_or_default()),
                tx: tx.clone(),
            });
            let tx2 = tx.clone();
            let restored_todos = restore.as_ref().map(|s| s.todos.clone()).unwrap_or_default();
            if !restored_todos.is_empty() {
                hi_derola::todo::set_list(restored_todos);
            }
            let mcp_slot = sh.mcp.clone();
            let mcp_cfg = sh.cfg.lock().unwrap().clone();
            let mcp_hooks = mcp_hooks(&mcp_cfg, tx.clone(), sh.mcp_session.clone());
            let mcp_cfgs = mcp_cfg.mcp;
            tauri::async_runtime::spawn(async move {
                let (client, logs) = mcp::connect_all(&mcp_cfgs, &mcp_hooks).await;
                *mcp_slot.lock().unwrap() = client;
                for l in logs {
                    let _ = tx2.send(ApiEvent::Note(l));
                }
            });
            pump(rx, app.handle().clone(), sh.clone());
            app.manage(sh);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            init, save, send, confirm, answer, allow_all, stop, list_models, mcp_reconnect,
            mcp_auth, mcp_resources, mcp_read_resource, mcp_subscribe, mcp_unsubscribe, mcp_prompts,
            mcp_templates, mcp_subscriptions, mcp_get_prompt, sandbox_detect, sandbox_list,
            sandbox_create, sandbox_action, sandbox_attach, sandbox_detach, sandbox_ssh_exec,
            sandbox_agent_install, sandbox_ssh_terminal, undo,
            redo, list_sessions, new_session, open_session, delete_session, list_dir, attach_path,
            detach, set_theme, list_project_files, set_plan, task_kill, list_agents
        ])
        .run(tauri::generate_context!())
        .map_err(|e| anyhow!("{e}"))?;
    Ok(())
}
