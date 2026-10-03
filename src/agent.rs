use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

use crate::chat::{Message, Role};
use crate::config::CompactionCfg;
use crate::mcp::McpSlot;
use crate::provider::{ApiEvent, ChatRequest, Provider};
use crate::tools;

#[derive(Clone)]
pub struct AgentCfg {
    pub context_limit: u64,
    pub max_rounds: usize,
    pub output_budget: usize,
    pub perm: crate::perm::PermCfg,
    pub nested: bool,
    pub plan: bool,
    pub read_only: bool,
    pub parent_sid: Option<String>,
    pub depth: usize,
    pub max_depth: usize,
    pub compaction: CompactionCfg,
}

impl Default for AgentCfg {
    fn default() -> Self {
        Self {
            context_limit: 0,
            max_rounds: 15,
            output_budget: 32 * 1024,
            perm: Default::default(),
            nested: false,
            plan: false,
            read_only: false,
            parent_sid: None,
            depth: 0,
            max_depth: 1,
            compaction: Default::default(),
        }
    }
}

const COMPACT_KEEP_TOKENS: usize = 15_000;
const COMPACT_MIN_MSGS: usize = 8;
const COMPACT_TOOL_CLIP: usize = 1_250;
/// separator between the original first user message and the compaction summary
const SUMMARY_MARKER: &str =
    "\n\n---\nSummary of the earlier conversation (dropped to fit the context window):\n";
const IMG_TOKEN_EST: usize = 1_500;
const OUTPUT_FLOOR: u64 = 1_024;
const EMPTY_RETRIES: usize = 2;
const OVERFLOW_RETRIES: usize = 2;
const MAX_CONTINUES: usize = 3;
const CONTINUE_PROMPT: &str = "The previous response was interrupted by the output token limit. Continue from where you left off without repeating completed content.";

fn effective_limit(cfg: &AgentCfg, model: &str) -> u64 {
    if cfg.context_limit > 0 {
        return cfg.context_limit;
    }
    let mi = crate::models::lookup(model);
    if mi.window > 0 {
        mi.window / 10 * 9
    } else {
        0
    }
}

/// headroom kept from the context window before proactive compaction:
/// the configured absolute buffer, or a quarter of the window (legacy default)
fn compact_headroom(cfg: &CompactionCfg, ctx_limit: u64) -> u64 {
    if cfg.buffer > 0 {
        cfg.buffer as u64
    } else {
        ctx_limit / 4
    }
}

/// proactive compaction trigger; overflow recovery and manual /compact
/// are gated separately and stay available when this returns false
fn auto_compact_due(cfg: &CompactionCfg, ctx_limit: u64, used: u64, est: u64) -> bool {
    if !cfg.auto || ctx_limit == 0 {
        return false;
    }
    used.max(est) > ctx_limit.saturating_sub(compact_headroom(cfg, ctx_limit))
}

/// keep budget for compact_split: configured value, or the built-in default
fn effective_keep(keep: usize) -> usize {
    if keep == 0 {
        COMPACT_KEEP_TOKENS
    } else {
        keep
    }
}

/// shrink max_tokens so the reply fits in the remaining context window;
/// never grows a user-configured cap, provider defaults stay intact when
/// the context limit is unknown
fn clamp_output(configured: Option<u32>, ctx_limit: u64, prompt_tokens: u64) -> Option<u32> {
    if ctx_limit == 0 {
        return configured;
    }
    let headroom = prompt_tokens + prompt_tokens * 3 / 20;
    let room = ctx_limit.saturating_sub(headroom).max(OUTPUT_FLOOR);
    let base = configured.map(|v| v as u64).unwrap_or(4_096);
    if base <= room {
        return configured;
    }
    Some(room.min(u32::MAX as u64) as u32)
}

pub async fn run(
    provider: Arc<dyn Provider>,
    mut req: ChatRequest,
    tx: UnboundedSender<ApiEvent>,
    allow_all: Arc<AtomicBool>,
    mcp: McpSlot,
    queue: Arc<Mutex<Vec<String>>>,
    mut cfg: AgentCfg,
) -> Result<()> {
    let ctx_limit = effective_limit(&cfg, &req.model);
    let configured_max = req.max_tokens;
    let mut msgs = req.messages.clone();
    let mut round = 0;
    let mut empty_retries = 0usize;
    let mut overflow_retries = 0usize;
    let mut continues = 0usize;
    let mut used: u64 = 0;
    loop {
        round += 1;
        let mcp_now = mcp.lock().unwrap().clone();
        let mut specs = if cfg.nested {
            tools::specs_nested(cfg.depth < cfg.max_depth)
        } else {
            tools::specs()
        };
        if cfg.plan {
            specs.retain(|s| {
                s.name != "write_file" && s.name != "edit" && s.name != "apply_patch"
            });
        }
        if let Some(m) = &mcp_now {
            specs.extend(m.specs().await);
        }
        if cfg.read_only {
            specs.retain(|s| {
                matches!(
                    s.name.as_str(),
                    "read_file" | "list_files" | "glob" | "grep" | "task_status" | "todoread" | "skill"
                )
            });
        }
        req.tools = specs;
        if round > cfg.max_rounds {
            return wrap_up(provider, &mut req, &mut msgs, &tx, cfg.max_rounds).await;
        }
        let est = est_tokens(&msgs) as u64;
        if auto_compact_due(&cfg.compaction, ctx_limit, used, est) {
            if compact(&provider, &req, &mut msgs, &tx, cfg.compaction.keep).await {
                used = 0;
                req.messages = msgs.clone();
            }
        }
        let mut steered = false;
        for text in queue.lock().unwrap().drain(..) {
            let head: String = text.chars().take(60).collect();
            let _ = tx.send(ApiEvent::Note(format!("steer: {head}")));
            msgs.push(Message::new(Role::User, text));
            steered = true;
        }
        if steered {
            req.messages = msgs.clone();
        }
        req.max_tokens = clamp_output(configured_max, ctx_limit, used.max(est));
        let (itx, mut irx) = tokio::sync::mpsc::unbounded_channel();
        let counter = Arc::new(AtomicU64::new(0));
        let outer = tx.clone();
        let c2 = counter.clone();
        let fwd = tokio::spawn(async move {
            while let Some(ev) = irx.recv().await {
                if let ApiEvent::Usage { input, .. } = ev {
                    c2.fetch_max(input, Ordering::Relaxed);
                }
                let _ = outer.send(ev);
            }
        });
        let outcome = provider.chat(&req, &itx).await;
        drop(itx);
        let _ = fwd.await;
        let reply = match outcome {
            Ok(r) => r,
            Err(e) => {
                if overflow_retries < OVERFLOW_RETRIES && is_overflow(&e) {
                    overflow_retries += 1;
                    let _ = tx.send(ApiEvent::Note(format!(
                        "context overflow, compacting and retrying ({overflow_retries}/{OVERFLOW_RETRIES})"
                    )));
                    if compact(&provider, &req, &mut msgs, &tx, cfg.compaction.keep).await {
                        req.messages = msgs.clone();
                        round -= 1;
                        continue;
                    }
                }
                return Err(e);
            }
        };
        overflow_retries = 0;
        let observed = counter.load(Ordering::Relaxed);
        used = if observed == 0 { est } else { observed };
        if reply.text.trim().is_empty() && reply.calls.is_empty() {
            empty_retries += 1;
            if empty_retries <= EMPTY_RETRIES {
                let _ = tx.send(ApiEvent::Note(format!(
                    "empty response, retrying ({empty_retries}/{EMPTY_RETRIES})"
                )));
                round -= 1;
                continue;
            }
        }
        if reply.truncated && continues < MAX_CONTINUES {
            continues += 1;
            let _ = tx.send(ApiEvent::Note(format!(
                "output was cut off by the token limit, continuing ({continues}/{MAX_CONTINUES})"
            )));
            // truncated tool calls are dropped: their args would be broken JSON
            // and an unanswered tool_use would corrupt the transcript
            msgs.push(Message::new(Role::Assistant, reply.text.clone()));
            msgs.push(Message::new(Role::User, CONTINUE_PROMPT));
            req.messages = msgs.clone();
            continue;
        }
        if reply.calls.is_empty() {
            msgs.push(Message::new(Role::Assistant, reply.text.clone()));
            let _ = tx.send(ApiEvent::Done {
                text: reply.text,
                messages: msgs,
            });
            return Ok(());
        }
        empty_retries = 0;
        msgs.push(Message::new(Role::Assistant, reply.text.clone()).with_calls(reply.calls.clone()));
        req.messages = msgs.clone();
        let mut pending_images: Vec<(String, crate::chat::Image)> = Vec::new();
        for call in reply.calls {
            tx.send(ApiEvent::Tool {
                name: call.name.clone(),
                detail: tools::detail(&call.name, &call.args),
                diff: tools::preview(&call.name, &call.args),
                paths: tools::paths(&call.name, &call.args),
            })
            .map_err(|_| anyhow!("closed"))?;
            let plan_block = cfg.plan
                && matches!(
                    call.name.as_str(),
                    "write_file" | "edit" | "apply_patch"
                );
            let perm = if plan_block {
                crate::perm::Perm::Deny
            } else {
                cfg.perm.check(&call.name, &call.args)
            };
            match perm {
                crate::perm::Perm::Deny => {
                    let why = if plan_block {
                        "plan mode is active: file modifications are disabled. Research the codebase and present a plan instead."
                    } else {
                        "denied by permissions config"
                    };
                    let _ = tx.send(ApiEvent::Note(if plan_block {
                        format!("{} denied: plan mode", call.name)
                    } else {
                        format!("{} denied by permissions config", call.name)
                    }));
                    msgs.push(Message::tool(&call.id, why));
                    req.messages = msgs.clone();
                    continue;
                }
                crate::perm::Perm::Allow => {}
                crate::perm::Perm::Ask => {
                    if !allow_all.load(Ordering::Relaxed) {
                        let (otx, orx) = oneshot::channel();
                        tx.send(ApiEvent::Confirm {
                            name: call.name.clone(),
                            args: call.args.clone(),
                            rx: otx,
                        })
                        .map_err(|_| anyhow!("closed"))?;
                        let reply = orx.await.unwrap_or_default();
                        if !reply.approved {
                            msgs.push(Message::tool(&call.id, deny_message(&call.name, &reply.feedback)));
                            req.messages = msgs.clone();
                            continue;
                        }
                        if reply.always {
                            if let Some(rule) = crate::perm::derive_rule(&call.name, &call.args) {
                                cfg.perm.rules.push(rule);
                            }
                        }
                    }
                }
            }
            let out = if call.name == "question" {
                match ask_user(&call.args, &tx).await {
                    Ok(a) => a,
                    Err(e) => format!("error: {e:#}"),
                }
            } else if call.name == "todowrite" {
                match crate::todo::write_from_args(&call.args) {
                    Ok(rendered) => {
                        let _ = tx.send(ApiEvent::Todo(rendered.clone()));
                        format!("Todo list updated:\n{rendered}")
                    }
                    Err(e) => format!("error: {e:#}"),
                }
            } else if call.name == "bash" {
                let v: Value = serde_json::from_str(&call.args).unwrap_or(Value::Null);
                if v["background"].as_bool().unwrap_or(false) {
                    run_bash_background(&v, &tx, queue.clone(), cfg.output_budget)
                } else {
                    match tools::execute("bash", &call.args, mcp_now.as_deref()).await {
                        Ok(o) => o,
                        Err(e) => format!("error: {e:#}"),
                    }
                }
            } else if call.name == "subagent" {
                let v: Value = serde_json::from_str(&call.args).unwrap_or(Value::Null);
                let prompt = v["prompt"].as_str().unwrap_or("").to_string();
                let desc = v["description"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let agent_name = v["agent"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty() && *s != "general")
                    .map(str::to_string);
                let background = v["background"].as_bool().unwrap_or(false);
                let session_id = v["session_id"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                if prompt.is_empty() {
                    "error: subagent: prompt required".to_string()
                } else if cfg.depth >= cfg.max_depth {
                    "error: subagent: max nesting depth reached (agent.subagent_depth)".to_string()
                } else if let Some(name) = &agent_name {
                    if crate::agents::get(name).is_none() {
                        format!("error: subagent: unknown agent: {name}")
                    } else {
                        dispatch_subagent(
                            provider.clone(),
                            agent_name.as_deref(),
                            &prompt,
                            session_id.as_deref(),
                            cfg.parent_sid.as_deref(),
                            &req,
                            desc,
                            background,
                            cfg.clone(),
                            allow_all.clone(),
                            mcp.clone(),
                            queue.clone(),
                            &tx,
                        )
                        .await
                    }
                } else {
                    dispatch_subagent(
                        provider.clone(),
                        None,
                        &prompt,
                        session_id.as_deref(),
                        cfg.parent_sid.as_deref(),
                        &req,
                        desc,
                        background,
                        cfg.clone(),
                        allow_all.clone(),
                        mcp.clone(),
                        queue.clone(),
                        &tx,
                    )
                    .await
                }
            } else if call.name == "read_file" {
                let v: Value = serde_json::from_str(&call.args).unwrap_or(Value::Null);
                let p = v["path"].as_str().unwrap_or("").to_string();
                if !p.is_empty() && crate::files::is_image(&p) {
                    match crate::files::read_image(&p) {
                        Ok((mime, data)) => {
                            let msg =
                                format!("Image loaded: {p} ({mime}); the image is attached in the next message.");
                            pending_images.push((p.clone(), crate::chat::Image { mime, data }));
                            msg
                        }
                        Err(e) => format!("error: {e:#}"),
                    }
                } else {
                    match tools::execute("read_file", &call.args, mcp_now.as_deref()).await {
                        Ok(mut o) => {
                            if let Some((ip, ib)) = crate::instructions_for_file(&p) {
                                o.push_str(&format!("\n\n---\nProject instructions from {ip}:\n{ib}"));
                            }
                            o
                        }
                        Err(e) => format!("error: {e:#}"),
                    }
                }
            } else {
                match tools::execute(&call.name, &call.args, mcp_now.as_deref()).await {
                    Ok(o) => o,
                    Err(e) => format!("error: {e:#}"),
                }
            };
            let out = tools::budget(out, cfg.output_budget);
            let summary: String = out
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(70)
                .collect();
            let _ = tx.send(ApiEvent::Note(summary));
            msgs.push(Message::tool(&call.id, out));
            req.messages = msgs.clone();
        }
        if !pending_images.is_empty() {
            let names: Vec<String> = pending_images.iter().map(|(p, _)| p.clone()).collect();
            let imgs: Vec<crate::chat::Image> =
                pending_images.into_iter().map(|(_, i)| i).collect();
            msgs.push(
                Message::new(
                    Role::User,
                    format!("image(s) read via read_file: {}", names.join(", ")),
                )
                .with_images(imgs),
            );
            req.messages = msgs.clone();
        }
    }
}

fn deny_message(name: &str, feedback: &str) -> String {
    let f = feedback.trim();
    if f.is_empty() {
        "user denied this action".to_string()
    } else {
        format!(
            "The user rejected the {name} tool call and provided feedback: {f}\n\
             Adjust your approach according to this feedback and continue with a different plan."
        )
    }
}

fn run_bash_background(
    v: &Value,
    tx: &UnboundedSender<ApiEvent>,
    queue: Arc<Mutex<Vec<String>>>,
    budget: usize,
) -> String {
    let Some(cmd) = v["command"].as_str().filter(|s| !s.trim().is_empty()) else {
        return "error: bash: command required".to_string();
    };
    let workdir = v["workdir"].as_str().map(|s| s.to_string());
    let timeout = v["timeout"].as_u64();
    let desc: String = cmd.lines().next().unwrap_or("").chars().take(60).collect();
    let mut child = match tools::spawn_shell(cmd, workdir.as_deref()) {
        Ok(c) => c,
        Err(e) => return format!("error: bash: {e:#}"),
    };
    let id = crate::bg::start("bash", &desc);
    if let Some(p) = child.id() {
        crate::bg::attach_pid(&id, p);
    }
    let buf = Arc::new(Mutex::new(String::new()));
    crate::bg::attach_out(&id, buf.clone());
    let _ = tx.send(ApiEvent::Note(format!(
        "background task {id} started: {desc}"
    )));
    let out: Box<dyn tokio::io::AsyncRead + Unpin + Send> = match child.stdout.take() {
        Some(s) => Box::new(s),
        None => Box::new(tokio::io::empty()),
    };
    let err: Box<dyn tokio::io::AsyncRead + Unpin + Send> = match child.stderr.take() {
        Some(s) => Box::new(s),
        None => Box::new(tokio::io::empty()),
    };
    let r1 = tokio::spawn(pump_bg(out, id.clone(), buf.clone(), tx.clone()));
    let r2 = tokio::spawn(pump_bg(err, id.clone(), buf.clone(), tx.clone()));
    let tx2 = tx.clone();
    let id2 = id.clone();
    let desc2 = desc.clone();
    tokio::spawn(async move {
        let wait = async {
            let _ = r1.await;
            let _ = r2.await;
            child.wait().await
        };
        let st = match timeout {
            Some(t) => match tokio::time::timeout(std::time::Duration::from_secs(t.max(1)), wait).await {
                Err(_) => None,
                Ok(s) => s.ok(),
            },
            None => wait.await.ok(),
        };
        let out = buf.lock().unwrap().clone();
        let killed = crate::bg::killed(&id2);
        let code = st.and_then(|s| s.code());
        let msg = if killed {
            crate::bg::finish(&id2, Some(out.clone()));
            format!("Background task {id2} ({desc2}) was killed. Output before kill:\n{out}")
        } else if st.is_none() {
            let _ = crate::bg::kill(&id2);
            crate::bg::finish(&id2, None);
            format!("Background task {id2} ({desc2}) timed out and was stopped. Output:\n{out}")
        } else {
            let tail = if code == Some(0) {
                String::new()
            } else {
                format!("\nexit code: {}", code.unwrap_or(-1))
            };
            let body = if out.trim().is_empty() {
                "(no output)".to_string()
            } else {
                out.clone()
            };
            let text = format!("{body}{tail}");
            if code == Some(0) {
                crate::bg::finish(&id2, Some(text.clone()));
                let _ = tx2.send(ApiEvent::Note(format!(
                    "background task {id2} finished: {desc2}"
                )));
                tools::budget(
                    format!("Background task {id2} ({desc2}) finished. Output:\n{text}"),
                    budget,
                )
            } else {
                crate::bg::finish(&id2, Some(text.clone()));
                let _ = tx2.send(ApiEvent::Note(format!(
                    "background task {id2} failed (exit {code:?}): {desc2}"
                )));
                tools::budget(
                    format!("Background task {id2} ({desc2}) failed. Output:\n{text}"),
                    budget,
                )
            }
        };
        queue.lock().unwrap().push(tools::budget(msg, budget));
        let _ = tx2.send(ApiEvent::Wake);
    });
    format!(
        "Command moved to the background (task {id}). You will be notified automatically when it finishes; the notification will include the output. Do not poll task_status for completion; keep working on anything that does not depend on the result. Use task_kill to stop it."
    )
}

async fn pump_bg<R: tokio::io::AsyncRead + Unpin>(
    mut r: R,
    id: String,
    buf: Arc<Mutex<String>>,
    tx: UnboundedSender<ApiEvent>,
) {
    let mut b = [0u8; 4096];
    loop {
        match r.read(&mut b).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let s = String::from_utf8_lossy(&b[..n]).to_string();
                {
                    let mut g = buf.lock().unwrap();
                    g.push_str(&s);
                    if g.len() > 256 * 1024 {
                        let mut cut = g.len() - 256 * 1024;
                        while cut < g.len() && !g.is_char_boundary(cut) {
                            cut += 1;
                        }
                        g.drain(..cut);
                    }
                }
                crate::bg::append(&id, &s);
                let _ = tx.send(ApiEvent::BgOut { id: id.clone(), chunk: s });
            }
        }
    }
}

async fn ask_user(args: &str, tx: &UnboundedSender<ApiEvent>) -> Result<String> {
    let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
    let qs = v["questions"].as_array().cloned().unwrap_or_default();
    if qs.is_empty() {
        bail!("question: no questions given");
    }
    let (otx, orx) = oneshot::channel();
    tx.send(ApiEvent::Ask {
        name: "question".into(),
        args: args.to_string(),
        rx: otx,
    })
    .map_err(|_| anyhow!("closed"))?;
    let answer = orx.await.unwrap_or_default();
    if answer.trim().is_empty() {
        return Ok("The user dismissed this question.".into());
    }
    Ok(format!(
        "User has answered your questions: {}. You can now continue with the user's answers in mind.",
        answer.trim()
    ))
}

pub fn subagent_system() -> String {
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    format!(
        "You are a focused subagent of hi-derola working in the user's working directory: {cwd}\n\
         Complete the given task autonomously using the available tools. \
         The final message is returned to the parent agent as the tool result, so make it a \
         complete summary: what was done, files changed, key results, anything the parent must know. \
         The user cannot see this conversation and cannot answer questions: do not ask."
    )
}

fn build_sub_req(
    decl: Option<&crate::agents::AgentDecl>,
    prompt: &str,
    model: &str,
    max_tokens: Option<u32>,
    temperature: Option<f64>,
    top_p: Option<f64>,
) -> ChatRequest {
    let (system, model, temperature) = match decl {
        Some(d) => {
            let tail = subagent_system();
            let system = if d.prompt.trim().is_empty() {
                tail
            } else {
                format!("{}\n\n{tail}", d.prompt.trim())
            };
            (
                system,
                d.model.clone().unwrap_or_else(|| model.to_string()),
                d.temperature.or(temperature),
            )
        }
        None => (subagent_system(), model.to_string(), temperature),
    };
    ChatRequest {
        system,
        messages: vec![Message::new(Role::User, prompt.to_string())],
        model,
        max_tokens,
        temperature,
        top_p,
        stream: false,
        tools: Vec::new(),
    }
}

/// resolve + launch a subagent call from the tool loop (fresh or continued)
#[allow(clippy::too_many_arguments)]
async fn dispatch_subagent(
    provider: Arc<dyn Provider>,
    agent: Option<&str>,
    prompt: &str,
    session_id: Option<&str>,
    parent_sid: Option<&str>,
    req: &ChatRequest,
    desc: String,
    background: bool,
    cfg: AgentCfg,
    allow_all: Arc<AtomicBool>,
    mcp: McpSlot,
    queue: Arc<Mutex<Vec<String>>>,
    tx: &UnboundedSender<ApiEvent>,
) -> String {
    let (sub_req, sid, read_only) = match resolve_sub_req(
        agent,
        prompt,
        session_id,
        parent_sid,
        &req.model,
        req.max_tokens,
        req.temperature,
        req.top_p,
    ) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if background {
        spawn_standalone_subagent(
            provider,
            sub_req,
            sid,
            desc,
            read_only,
            cfg,
            allow_all,
            mcp,
            queue,
            tx.clone(),
        )
    } else {
        let _ = tx.send(ApiEvent::Note(format!("subagent started: {desc}")));
        match run_subagent(provider, sub_req, sid, cfg, allow_all, tx, mcp, read_only, desc.clone())
            .await
        {
            Ok(t) => {
                let _ = tx.send(ApiEvent::Note(format!("subagent done: {desc}")));
                t
            }
            Err(e) => format!("error: subagent failed: {e:#}"),
        }
    }
}

/// Build the request for a subagent run. When session_id refers to a stored child
/// of this session the conversation is continued, otherwise a fresh one starts.
/// Returns (request, session id, read_only).
#[allow(clippy::too_many_arguments)]
pub fn resolve_sub_req(
    agent: Option<&str>,
    prompt: &str,
    session_id: Option<&str>,
    parent_sid: Option<&str>,
    model: &str,
    max_tokens: Option<u32>,
    temperature: Option<f64>,
    top_p: Option<f64>,
) -> Result<(ChatRequest, String, bool), String> {
    let decl = agent.and_then(crate::agents::get);
    let read_only = decl.as_ref().map(|d| d.read_only).unwrap_or(false);
    let mut sub_req = build_sub_req(decl.as_ref(), prompt, model, max_tokens, temperature, top_p);
    if let Some(sid) = session_id {
        let st = crate::sessions::load(sid)
            .map_err(|_| format!("error: subagent session not found: {sid}"))?;
        if st.parent.is_none() || st.parent.as_deref() != parent_sid {
            return Err("error: subagent session was not started from this session".to_string());
        }
        sub_req.system = st.system;
        sub_req.messages = st.messages;
        sub_req.messages.push(Message::new(Role::User, prompt.to_string()));
        Ok((sub_req, sid.to_string(), read_only))
    } else {
        Ok((sub_req, crate::sessions::new_id(), read_only))
    }
}

/// Spawn a background subagent outside of the tool loop (subagent tool with background=true
/// and manual @agent invocations from the UIs). Returns the task id.
#[allow(clippy::too_many_arguments)]
pub fn spawn_standalone_subagent(
    provider: Arc<dyn Provider>,
    sub_req: ChatRequest,
    sid: String,
    desc: String,
    read_only: bool,
    cfg: AgentCfg,
    allow_all: Arc<AtomicBool>,
    mcp: McpSlot,
    queue: Arc<Mutex<Vec<String>>>,
    tx: UnboundedSender<ApiEvent>,
) -> String {
    let desc = if desc.trim().is_empty() {
        sub_req
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .map(|m| {
                m.content
                    .lines()
                    .next()
                    .unwrap_or("subagent")
                    .chars()
                    .take(60)
                    .collect()
            })
            .unwrap_or_else(|| "subagent".to_string())
    } else {
        desc
    };
    let id = crate::bg::start("subagent", &desc);
    let _ = tx.send(ApiEvent::Note(format!(
        "background task {id} started: {desc}"
    )));
    let tx2 = tx.clone();
    let id2 = id.clone();
    let desc2 = desc.clone();
    let budget = cfg.output_budget;
    let inner = tokio::spawn({
        let tx3 = tx2.clone();
        let cfg2 = cfg.clone();
        let allow2 = allow_all.clone();
        async move {
            run_subagent(
                provider,
                sub_req,
                sid,
                cfg2,
                allow2,
                &tx3,
                mcp,
                read_only,
                desc.clone(),
            )
            .await
        }
    });
    crate::bg::attach_abort(&id, inner.abort_handle());
    tokio::spawn(async move {
        let msg = match inner.await {
            Ok(Ok(text)) => {
                crate::bg::finish(&id2, Some(text.clone()));
                let _ = tx2.send(ApiEvent::Note(format!(
                    "background task {id2} finished: {desc2}"
                )));
                tools::budget(
                    format!("Background task {id2} ({desc2}) finished. Result:\n{text}"),
                    budget,
                )
            }
            Ok(Err(e)) => {
                crate::bg::finish(&id2, None);
                let _ = tx2.send(ApiEvent::Note(format!(
                    "background task {id2} failed: {e:#}"
                )));
                format!("Background task {id2} ({desc2}) failed: {e:#}")
            }
            Err(_) => {
                crate::bg::finish(&id2, None);
                format!("Background task {id2} ({desc2}) was killed.")
            }
        };
        queue.lock().unwrap().push(msg);
        let _ = tx2.send(ApiEvent::Wake);
    });
    format!(
        "started in background as task {id}; the result will arrive as a new message when the task finishes (progress: task_status, stop it: task_kill)"
    )
}

#[allow(clippy::too_many_arguments)]
fn run_subagent<'a>(
    provider: Arc<dyn Provider>,
    req: ChatRequest,
    sid: String,
    cfg: AgentCfg,
    allow_all: Arc<AtomicBool>,
    tx: &'a UnboundedSender<ApiEvent>,
    mcp: McpSlot,
    read_only: bool,
    desc: String,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + 'a>> {
    Box::pin(async move {
        let desc = desc;
        let (stx, mut srx) = tokio::sync::mpsc::unbounded_channel();
        let fwd_tx = tx.clone();
        let result: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let messages: Arc<Mutex<Option<Vec<Message>>>> = Arc::new(Mutex::new(None));
        let r2 = result.clone();
        let m2 = messages.clone();
        let fwd = tokio::spawn(async move {
            while let Some(ev) = srx.recv().await {
                match ev {
                    ApiEvent::Done { text, messages } => {
                        *r2.lock().unwrap() = Some(text);
                        *m2.lock().unwrap() = Some(messages);
                    }
                    ApiEvent::Note(_)
                    | ApiEvent::Tool { .. }
                    | ApiEvent::Confirm { .. }
                    | ApiEvent::Usage { .. }
                    | ApiEvent::BgOut { .. }
                    | ApiEvent::Failed(_) => {
                        let _ = fwd_tx.send(ev);
                    }
                    _ => {}
                }
            }
        });
        let sub_cfg = AgentCfg {
            nested: true,
            depth: cfg.depth + 1,
            read_only,
            parent_sid: None,
            ..cfg.clone()
        };
        let fut: std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> =
            Box::pin(run(
                provider,
                req.clone(),
                stx,
                allow_all,
                mcp,
                Arc::new(Mutex::new(Vec::new())),
                sub_cfg,
            ));
        let res = fut.await;
        let _ = fwd.await; // fix: abort could drop the queued Done before it was captured
        if let Err(e) = res {
            return Err(e);
        }
        let text = result.lock().unwrap().take();
        // persist the subagent conversation under a fixed id so the model can
        // continue it later via the session_id argument
        if let Some(msgs) = messages.lock().unwrap().take() {
            if !msgs.is_empty() {
                let st = crate::sessions::StoredSession {
                    id: sid.clone(),
                    title: format!("↳ {desc}"),
                    created: 0,
                    updated: 0,
                    system: req.system.clone(),
                    messages: msgs,
                    tokens_in: 0,
                    tokens_out: 0,
                    cost: 0.0,
                    todos: Vec::new(),
                    parent: cfg.parent_sid.clone(),
                    changes: Vec::new(),
                    queue: Vec::new(),
                };
                let _ = crate::sessions::save(&st);
            }
        }
        let text = text.unwrap_or_else(|| "(subagent finished without a final message)".into());
        Ok(format!(
            "{text}\n\n(subagent session: {sid}; pass it back as session_id to continue this conversation with full context)"
        ))
    })
}

async fn wrap_up(
    provider: Arc<dyn Provider>,
    req: &mut ChatRequest,
    msgs: &mut Vec<Message>,
    tx: &UnboundedSender<ApiEvent>,
    max_rounds: usize,
) -> Result<()> {
    let _ = tx.send(ApiEvent::Note(format!(
        "tool loop exceeded {max_rounds} rounds, asking the model to wrap up"
    )));
    msgs.push(Message::new(
        Role::User,
        "The tool call limit has been reached. Stop making changes and write a short final summary: what was done, what was changed, what remains.",
    ));
    // keep tool definitions so the cached system+tools prefix stays intact;
    // tool calls from here on are answered with an error instead of executing
    req.messages = msgs.clone();
    let mut text = String::new();
    for _ in 0..3 {
        match provider.chat(req, tx).await {
            Ok(r) if !r.text.trim().is_empty() => {
                text = r.text;
                break;
            }
            Ok(r) if !r.calls.is_empty() => {
                msgs.push(Message::new(Role::Assistant, r.text.clone()).with_calls(r.calls.clone()));
                for c in r.calls {
                    msgs.push(Message::tool(
                        &c.id,
                        "error: the tool call limit has been reached; tools are disabled. Reply with a text summary now.",
                    ));
                }
                req.messages = msgs.clone();
            }
            Ok(_) => break,
            Err(e) => {
                let _ = tx.send(ApiEvent::Note(format!("wrap-up failed: {e:#}")));
                break;
            }
        }
    }
    if text.is_empty() {
        text = "stopped: tool loop limit reached".into();
    }
    msgs.push(Message::new(Role::Assistant, text.clone()));
    let _ = tx.send(ApiEvent::Done {
        text,
        messages: msgs.clone(),
    });
    Ok(())
}

fn msg_tokens(m: &Message) -> usize {
    let chars: usize = m
        .content
        .len()
        + m.tool_calls
            .iter()
            .map(|c| c.args.len() + c.name.len())
            .sum::<usize>();
    chars / 4 + m.images.len() * IMG_TOKEN_EST
}

/// token-budgeted split: keep the newest messages within `keep_tokens`,
/// boundary snapped forward past Tool results so call/result pairs stay together
fn compact_split(msgs: &[Message], keep_tokens: usize) -> Option<usize> {
    if msgs.len() < COMPACT_MIN_MSGS || msgs.first()?.role != Role::User {
        return None;
    }
    let mut cut = msgs.len();
    let mut acc = 0usize;
    while cut > 1 {
        let t = msg_tokens(&msgs[cut - 1]);
        if acc + t > keep_tokens && cut < msgs.len() {
            break;
        }
        acc += t;
        cut -= 1;
    }
    while cut < msgs.len() && msgs[cut].role == Role::Tool {
        cut += 1;
    }
    if cut <= 1 || cut >= msgs.len() {
        return None;
    }
    Some(cut)
}

fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end < s.len() && !s.is_char_boundary(end) {
        end += 1;
    }
    let mut out = s[..end].to_string();
    out.push_str("...");
    out
}

fn transcript(msgs: &[Message]) -> String {
    let mut t = String::new();
    for m in msgs {
        match m.role {
            Role::User => t.push_str(&format!("[user]\n{}\n\n", clip(&m.content, 4000))),
            Role::Assistant => {
                if !m.content.trim().is_empty() {
                    t.push_str(&format!("[assistant]\n{}\n\n", clip(&m.content, 4000)));
                }
                for c in &m.tool_calls {
                    t.push_str(&format!("[assistant called tool] {} {}\n\n", c.name, clip(&c.args, 300)));
                }
            }
            Role::Tool => t.push_str(&format!("[tool result]\n{}\n\n", clip(&m.content, COMPACT_TOOL_CLIP))),
        }
    }
    t
}

/// Manual compaction entry point for /compact in the UIs.
/// `keep` = [agent.compaction].keep (0 falls back to the default).
pub async fn compact_session(
    provider: Arc<dyn Provider>,
    req: &ChatRequest,
    msgs: &mut Vec<Message>,
    tx: &UnboundedSender<ApiEvent>,
    keep: usize,
) -> bool {
    compact(&provider, req, msgs, tx, keep).await
}

async fn compact(
    provider: &Arc<dyn Provider>,
    req: &ChatRequest,
    msgs: &mut Vec<Message>,
    tx: &UnboundedSender<ApiEvent>,
    keep_tokens: usize,
) -> bool {
    let Some(cut) = compact_split(msgs, effective_keep(keep_tokens)) else {
        return false;
    };
    let text = transcript(&msgs[1..cut]);
    let kept = msgs.len() - cut;
    // summary left by a previous compaction lives in the first user message;
    // on re-compaction it must be merged, not dropped or duplicated
    let prior = prior_summary(&msgs[0].content).map(str::to_string);
    const SUMMARY_RULES: &str = "Sections: Objective; Requirements; Decisions; Work State (Completed / Active / Blocked); Next Move; Relevant Files (up to 15, one line each); Important Context.\n\
         Rules: dense facts only, no fluff; keep file paths, commands and error messages exact; do not restate coding conventions from AGENTS.md (they are provided to the next agent separately); at most 600 words.";
    const MERGE_RULES: &str = "The <prior-summary> summarizes everything that happened before the transcript above. Write a new summary that merges both sources. The prior summary is discarded after this: anything you do not carry into the new summary is lost.\n\
         When merging:\n\
         - carry objectives, constraints, user directives, decisions and parallel workstreams from the prior summary even when the transcript does not mention them; drop only what is finished and no longer needed;\n\
         - the transcript is more recent than the prior summary: where they conflict, the transcript wins - state the corrected fact and drop the old claim;\n\
         - add new progress, decisions, constraints and context from the transcript;\n\
         - move finished work from Work State / Active to Completed;\n\
         - if a blocker has been resolved, update it while keeping details still needed to continue;\n\
         - update Objective and Next Move to reflect the current state.";
    let task = match prior.as_deref() {
        None => format!(
            "Conversation transcript:\n\n{text}\n\nWrite the summary now, following the template sections."
        ),
        Some(p) => format!(
            "Conversation transcript:\n\n{text}\n\nHere is the summary of the conversation before the transcript:\n\n<prior-summary>\n{p}\n</prior-summary>\n\n{MERGE_RULES}\n\nWrite the merged summary now, following the template sections."
        ),
    };
    let mut sum_req = ChatRequest {
        system: format!(
            "You summarize coding agent conversations so work can continue seamlessly. Reply following the template exactly.\n\n{SUMMARY_RULES}"
        ),
        messages: vec![Message::new(Role::User, task)],
        model: req.model.clone(),
        max_tokens: req.max_tokens,
        temperature: req.temperature,
        top_p: req.top_p,
        stream: false,
        tools: Vec::new(),
    };
    let (btx, _brx) = tokio::sync::mpsc::unbounded_channel();
    let mut summary = match provider.chat(&sum_req, &btx).await {
        Ok(r) if !r.text.trim().is_empty() => r.text.trim().to_string(),
        Ok(_) => {
            let _ = tx.send(ApiEvent::Note("compaction failed: empty summary".into()));
            return false;
        }
        Err(e) => {
            let _ = tx.send(ApiEvent::Note(format!("compaction failed: {e:#}")));
            return false;
        }
    };
    if !summary.to_lowercase().contains("objective") {
        let _ = tx.send(ApiEvent::Note(
            "summary missed the template, retrying once".into(),
        ));
        sum_req.messages.push(Message::new(Role::Assistant, summary.clone()));
        sum_req.messages.push(Message::new(
            Role::User,
            "Your reply did not follow the required template. Rewrite it with the exact sections: Objective, Requirements, Decisions, Work State, Next Move, Relevant Files, Important Context.",
        ));
        if let Ok(r) = provider.chat(&sum_req, &btx).await {
            let t = r.text.trim().to_string();
            if !t.is_empty() {
                summary = t;
            }
        }
    }
    let mut first = msgs[0].clone();
    apply_summary(&mut first, &summary);
    msgs.drain(1..cut);
    msgs[0] = first;
    let _ = tx.send(ApiEvent::Note(format!(
        "{}: {} messages summarized, {} kept",
        if prior.is_some() {
            "context compacted (merged with the previous summary)"
        } else {
            "context compacted"
        },
        cut - 1,
        kept
    )));
    true
}

/// summary text left by a previous compaction in the first user message, if any
fn prior_summary(content: &str) -> Option<&str> {
    content
        .split_once(SUMMARY_MARKER)
        .map(|(_, s)| s.trim())
        .filter(|s| !s.is_empty())
}

/// attach `summary` to the first user message, replacing any previous summary
/// so repeated compactions do not accumulate stale summaries
fn apply_summary(first: &mut Message, summary: &str) {
    if let Some(i) = first.content.find(SUMMARY_MARKER) {
        first.content.truncate(i);
    }
    first.content.push_str(SUMMARY_MARKER);
    first.content.push_str(summary);
}

fn est_tokens(msgs: &[Message]) -> usize {
    let chars: usize = msgs
        .iter()
        .map(|m| {
            m.content.len()
                + m.tool_calls
                    .iter()
                    .map(|c| c.args.len() + c.name.len())
                    .sum::<usize>()
        })
        .sum();
    chars / 4
}

fn is_overflow(e: &anyhow::Error) -> bool {
    let s = format!("{e:#}").to_lowercase();
    [
        "context length",
        "maximum context",
        "context_length_exceeded",
        "context window",
        "prompt is too long",
        "too many tokens",
        "input length exceeds",
        "reduce the length",
    ]
    .iter()
    .any(|m| s.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(role: Role, text: &str) -> Message {
        Message::new(role, text)
    }

    #[test]
    fn split_and_overflow() {
        // 6 messages < COMPACT_MIN_MSGS -> never split
        let msgs = vec![
            m(Role::User, "task"),
            m(Role::Assistant, "a1"),
            m(Role::Tool, "t1"),
            m(Role::Assistant, "a2"),
            m(Role::User, "u2"),
            m(Role::Assistant, "a3"),
        ];
        assert_eq!(compact_split(&msgs, 100), None);
        // 13 messages of 40 chars (10 tokens each), budget 50 tokens -> keep last 5
        let long: Vec<Message> = std::iter::once(m(Role::User, "task"))
            .chain((0..12).map(|_| m(Role::Assistant, &"x".repeat(40))))
            .collect();
        assert_eq!(compact_split(&long, 50), Some(8));
        // boundary must not orphan a tool result: snap forward past Tool
        let body = "y".repeat(40);
        let orphan: Vec<Message> = std::iter::once(m(Role::User, "task"))
            .chain((0..6).map(|_| m(Role::Assistant, &body)))
            .chain(std::iter::once(m(Role::Tool, &body)))
            .chain((0..4).map(|_| m(Role::Assistant, &body)))
            .collect();
        let cut = compact_split(&orphan, 50).unwrap();
        assert_eq!(cut, 8);
        assert_eq!(orphan[cut].role, Role::Assistant);
        // a single message larger than the budget still leaves one kept
        let huge: Vec<Message> = std::iter::once(m(Role::User, "task"))
            .chain((0..7).map(|_| m(Role::Assistant, "small")))
            .chain(std::iter::once(m(Role::Assistant, &"z".repeat(40_000))))
            .collect();
        assert_eq!(compact_split(&huge, 100), Some(8));
        let e = anyhow::anyhow!("400 Bad Request: prompt is too long: 200000 tokens > 180000 maximum");
        assert!(is_overflow(&e));
        assert!(!is_overflow(&anyhow::anyhow!("401 unauthorized")));
    }

    #[test]
    fn compaction_knobs() {
        let mut c = CompactionCfg::default();
        assert!(c.auto);
        assert_eq!(c.keep, 15_000);
        // buffer=0 -> a quarter of the window (legacy 75% trigger)
        assert_eq!(compact_headroom(&c, 200_000), 50_000);
        assert!(!auto_compact_due(&c, 200_000, 150_000, 0));
        assert!(auto_compact_due(&c, 200_000, 150_001, 0));
        // the estimate counts too when no usage was observed yet
        assert!(!auto_compact_due(&c, 200_000, 0, 150_000));
        assert!(auto_compact_due(&c, 200_000, 0, 150_001));
        // auto off -> never proactive (overflow recovery / /compact still work)
        c.auto = false;
        assert!(!auto_compact_due(&c, 200_000, 500_000, 0));
        c.auto = true;
        // unknown window -> no proactive compaction
        assert!(!auto_compact_due(&c, 0, 500_000, 0));
        // absolute buffer overrides the quarter
        c.buffer = 20_000;
        assert_eq!(compact_headroom(&c, 200_000), 20_000);
        assert!(!auto_compact_due(&c, 200_000, 180_000, 0));
        assert!(auto_compact_due(&c, 200_000, 180_001, 0));
        // keep=0 falls back to the built-in default
        assert_eq!(effective_keep(0), 15_000);
        assert_eq!(effective_keep(30_000), 30_000);
    }

    #[test]
    fn output_fitting() {
        // unknown window -> untouched
        assert_eq!(clamp_output(None, 0, 90_000), None);
        assert_eq!(clamp_output(Some(4_096), 0, 90_000), Some(4_096));
        // plenty of room -> untouched (even the provider default)
        assert_eq!(clamp_output(None, 200_000, 10_000), None);
        // configured cap larger than the remaining room -> shrunk to the floor
        assert_eq!(clamp_output(Some(16_000), 10_000, 9_900), Some(1_024));
        // user cap smaller than the room -> untouched
        assert_eq!(clamp_output(Some(2_000), 200_000, 100_000), Some(2_000));
        // no configured cap, tight room -> shrunk default
        assert_eq!(clamp_output(None, 8_192, 7_900), Some(1_024));
        assert_eq!(clamp_output(None, 100_000, 86_000), Some(1_100));
    }

    #[test]
    fn clip_and_est() {
        assert_eq!(clip("hello", 10), "hello");
        assert!(clip("0123456789abcdef", 8).ends_with("..."));
        assert_eq!(est_tokens(&[m(Role::User, "x")]), 0);
        assert_eq!(est_tokens(&[m(Role::User, "abcd")]), 1);
    }

    #[test]
    fn summary_merge() {
        // plain first message -> no prior summary
        assert_eq!(prior_summary("fix the parser"), None);
        assert_eq!(prior_summary(""), None);
        // a marker with an empty body is not a summary
        assert_eq!(prior_summary("task\n\n---\nSummary of the earlier conversation (dropped to fit the context window):\n  "), None);

        // first compaction attaches the summary after the original task text
        let mut first = m(Role::User, "fix the parser");
        apply_summary(&mut first, "Objective: fix the parser");
        assert!(first.content.starts_with("fix the parser\n\n---\nSummary"));
        assert!(first.content.ends_with("Objective: fix the parser"));
        assert_eq!(prior_summary(&first.content), Some("Objective: fix the parser"));

        // second compaction REPLACES the old summary instead of accumulating
        apply_summary(&mut first, "Objective: fix the parser\nWork State: done");
        assert_eq!(first.content.matches(SUMMARY_MARKER).count(), 1);
        assert!(first.content.starts_with("fix the parser\n\n---\nSummary"));
        assert!(!first.content.contains("done\n\n---\nSummary"));
        assert_eq!(
            prior_summary(&first.content),
            Some("Objective: fix the parser\nWork State: done")
        );
        // the original task text survives every re-compaction
        assert!(prior_summary(&first.content).is_some());
        assert!(first.content.split_once(SUMMARY_MARKER).unwrap().0 == "fix the parser");

        // a merged first message keeps compact_split happy: role stays User
        let msgs: Vec<Message> = std::iter::once(first)
            .chain((0..12).map(|_| m(Role::Assistant, &"x".repeat(40))))
            .collect();
        assert!(compact_split(&msgs, 50).is_some());
        // and the prior summary never leaks into the transcript being summarized
        let cut = compact_split(&msgs, 50).unwrap();
        let t = transcript(&msgs[1..cut]);
        assert!(!t.contains("Objective: fix the parser"));
    }
}
