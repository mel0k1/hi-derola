use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;

use crate::config::McpConfig;
use crate::provider::ToolSpec;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_SAMPLING_TOKENS: u32 = 4096;

struct McpTool {
    name: String,
    description: String,
    schema: Value,
}

struct McpResource {
    uri: String,
    name: String,
    description: String,
    mime: Option<String>,
}

struct McpPromptArg {
    name: String,
    description: String,
    required: bool,
}

struct McpPrompt {
    name: String,
    description: String,
    arguments: Vec<McpPromptArg>,
}

fn has_cap(caps: &Value, key: &str) -> bool {
    let c = &caps[key];
    c.is_object() || c.as_bool() == Some(true)
}

fn parse_tools(items: &[Value]) -> Vec<McpTool> {
    items
        .iter()
        .map(|t| McpTool {
            name: t["name"].as_str().unwrap_or("").to_string(),
            description: t["description"].as_str().unwrap_or("").to_string(),
            schema: t["inputSchema"].clone(),
        })
        .filter(|t| !t.name.is_empty())
        .collect()
}

fn parse_resources(items: &[Value]) -> Vec<McpResource> {
    items
        .iter()
        .map(|r| McpResource {
            uri: r["uri"].as_str().unwrap_or("").to_string(),
            name: r["name"].as_str().unwrap_or("").to_string(),
            description: r["description"].as_str().unwrap_or("").to_string(),
            mime: r["mimeType"].as_str().map(|s| s.to_string()),
        })
        .filter(|r| !r.uri.is_empty())
        .collect()
}

fn parse_prompts(items: &[Value]) -> Vec<McpPrompt> {
    items
        .iter()
        .map(|p| McpPrompt {
            name: p["name"].as_str().unwrap_or("").to_string(),
            description: p["description"].as_str().unwrap_or("").to_string(),
            arguments: p["arguments"]
                .as_array()
                .map(|args| {
                    args.iter()
                        .map(|a| McpPromptArg {
                            name: a["name"].as_str().unwrap_or("").to_string(),
                            description: a["description"].as_str().unwrap_or("").to_string(),
                            required: a["required"].as_bool().unwrap_or(false),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .filter(|p| !p.name.is_empty())
        .collect()
}

fn render_resource_contents(res: &Value) -> String {
    let mut out = String::new();
    for c in res["contents"].as_array().into_iter().flatten() {
        let uri = c["uri"].as_str().unwrap_or("");
        if let Some(t) = c["text"].as_str() {
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(t);
        } else if let Some(b) = c["blob"].as_str() {
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(&format!("[binary resource {uri}, ~{} bytes]", b.len() * 3 / 4));
        }
    }
    if out.is_empty() {
        out.push_str("(empty resource)");
    }
    out
}

fn parse_prompt_messages(res: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for m in res["messages"].as_array().into_iter().flatten() {
        let role = m["role"].as_str().unwrap_or("user").to_string();
        if m["content"]["type"].as_str() == Some("text") {
            if let Some(t) = m["content"]["text"].as_str() {
                out.push((role, t.to_string()));
            }
        }
    }
    out
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("{n} {word}")
    } else {
        format!("{n} {word}s")
    }
}

// ---------- client hooks: roots + sampling ----------

/// one sampling/createMessage request from a server
pub struct SampleReq {
    pub server: String,
    pub system: Option<String>,
    /// (role, text) pairs; only text content is forwarded
    pub messages: Vec<(String, String)>,
    pub max_tokens: u32,
}

pub struct SampleOut {
    pub model: String,
    pub text: String,
}

pub type SampleFut = Pin<Box<dyn Future<Output = Result<SampleOut>> + Send>>;
pub type Sampler = Arc<dyn Fn(SampleReq) -> SampleFut + Send + Sync>;

/// one notifications/message entry from an mcp server
#[derive(Clone)]
pub struct McpLogEntry {
    pub server: String,
    pub level: String,
    pub logger: String,
    pub data: String,
}

/// ring buffer size for mcp log messages across all servers
pub const MAX_LOGS: usize = 500;

pub type McpLogBuf = Arc<std::sync::Mutex<VecDeque<McpLogEntry>>>;

/// one notifications/resources/updated subscriber set lives on Shared; this is
/// the note sink shared with every connected server (resource updates land in
/// the chat)
#[derive(Clone, Default)]
pub struct McpHooks {
    pub roots: Arc<RwLock<Vec<String>>>,
    pub sampler: Option<Sampler>,
    pub notes: Option<tokio::sync::mpsc::UnboundedSender<crate::provider::ApiEvent>>,
    /// notifications/message ring buffer shared by every server
    pub logs: McpLogBuf,
}

impl McpHooks {
    /// expose one workspace dir (defaults to cwd) as a root
    pub fn workspace(dir: Option<std::path::PathBuf>) -> Self {
        let dir = dir.or_else(|| std::env::current_dir().ok()).unwrap_or_default();
        let s = dir.display().to_string();
        Self {
            roots: Arc::new(RwLock::new(if s.is_empty() { Vec::new() } else { vec![s] })),
            sampler: None,
            notes: None,
            logs: Default::default(),
        }
    }

    pub fn with_sampler(mut self, sampler: Sampler) -> Self {
        self.sampler = Some(sampler);
        self
    }

    pub fn with_notes(
        mut self,
        tx: tokio::sync::mpsc::UnboundedSender<crate::provider::ApiEvent>,
    ) -> Self {
        self.notes = Some(tx);
        self
    }
}

/// default sampler: one-shot completion on hi-derola's own provider, with a
/// Note in the chat so the user sees what the server asked for
pub fn default_sampler(
    provider: Arc<dyn crate::provider::Provider>,
    model: String,
    temperature: Option<f64>,
    notes: tokio::sync::mpsc::UnboundedSender<crate::provider::ApiEvent>,
) -> Sampler {
    Arc::new(move |req| {
        let provider = provider.clone();
        let model = model.clone();
        let notes = notes.clone();
        Box::pin(async move {
            let preview = req
                .messages
                .iter()
                .find(|(r, _)| r == "user")
                .map(|(_, t)| crate::provider::truncate(t))
                .unwrap_or_default();
            let _ = notes.send(crate::provider::ApiEvent::Note(format!(
                "mcp {}: sampling via {model} (max {} tok){}",
                req.server,
                req.max_tokens,
                if preview.is_empty() {
                    String::new()
                } else {
                    format!(": {preview}")
                }
            )));
            let messages: Vec<crate::chat::Message> = req
                .messages
                .iter()
                .map(|(r, c)| {
                    crate::chat::Message::new(
                        if r == "assistant" {
                            crate::chat::Role::Assistant
                        } else {
                            crate::chat::Role::User
                        },
                        c.clone(),
                    )
                })
                .collect();
            let creq = crate::provider::ChatRequest {
                system: req.system.unwrap_or_default(),
                messages,
                model: model.clone(),
                max_tokens: Some(req.max_tokens),
                temperature,
                top_p: None,
                stream: false,
                tools: Vec::new(),
            };
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            let reply = provider.chat(&creq, &tx).await?;
            Ok(SampleOut {
                model,
                text: reply.text,
            })
        })
    })
}

fn file_uri(dir: &str) -> String {
    let p = dir.replace('\\', "/");
    if p.starts_with('/') {
        format!("file://{p}")
    } else {
        format!("file:///{p}")
    }
}

fn root_name(dir: &str) -> String {
    let trimmed = dir.trim_end_matches(['/', '\\']);
    trimmed
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(dir)
        .to_string()
}

/// syslog-style severity used by notifications/message; debug is the least
/// severe, emergency the most
fn log_level_rank(level: &str) -> Option<u8> {
    match level {
        "debug" => Some(0),
        "info" => Some(1),
        "notice" => Some(2),
        "warning" => Some(3),
        "error" => Some(4),
        "critical" => Some(5),
        "alert" => Some(6),
        "emergency" => Some(7),
        _ => None,
    }
}

// ---------- transport ----------

struct HttpCtx {
    url: String,
    http: reqwest::Client,
    headers: BTreeMap<String, String>,
    oauth: Option<crate::config::McpOAuthCfg>,
    session: std::sync::Mutex<Option<String>>,
}

/// how outgoing messages are delivered and how replies to server->client
/// requests are sent back
enum Reply {
    Stdio { stdin: Arc<Mutex<ChildStdin>> },
    Http(Arc<HttpCtx>),
}

/// state shared between the request path and the background reader task
struct Shared {
    name: String,
    reply: Reply,
    hooks: McpHooks,
    sampling: bool,
    next_id: AtomicU64,
    stale_tools: AtomicBool,
    stale_resources: AtomicBool,
    stale_prompts: AtomicBool,
    closed: AtomicBool,
    /// server advertised resources.subscribe
    res_sub: AtomicBool,
    /// uris this client subscribed to (resources/subscribe)
    subs: std::sync::Mutex<Vec<String>>,
    /// server advertised logging
    logging: AtomicBool,
    /// logging = false drops notifications/message from this server
    logging_on: bool,
}

type Pending = Arc<Mutex<BTreeMap<u64, tokio::sync::oneshot::Sender<Result<Value>>>>>;

struct McpServer {
    shared: Arc<Shared>,
    pending: Pending,
    child: Option<Child>,
    /// background task routing server -> client traffic; aborted on drop
    task: Option<tokio::task::JoinHandle<()>>,
    tools: Vec<McpTool>,
    resources: Vec<McpResource>,
    prompts: Vec<McpPrompt>,
}

impl McpServer {
    fn summary(&self) -> String {
        let mut parts = vec![plural(self.tools.len(), "tool")];
        if !self.resources.is_empty() {
            parts.push(plural(self.resources.len(), "resource"));
        }
        if !self.prompts.is_empty() {
            parts.push(plural(self.prompts.len(), "prompt"));
        }
        parts.join(", ")
    }

    /// fetch a paginated list method (tools/list, resources/list, prompts/list)
    async fn list_page(&self, method: &str, key: &str) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut params = json!({});
            if let Some(c) = &cursor {
                params["cursor"] = json!(c);
            }
            let res = self.request_t(REQUEST_TIMEOUT, method, params).await?;
            if let Some(arr) = res[key].as_array() {
                out.extend(arr.clone());
            }
            cursor = res["nextCursor"].as_str().map(|s| s.to_string());
            if cursor.is_none() || out.len() > 1000 {
                break;
            }
        }
        Ok(out)
    }

    async fn read_resource(&self, uri: &str) -> Result<String> {
        let res = self
            .request_t(CALL_TIMEOUT, "resources/read", json!({"uri": uri}))
            .await?;
        Ok(render_resource_contents(&res))
    }

    /// returns prompt messages as (role, text) pairs
    async fn get_prompt(&self, name: &str, args: &Value) -> Result<Vec<(String, String)>> {
        let res = self
            .request_t(
                CALL_TIMEOUT,
                "prompts/get",
                json!({"name": name, "arguments": args}),
            )
            .await?;
        Ok(parse_prompt_messages(&res))
    }

    /// re-list tools after notifications/tools/list_changed
    async fn refresh_tools(&mut self) {
        if !self.shared.stale_tools.swap(false, Ordering::Relaxed) {
            return;
        }
        match self.list_page("tools/list", "tools").await {
            Ok(items) => self.tools = parse_tools(&items),
            Err(_) => self.shared.stale_tools.store(true, Ordering::Relaxed),
        }
    }

    async fn refresh_resources(&mut self) {
        if !self.shared.stale_resources.swap(false, Ordering::Relaxed) {
            return;
        }
        match self.list_page("resources/list", "resources").await {
            Ok(items) => self.resources = parse_resources(&items),
            Err(_) => self.shared.stale_resources.store(true, Ordering::Relaxed),
        }
    }

    async fn refresh_prompts(&mut self) {
        if !self.shared.stale_prompts.swap(false, Ordering::Relaxed) {
            return;
        }
        match self.list_page("prompts/list", "prompts").await {
            Ok(items) => self.prompts = parse_prompts(&items),
            Err(_) => self.shared.stale_prompts.store(true, Ordering::Relaxed),
        }
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        if let Some(t) = self.task.take() {
            t.abort();
        }
        if let Some(c) = &mut self.child {
            let _ = c.start_kill();
        }
    }
}

fn extract_result(v: Value, name: &str) -> Result<Value> {
    if let Some(e) = v.get("error") {
        bail!(
            "mcp {}: {}",
            name,
            e["message"].as_str().unwrap_or("error")
        );
    }
    Ok(v["result"].clone())
}

/// route one incoming jsonrpc value: responses go to pending waiters,
/// server requests get answered, notifications update stale flags
async fn dispatch_incoming(shared: &Arc<Shared>, pending: &Pending, v: Value) {
    let method = v
        .get("method")
        .and_then(|m| m.as_str())
        .map(|s| s.to_string());
    let id = v.get("id").cloned();
    match (method, id) {
        (Some(method), Some(id)) => {
            handle_server_request(shared, id, &method, v.get("params").cloned().unwrap_or(json!({})))
                .await;
        }
        (Some(method), None) => {
            let params = v.get("params").cloned().unwrap_or(json!({}));
            handle_notification(shared, pending, &method, &params);
        }
        (None, Some(id)) => {
            if let Some(n) = id.as_u64() {
                if let Some(tx) = pending.lock().await.remove(&n) {
                    let _ = tx.send(extract_result(v, &shared.name));
                }
            }
        }
        (None, None) => {}
    }
}

fn handle_notification(shared: &Arc<Shared>, pending: &Pending, method: &str, params: &Value) {
    match method {
        "notifications/tools/list_changed" => {
            shared.stale_tools.store(true, Ordering::Relaxed);
        }
        "notifications/resources/list_changed" => {
            shared.stale_resources.store(true, Ordering::Relaxed);
        }
        "notifications/prompts/list_changed" => {
            shared.stale_prompts.store(true, Ordering::Relaxed);
        }
        "notifications/resources/updated" => {
            let uri = params["uri"].as_str().unwrap_or("").to_string();
            let subscribed = shared
                .subs
                .lock()
                .map(|g| g.iter().any(|u| *u == uri))
                .unwrap_or(false);
            if uri.is_empty() || !subscribed {
                return;
            }
            // re-read in the background, surface the fresh content as a note
            let shared = shared.clone();
            let pending = pending.clone();
            tokio::spawn(async move {
                let text = match request(
                    &shared,
                    &pending,
                    CALL_TIMEOUT,
                    "resources/read",
                    json!({"uri": uri}),
                )
                .await
                {
                    Ok(v) => render_resource_contents(&v),
                    Err(e) => format!("(read failed: {e:#})"),
                };
                if let Some(tx) = &shared.hooks.notes {
                    let _ = tx.send(crate::provider::ApiEvent::Note(format!(
                        "mcp {}: resource {uri} updated:\n{}",
                        shared.name,
                        crate::provider::truncate(&text).trim_end()
                    )));
                }
            });
        }
        "notifications/message" => {
            if !shared.logging_on {
                return;
            }
            let level = params["level"].as_str().unwrap_or("info").to_string();
            let logger = params["logger"].as_str().unwrap_or("").to_string();
            let data = match params["data"].as_str() {
                Some(s) => s.to_string(),
                None => params["data"].to_string(),
            };
            {
                let mut g = shared
                    .hooks
                    .logs
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                g.push_back(McpLogEntry {
                    server: shared.name.clone(),
                    level: level.clone(),
                    logger: logger.clone(),
                    data: data.clone(),
                });
                while g.len() > MAX_LOGS {
                    g.pop_front();
                }
            }
            // warning and above also pop into the chat
            if log_level_rank(&level).unwrap_or(1) >= 3 {
                if let Some(tx) = &shared.hooks.notes {
                    let head = if logger.is_empty() {
                        String::new()
                    } else {
                        format!("{logger}: ")
                    };
                    let _ = tx.send(crate::provider::ApiEvent::Note(format!(
                        "mcp {} [{}]: {}{}",
                        shared.name,
                        level,
                        head,
                        crate::provider::truncate(&data).trim_end()
                    )));
                }
            }
        }
        _ => {}
    }
}

/// answer a server -> client request: roots/list from the hooks, sampling by
/// calling the sampler; anything else is method-not-found
async fn handle_server_request(shared: &Arc<Shared>, id: Value, method: &str, params: Value) {
    type R = std::result::Result<Value, (i64, String)>;
    let outcome: R = match method {
        "roots/list" => {
            let dirs: Vec<String> = shared
                .hooks
                .roots
                .read()
                .map(|g| g.clone())
                .unwrap_or_default();
            Ok(json!({
                "roots": dirs
                    .iter()
                    .map(|d| json!({"uri": file_uri(d), "name": root_name(d)}))
                    .collect::<Vec<_>>()
            }))
        }
        "sampling/createMessage" => {
            if !shared.sampling {
                Err((-32601, "sampling is disabled for this server".into()))
            } else {
                match &shared.hooks.sampler {
                    None => Err((-32601, "sampling is not supported by this client".into())),
                    Some(sampler) => {
                        let msgs: Vec<(String, String)> = params["messages"]
                            .as_array()
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|m| {
                                        let role = m["role"].as_str()?.to_string();
                                        let text = if m["content"]["type"].as_str() == Some("text") {
                                            m["content"]["text"].as_str()?.to_string()
                                        } else {
                                            String::new()
                                        };
                                        if text.is_empty() {
                                            None
                                        } else {
                                            Some((role, text))
                                        }
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        if msgs.is_empty() {
                            Err((-32602, "sampling request has no text messages".into()))
                        } else {
                            let req = SampleReq {
                                server: shared.name.clone(),
                                system: params["systemPrompt"].as_str().map(|s| s.to_string()),
                                messages: msgs,
                                max_tokens: params["maxTokens"]
                                    .as_u64()
                                    .unwrap_or(512)
                                    .clamp(1, MAX_SAMPLING_TOKENS as u64) as u32,
                            };
                            match tokio::time::timeout(CALL_TIMEOUT, sampler(req)).await {
                                Ok(Ok(out)) => Ok(json!({
                                    "role": "assistant",
                                    "model": out.model,
                                    "content": {"type": "text", "text": out.text}
                                })),
                                Ok(Err(e)) => Err((-32000, format!("{e:#}"))),
                                Err(_) => Err((-32000, "sampling timed out".into())),
                            }
                        }
                    }
                }
            }
        }
        _ => Err((
            -32601,
            format!("method '{method}' is not supported by hi-derola"),
        )),
    };
    let msg = match outcome {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err((code, message)) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": code, "message": message}
        }),
    };
    reply_message(shared, msg).await;
}

async fn reply_message(shared: &Arc<Shared>, msg: Value) {
    match &shared.reply {
        Reply::Stdio { stdin } => {
            let mut w = stdin.lock().await;
            let mut line = msg.to_string();
            line.push('\n');
            let _ = w.write_all(line.as_bytes()).await;
            let _ = w.flush().await;
        }
        Reply::Http(ctx) => {
            let _ = http_send(ctx, &shared.name, &msg, false).await;
        }
    }
}

/// background reader for stdio servers: routes replies to pending waiters,
/// answers server -> client requests, tracks notifications
async fn stdio_reader(
    mut reader: tokio::io::BufReader<ChildStdout>,
    shared: Arc<Shared>,
    pending: Pending,
) {
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        dispatch_incoming(&shared, &pending, v).await;
    }
    shared.closed.store(true, Ordering::Relaxed);
    let mut p = pending.lock().await;
    let ids: Vec<u64> = p.keys().copied().collect();
    for id in ids {
        if let Some(tx) = p.remove(&id) {
            let _ = tx.send(Err(anyhow::anyhow!("mcp {}: server closed", shared.name)));
        }
    }
}

/// best-effort standalone sse stream (streamable http): carries live
/// notifications and server -> client requests; exits when the server
/// answers 405 (no stream support)
async fn http_live(shared: Arc<Shared>, pending: Pending) {
    let Reply::Http(ctx) = &shared.reply else {
        return;
    };
    let mut backoff = 1u64;
    loop {
        if shared.closed.load(Ordering::Relaxed) {
            return;
        }
        match http_get_stream(ctx, &shared.name).await {
            Ok(r) if r.status().as_u16() == 405 => return,
            Ok(r) if r.status().is_success() => {
                backoff = 1;
                let _ = route_sse(r, &shared, &pending, None).await;
            }
            _ => {}
        }
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(30);
    }
}

async fn http_get_stream(ctx: &HttpCtx, name: &str) -> Result<reqwest::Response> {
    let token = crate::mcpauth::bearer(name, &ctx.url, ctx.oauth.as_ref(), &ctx.http, false)
        .await
        .unwrap_or(None);
    let mut req = ctx
        .http
        .get(ctx.url.as_str())
        .header("Accept", "text/event-stream");
    if let Some(t) = &token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    if let Some(sid) = ctx.session.lock().unwrap().as_ref() {
        req = req.header("mcp-session-id", sid);
    }
    for (k, v) in ctx.headers.iter() {
        req = req.header(k.as_str(), v.as_str());
    }
    Ok(req.send().await?)
}

/// consume an sse stream, routing every json event through pending/dispatch.
/// with want_id: resolve when the matching response arrives (request path);
/// without: consume until the stream ends (live GET stream)
async fn route_sse(
    mut resp: reqwest::Response,
    shared: &Arc<Shared>,
    pending: &Pending,
    want_id: Option<u64>,
) -> Result<Value> {
    let mut buf = String::new();
    loop {
        let Some(bytes) = resp.chunk().await? else {
            break;
        };
        buf.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(pos) = buf.find('\n') {
            let line: String = buf.drain(..pos + 1).collect();
            let Some(data) = line.trim_end().strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            let mid = v.get("id").and_then(|x| x.as_u64());
            if v.get("method").is_none() && want_id.is_some() && mid == want_id {
                return extract_result(v, &shared.name);
            }
            dispatch_incoming(shared, pending, v).await;
        }
    }
    match want_id {
        Some(_) => bail!("mcp {}: no response in sse stream", shared.name),
        None => Ok(json!({})),
    }
}

/// POST one jsonrpc message with auth/session headers
async fn http_send(
    ctx: &HttpCtx,
    name: &str,
    body: &Value,
    refresh_auth: bool,
) -> Result<reqwest::Response> {
    let token = crate::mcpauth::bearer(name, &ctx.url, ctx.oauth.as_ref(), &ctx.http, refresh_auth)
        .await?;
    let mut req = ctx
        .http
        .post(ctx.url.as_str())
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream");
    if let Some(t) = &token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    if let Some(sid) = ctx.session.lock().unwrap().as_ref() {
        req = req.header("mcp-session-id", sid);
    }
    for (k, v) in ctx.headers.iter() {
        req = req.header(k.as_str(), v.as_str());
    }
    let fut = req.body(body.to_string()).send();
    match tokio::time::timeout(REQUEST_TIMEOUT, fut).await {
        Err(_) => bail!("mcp {name}: request timeout"),
        Ok(r) => Ok(r?),
    }
}

/// send a request or notification; for http requests the reply is the
/// return value, for stdio it arrives via pending
async fn send_msg(shared: &Arc<Shared>, pending: &Pending, msg: &Value) -> Result<Value> {
    match &shared.reply {
        Reply::Stdio { stdin } => {
            let mut w = stdin.lock().await;
            let mut line = msg.to_string();
            line.push('\n');
            w.write_all(line.as_bytes()).await?;
            w.flush().await?;
            Ok(json!({}))
        }
        Reply::Http(ctx) => http_post(shared, ctx, pending, msg).await,
    }
}

async fn http_post(
    shared: &Arc<Shared>,
    ctx: &HttpCtx,
    pending: &Pending,
    body: &Value,
) -> Result<Value> {
    let name = &shared.name;
    let Some(id) = body.get("id").and_then(|x| x.as_u64()) else {
        // notification: best effort
        let _ = http_send(ctx, name, body, false).await;
        return Ok(json!({}));
    };
    let method = body["method"].as_str().unwrap_or("").to_string();
    let mut resp = None;
    for attempt in 0..2 {
        let r = http_send(ctx, name, body, attempt > 0).await?;
        if r.status().as_u16() == 401 && attempt == 0 {
            continue;
        }
        if r.status().as_u16() == 401 {
            let hint = if ctx.oauth.is_some() {
                format!(" — run /mcpauth {name}")
            } else {
                String::new()
            };
            bail!("mcp {name}: 401 unauthorized{hint}");
        }
        resp = Some(r);
        break;
    }
    let resp = resp.context("mcp: no response")?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        bail!("mcp {name}: {} {}", status, crate::provider::truncate(&text).trim());
    }
    if method == "initialize" {
        if let Some(sid) = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            *ctx.session.lock().unwrap() = Some(sid.to_string());
        }
    }
    let ctype = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    if ctype.contains("text/event-stream") {
        route_sse(resp, shared, pending, Some(id)).await
    } else {
        let text = resp.text().await?;
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("mcp {name}: bad json: {e}"))?;
        extract_result(v, name)
    }
}

/// one request/response round trip; shared between McpServer and the
/// notification-driven re-reads
async fn request(
    shared: &Arc<Shared>,
    pending: &Pending,
    timeout: Duration,
    method: &str,
    params: Value,
) -> Result<Value> {
    let id = shared.next_id.fetch_add(1, Ordering::Relaxed) + 1;
    let (tx, rx) = tokio::sync::oneshot::channel();
    pending.lock().await.insert(id, tx);
    let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    let sent = send_msg(shared, pending, &msg).await;
    let res = match sent {
        Err(e) => Err(e),
        // http replies come back inline; stdio replies arrive via pending
        Ok(v) if matches!(shared.reply, Reply::Http(_)) => Ok(v),
        Ok(_) => match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => bail!("mcp {}: server closed", shared.name),
            Err(_) => bail!("mcp {}: {method} timeout", shared.name),
        },
    };
    pending.lock().await.remove(&id);
    res
}

impl McpServer {
    async fn request_t(&self, timeout: Duration, method: &str, params: Value) -> Result<Value> {
        request(&self.shared, &self.pending, timeout, method, params).await
    }

    async fn notify_t(&self, method: &str) -> Result<()> {
        let msg = json!({"jsonrpc": "2.0", "method": method});
        send_msg(&self.shared, &self.pending, &msg).await.map(|_| ())
    }

    /// subscribe to resource updates; the uri registers before the request so
    /// an update pushed right after the answer is not missed
    async fn subscribe(&self, uri: &str) -> Result<()> {
        if !self.shared.res_sub.load(Ordering::Relaxed) {
            bail!(
                "mcp {}: server does not support resource subscriptions",
                self.shared.name
            );
        }
        if let Ok(mut g) = self.shared.subs.lock() {
            if !g.iter().any(|u| u == uri) {
                g.push(uri.to_string());
            }
        }
        match self
            .request_t(REQUEST_TIMEOUT, "resources/subscribe", json!({"uri": uri}))
            .await
        {
            Ok(_) => Ok(()),
            Err(e) => {
                if let Ok(mut g) = self.shared.subs.lock() {
                    g.retain(|u| u != uri);
                }
                Err(e)
            }
        }
    }

    /// best-effort: local state wins, a failed unsubscribe request is tolerated
    async fn unsubscribe(&self, uri: &str) -> Result<()> {
        if let Ok(mut g) = self.shared.subs.lock() {
            g.retain(|u| u != uri);
        }
        let _ = self
            .request_t(REQUEST_TIMEOUT, "resources/unsubscribe", json!({"uri": uri}))
            .await;
        Ok(())
    }

    async fn connect(cfg: &McpConfig, hooks: &McpHooks) -> Result<Self> {
        let remote = cfg.r#type.as_deref() == Some("remote")
            || (cfg.command.is_empty() && cfg.url.is_some());
        let sampling = hooks.sampler.is_some() && cfg.sampling != Some(false);
        let (shared, pending, child, reader) = if remote {
            let url = cfg.url.clone().context("mcp: url required")?;
            let http = reqwest::Client::builder().user_agent("hi-derola").build()?;
            (
                Arc::new(Shared {
                    name: cfg.name.clone(),
                    reply: Reply::Http(Arc::new(HttpCtx {
                        url,
                        http,
                        headers: cfg.headers.clone(),
                        oauth: cfg.oauth_cfg(),
                        session: std::sync::Mutex::new(None),
                    })),
                    hooks: hooks.clone(),
                    sampling,
                    next_id: AtomicU64::new(0),
                    stale_tools: AtomicBool::new(false),
                    stale_resources: AtomicBool::new(false),
                    stale_prompts: AtomicBool::new(false),
                    closed: AtomicBool::new(false),
                    res_sub: AtomicBool::new(false),
                    subs: std::sync::Mutex::new(Vec::new()),
                    logging: AtomicBool::new(false),
                    logging_on: cfg.logging != Some(false),
                }),
                Arc::new(Mutex::new(BTreeMap::new())),
                None,
                None,
            )
        } else {
            let mut cmd = tokio::process::Command::new(&cfg.command);
            cmd.args(&cfg.args).envs(&cfg.env);
            #[cfg(windows)]
            {
                cmd.creation_flags(0x0800_0000);
            }
            let mut child = cmd
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()
                .with_context(|| format!("mcp {}: spawn {}", cfg.name, cfg.command))?;
            let stdin = child.stdin.take().context("mcp: no stdin")?;
            let stdout = child.stdout.take().context("mcp: no stdout")?;
            (
                Arc::new(Shared {
                    name: cfg.name.clone(),
                    reply: Reply::Stdio {
                        stdin: Arc::new(Mutex::new(stdin)),
                    },
                    hooks: hooks.clone(),
                    sampling,
                    next_id: AtomicU64::new(0),
                    stale_tools: AtomicBool::new(false),
                    stale_resources: AtomicBool::new(false),
                    stale_prompts: AtomicBool::new(false),
                    closed: AtomicBool::new(false),
                    res_sub: AtomicBool::new(false),
                    subs: std::sync::Mutex::new(Vec::new()),
                    logging: AtomicBool::new(false),
                    logging_on: cfg.logging != Some(false),
                }),
                Arc::new(Mutex::new(BTreeMap::new())),
                Some(child),
                Some(tokio::io::BufReader::new(stdout)),
            )
        };
        let mut s = Self {
            shared,
            pending,
            child,
            task: None,
            tools: Vec::new(),
            resources: Vec::new(),
            prompts: Vec::new(),
        };
        // background traffic: stdio reader or the http live stream
        if let Some(reader) = reader {
            s.task = Some(tokio::spawn(stdio_reader(
                reader,
                s.shared.clone(),
                s.pending.clone(),
            )));
        } else {
            s.task = Some(tokio::spawn(http_live(
                s.shared.clone(),
                s.pending.clone(),
            )));
        }
        let proto = match s.shared.reply {
            Reply::Http(_) => "2025-03-26",
            Reply::Stdio { .. } => "2024-11-05",
        };
        let mut client_caps = json!({"roots": {"listChanged": true}});
        if sampling {
            client_caps["sampling"] = json!({});
        }
        let init = s
            .request_t(
                REQUEST_TIMEOUT,
                "initialize",
                json!({
                    "protocolVersion": proto,
                    "capabilities": client_caps,
                    "clientInfo": {"name": "hi-derola", "version": "0.1.0"}
                }),
            )
            .await?;
        let caps = init["capabilities"].clone();
        s.shared.res_sub.store(
            caps["resources"]["subscribe"].is_object()
                || caps["resources"]["subscribe"].as_bool() == Some(true),
            Ordering::Relaxed,
        );
        s.shared
            .logging
            .store(has_cap(&caps, "logging"), Ordering::Relaxed);
        s.notify_t("notifications/initialized").await?;
        // tools: strict when the server declares the capability (keeps the oauth 401
        // hints), tolerated otherwise so tools-less servers still connect
        let tools_cap = has_cap(&caps, "tools");
        match s.list_page("tools/list", "tools").await {
            Ok(items) => s.tools = parse_tools(&items),
            Err(e) => {
                if tools_cap {
                    return Err(e);
                }
            }
        }
        // resources and prompts never fail the connect: older servers may
        // answer with method-not-found even after advertising the capability
        if has_cap(&caps, "resources") {
            if let Ok(items) = s.list_page("resources/list", "resources").await {
                s.resources = parse_resources(&items);
            }
        }
        if has_cap(&caps, "prompts") {
            if let Ok(items) = s.list_page("prompts/list", "prompts").await {
                s.prompts = parse_prompts(&items);
            }
        }
        Ok(s)
    }
}

pub struct McpClient {
    servers: Mutex<Vec<McpServer>>,
    hooks: McpHooks,
}

pub type McpSlot = std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<McpClient>>>>;

pub async fn connect_all(
    cfgs: &[McpConfig],
    hooks: &McpHooks,
) -> (Option<std::sync::Arc<McpClient>>, Vec<String>) {
    let mut servers = Vec::new();
    let mut logs = Vec::new();
    for c in cfgs {
        match tokio::time::timeout(CONNECT_TIMEOUT, McpServer::connect(c, hooks)).await {
            Ok(Ok(s)) => {
                logs.push(format!("mcp {}: connected ({})", c.name, s.summary()));
                servers.push(s);
            }
            Ok(Err(e)) => logs.push(format!("mcp {}: {e:#}", c.name)),
            Err(_) => logs.push(format!("mcp {}: connect timeout", c.name)),
        }
    }
    let client = if servers.is_empty() {
        None
    } else {
        Some(std::sync::Arc::new(McpClient {
            servers: Mutex::new(servers),
            hooks: hooks.clone(),
        }))
    };
    (client, logs)
}

pub async fn reconnect_one(
    slot: &McpSlot,
    cfgs: &[McpConfig],
    hooks: &McpHooks,
    name: &str,
) -> Vec<String> {
    let Some(cfg) = cfgs.iter().find(|c| c.name == name) else {
        return vec![format!("mcp {name}: not in config")];
    };
    let existing = slot.lock().unwrap().clone();
    if let Some(client) = existing {
        let mut logs = Vec::new();
        match tokio::time::timeout(CONNECT_TIMEOUT, client.replace(cfg, hooks)).await {
            Ok(Ok(sum)) => logs.push(format!("mcp {name}: connected ({sum})")),
            Ok(Err(e)) => logs.push(format!("mcp {name}: {e:#}")),
            Err(_) => logs.push(format!("mcp {name}: connect timeout")),
        }
        logs
    } else {
        let (client, logs) = connect_all(cfgs, hooks).await;
        *slot.lock().unwrap() = client;
        logs
    }
}

impl McpClient {
    async fn replace(&self, cfg: &McpConfig, hooks: &McpHooks) -> Result<String> {
        let mut servers = self.servers.lock().await;
        servers.retain(|s| s.shared.name != cfg.name);
        let s = McpServer::connect(cfg, hooks).await?;
        let sum = s.summary();
        servers.push(s);
        Ok(sum)
    }

    pub async fn specs(&self) -> Vec<ToolSpec> {
        let mut servers = self.servers.lock().await;
        let mut out = Vec::new();
        for s in servers.iter_mut() {
            s.refresh_tools().await;
            for t in &s.tools {
                out.push(ToolSpec {
                    name: format!("mcp__{}__{}", s.shared.name, t.name),
                    description: t.description.clone(),
                    parameters: t.schema.clone(),
                });
            }
        }
        out
    }

    pub async fn call(&self, server_tool: &str, args: &str) -> Result<String> {
        let Some((server, tool)) = server_tool.split_once("__") else {
            bail!("bad mcp tool name: {server_tool}");
        };
        let mut servers = self.servers.lock().await;
        let Some(s) = servers.iter_mut().find(|s| s.shared.name == server) else {
            bail!("mcp server not found: {server}");
        };
        let arguments: Value = serde_json::from_str(args).unwrap_or(json!({}));
        let res = s
            .request_t(
                CALL_TIMEOUT,
                "tools/call",
                json!({"name": tool, "arguments": arguments}),
            )
            .await?;
        let mut text = String::new();
        for b in res["content"].as_array().into_iter().flatten() {
            if b["type"].as_str() == Some("text") {
                if let Some(t) = b["text"].as_str() {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(t);
                }
            }
        }
        if res["isError"].as_bool().unwrap_or(false) {
            bail!("{}", if text.is_empty() { "tool error" } else { &text });
        }
        if text.is_empty() {
            text = "(empty result)".into();
        }
        Ok(text)
    }

    /// all resources across servers, in connect order
    pub async fn resources(&self) -> Vec<McpResourceInfo> {
        let mut servers = self.servers.lock().await;
        let mut out = Vec::new();
        for s in servers.iter_mut() {
            s.refresh_resources().await;
            for r in &s.resources {
                out.push(McpResourceInfo {
                    server: s.shared.name.clone(),
                    uri: r.uri.clone(),
                    name: r.name.clone(),
                    description: r.description.clone(),
                    mime: r.mime.clone(),
                });
            }
        }
        out
    }

    /// read one resource; empty server name auto-resolves when the uri is unique
    pub async fn read_resource(&self, server: &str, uri: &str) -> Result<String> {
        let mut servers = self.servers.lock().await;
        if server.is_empty() {
            let owners: Vec<&mut McpServer> = servers
                .iter_mut()
                .filter(|s| s.resources.iter().any(|r| r.uri == uri))
                .collect();
            return match owners.len() {
                0 => bail!("mcp resource not found: {uri} (run /mcpres to list)"),
                1 => owners.into_iter().next().unwrap().read_resource(uri).await,
                _ => bail!("uri is exposed by multiple mcp servers, pass the server name"),
            };
        }
        let Some(s) = servers.iter_mut().find(|s| s.shared.name == server) else {
            bail!("mcp server not found: {server}");
        };
        s.read_resource(uri).await
    }

    /// all prompts across servers, in connect order
    pub async fn prompts(&self) -> Vec<McpPromptInfo> {
        let mut servers = self.servers.lock().await;
        let mut out = Vec::new();
        for s in servers.iter_mut() {
            s.refresh_prompts().await;
            for p in &s.prompts {
                out.push(McpPromptInfo {
                    server: s.shared.name.clone(),
                    name: p.name.clone(),
                    description: p.description.clone(),
                    arguments: p
                        .arguments
                        .iter()
                        .map(|a| McpPromptArgInfo {
                            name: a.name.clone(),
                            description: a.description.clone(),
                            required: a.required,
                        })
                        .collect(),
                });
            }
        }
        out
    }

    /// fetch prompt messages as (role, text) pairs
    pub async fn get_prompt(
        &self,
        server: &str,
        name: &str,
        args: &Value,
    ) -> Result<Vec<(String, String)>> {
        let mut servers = self.servers.lock().await;
        let Some(s) = servers.iter_mut().find(|s| s.shared.name == server) else {
            bail!("mcp server not found: {server}");
        };
        s.get_prompt(name, args).await
    }

    /// update the exposed roots and tell every live server
    pub async fn set_roots(&self, dirs: Vec<String>) {
        if let Ok(mut g) = self.hooks.roots.write() {
            *g = dirs;
        }
        let servers = self.servers.lock().await;
        for s in servers.iter() {
            let _ = s.notify_t("notifications/roots/list_changed").await;
        }
    }

    /// subscribe to a resource so updates arrive as chat notes
    pub async fn subscribe(&self, server: &str, uri: &str) -> Result<()> {
        let servers = self.servers.lock().await;
        let Some(s) = servers.iter().find(|s| s.shared.name == server) else {
            bail!("mcp server not found: {server}");
        };
        s.subscribe(uri).await
    }

    /// stop the subscription; best-effort
    pub async fn unsubscribe(&self, server: &str, uri: &str) -> Result<()> {
        let servers = self.servers.lock().await;
        let Some(s) = servers.iter().find(|s| s.shared.name == server) else {
            bail!("mcp server not found: {server}");
        };
        s.unsubscribe(uri).await
    }

    /// (server, uri) pairs currently subscribed
    pub async fn subscriptions(&self) -> Vec<(String, String)> {
        let servers = self.servers.lock().await;
        let mut out = Vec::new();
        for s in servers.iter() {
            if let Ok(g) = s.shared.subs.lock() {
                for u in g.iter() {
                    out.push((s.shared.name.clone(), u.clone()));
                }
            }
        }
        out
    }

    /// set the minimum log level (logging/setLevel); empty server name applies
    /// to every logging-capable server; returns one status line per server
    pub async fn set_log_level(&self, server: &str, level: &str) -> Vec<String> {
        if log_level_rank(level).is_none() {
            return vec![format!(
                "unknown log level: {level} (debug, info, notice, warning, error, critical, alert, emergency)"
            )];
        }
        let servers = self.servers.lock().await;
        let mut out = Vec::new();
        let mut matched = false;
        for s in servers.iter() {
            if !server.is_empty() && s.shared.name != server {
                continue;
            }
            matched = true;
            if !s.shared.logging.load(Ordering::Relaxed) {
                out.push(format!(
                    "mcp {}: server does not support logging",
                    s.shared.name
                ));
                continue;
            }
            match request(
                &s.shared,
                &s.pending,
                REQUEST_TIMEOUT,
                "logging/setLevel",
                json!({"level": level}),
            )
            .await
            {
                Ok(_) => out.push(format!("mcp {}: log level set to {level}", s.shared.name)),
                Err(e) => out.push(format!("mcp {}: {e:#}", s.shared.name)),
            }
        }
        if !matched {
            out.push(format!("mcp server not found: {server}"));
        }
        out
    }

    /// recent notifications/message entries, oldest first
    pub fn logs(&self) -> Vec<McpLogEntry> {
        self.hooks
            .logs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }
}

pub struct McpResourceInfo {
    pub server: String,
    pub uri: String,
    pub name: String,
    pub description: String,
    pub mime: Option<String>,
}

pub struct McpPromptArgInfo {
    pub name: String,
    pub description: String,
    pub required: bool,
}

pub struct McpPromptInfo {
    pub server: String,
    pub name: String,
    pub description: String,
    pub arguments: Vec<McpPromptArgInfo>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_helpers() {
        let tools = parse_tools(&[
            json!({"name": "ping", "description": "d", "inputSchema": {"type": "object"}}),
            json!({"description": "no name"}),
        ]);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "ping");

        let res = parse_resources(&[
            json!({"uri": "file:///a", "name": "a", "description": "d", "mimeType": "text/plain"}),
            json!({"name": "no uri"}),
        ]);
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].uri, "file:///a");
        assert_eq!(res[0].mime.as_deref(), Some("text/plain"));

        let prm = parse_prompts(&[json!({
            "name": "review", "description": "review code",
            "arguments": [{"name": "lang", "description": "language", "required": true}]
        })]);
        assert_eq!(prm.len(), 1);
        assert_eq!(prm[0].arguments[0].required, true);

        let contents = render_resource_contents(&json!({"contents": [
            {"uri": "u1", "text": "one"},
            {"uri": "u2", "blob": "aGVsbG8="}
        ]}));
        assert_eq!(contents, "one\n\n[binary resource u2, ~6 bytes]");
        assert_eq!(render_resource_contents(&json!({"contents": []})), "(empty resource)");

        let msgs = parse_prompt_messages(&json!({"messages": [
            {"role": "user", "content": {"type": "text", "text": "hi"}},
            {"role": "assistant", "content": {"type": "image"}}
        ]}));
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0], ("user".to_string(), "hi".to_string()));
    }

    #[test]
    fn capability_detection() {
        assert!(has_cap(&json!({"resources": {}}), "resources"));
        assert!(has_cap(&json!({"resources": {"subscribe": true}}), "resources"));
        assert!(has_cap(&json!({"prompts": true}), "prompts"));
        assert!(!has_cap(&json!({}), "tools"));
        assert!(!has_cap(&json!({"tools": false}), "tools"));
    }

    #[test]
    fn roots_helpers() {
        assert_eq!(file_uri("/tmp/ws"), "file:///tmp/ws");
        assert_eq!(file_uri("C:\\Users\\me"), "file:///C:/Users/me");
        assert_eq!(root_name("/tmp/ws"), "ws");
        assert_eq!(root_name("C:\\Users\\me\\"), "me");
        assert_eq!(root_name("/"), "/");
        let hooks = McpHooks::workspace(Some(std::path::PathBuf::from("/tmp/ws")));
        assert_eq!(hooks.roots.read().unwrap().len(), 1);
    }

    /// spawned as a child process by the connect tests; acts as a fake stdio
    /// mcp server. plain test run: no-op.
    #[test]
    fn fake_mcp_child() {
        if std::env::var("HI_DEROLA_FAKE_MCP").is_err() {
            return;
        }
        let mode = std::env::var("HI_DEROLA_FAKE_MCP").unwrap_or_default();
        use std::io::{BufRead, Write};
        let stdin = std::io::stdin();
        let mut out = std::io::stdout().lock();
        let mut caps_seen = Value::Null;
        let mut roots_reply = Value::Null;
        let mut sampling_reply = Value::Null;
        let mut sampling_error = false;
        let mut tools_listed = 0u32;
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let method = v["method"].as_str().unwrap_or("").to_string();
            if method == "notifications/initialized" {
                if mode != "min" {
                    // server -> client requests the client must answer
                    let reqs = [
                        json!({"jsonrpc": "2.0", "id": 501, "method": "roots/list", "params": {}}),
                        json!({"jsonrpc": "2.0", "id": 502, "method": "sampling/createMessage", "params": {
                            "messages": [{"role": "user", "content": {"type": "text", "text": "say hi"}}],
                            "maxTokens": 64
                        }}),
                        json!({"jsonrpc": "2.0", "method": "notifications/message",
                            "params": {"level": "error", "logger": "db", "data": "boom"}}),
                    ];
                    for r in reqs {
                        writeln!(out, "{r}").unwrap();
                    }
                    out.flush().unwrap();
                }
                continue;
            }
            if method.starts_with("notifications/") {
                continue;
            }
            let Some(id) = v.get("id").and_then(|x| x.as_u64()) else {
                continue;
            };
            // replies to the server-initiated requests above
            if id == 501 || id == 502 {
                if v.get("error").is_some() {
                    sampling_reply = v["error"].clone();
                    sampling_error = true;
                } else if id == 501 {
                    roots_reply = v["result"].clone();
                } else {
                    sampling_reply = v["result"].clone();
                }
                continue;
            }
            let result = match method.as_str() {
                "initialize" => {
                    caps_seen = v["params"]["capabilities"].clone();
                    if mode == "min" {
                        json!({"capabilities": {}})
                    } else {
                        json!({"capabilities": {"tools": {}, "resources": {"subscribe": true}, "prompts": {}, "logging": {}}})
                    }
                }
                "tools/list" => {
                    if mode == "min" {
                        // answered as an error: tolerated because no tools cap
                        let e = json!({"code": -32601, "message": "method not found"});
                        let resp = json!({"jsonrpc": "2.0", "id": id, "error": e});
                        writeln!(out, "{resp}").unwrap();
                        out.flush().unwrap();
                        continue;
                    }
                    tools_listed += 1;
                    if tools_listed == 1 {
                        json!({"tools": [{"name": "ping", "description": "d", "inputSchema": {"type": "object"}}]})
                    } else {
                        json!({"tools": [
                            {"name": "ping", "description": "d", "inputSchema": {"type": "object"}},
                            {"name": "ping2", "description": "added", "inputSchema": {"type": "object"}}
                        ]})
                    }
                }
                "tools/call" => {
                    // live refresh trigger: notify before answering
                    let n = json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"});
                    writeln!(out, "{n}").unwrap();
                    out.flush().unwrap();
                    json!({"content": [{"type": "text", "text": json!({
                        "caps": caps_seen,
                        "roots": roots_reply,
                        "sampling": sampling_reply,
                        "sampling_error": sampling_error,
                    }).to_string()}]})
                }
                "resources/list" => {
                    if v["params"]["cursor"].as_str().is_none() {
                        json!({"resources": [{"uri": "file:///a.txt", "name": "a", "description": "file a", "mimeType": "text/plain"}], "nextCursor": "p2"})
                    } else {
                        json!({"resources": [{"uri": "mem://stats", "name": "stats", "description": "live stats"}]})
                    }
                }
                "resources/read" => json!({"contents": [
                    {"uri": v["params"]["uri"], "mimeType": "text/plain", "text": "hello resource"}
                ]}),
                "prompts/list" => json!({"prompts": [{"name": "review", "description": "review code",
                    "arguments": [{"name": "lang", "description": "language", "required": true}]}]}),
                "prompts/get" => json!({"messages": [
                    {"role": "user", "content": {"type": "text", "text": "review the code in lang"}}
                ]}),
                "resources/subscribe" => {
                    // update pushed before the answer: the client must not miss it
                    let n = json!({"jsonrpc": "2.0", "method": "notifications/resources/updated",
                        "params": {"uri": v["params"]["uri"]}});
                    writeln!(out, "{n}").unwrap();
                    out.flush().unwrap();
                    json!({})
                }
                "resources/unsubscribe" => json!({}),
                "logging/setLevel" => json!({}),
                _ => json!({}),
            };
            let resp = json!({"jsonrpc": "2.0", "id": id, "result": result});
            writeln!(out, "{resp}").unwrap();
            out.flush().unwrap();
        }
    }

    fn child_cfg(mode: &str) -> McpConfig {
        child_cfg_opts(mode, None, None)
    }

    fn child_cfg_sampling(mode: &str, sampling: Option<bool>) -> McpConfig {
        child_cfg_opts(mode, sampling, None)
    }

    fn child_cfg_opts(mode: &str, sampling: Option<bool>, logging: Option<bool>) -> McpConfig {
        let exe = std::env::current_exe().unwrap();
        let mut env = BTreeMap::new();
        env.insert("HI_DEROLA_FAKE_MCP".to_string(), mode.to_string());
        McpConfig {
            name: "t".to_string(),
            r#type: None,
            command: exe.display().to_string(),
            args: vec![
                "mcp::tests::fake_mcp_child".to_string(),
                "--exact".to_string(),
                "--nocapture".to_string(),
            ],
            env,
            url: None,
            headers: BTreeMap::new(),
            oauth: None,
            sampling,
            logging,
        }
    }

    fn sampler_hooks() -> McpHooks {
        McpHooks::workspace(Some(std::path::PathBuf::from("/tmp/ws"))).with_sampler(Arc::new(
            |req: SampleReq| {
                Box::pin(async move {
                    Ok(SampleOut {
                        model: "test-model".into(),
                        text: format!("sampled:{}", req.messages[0].1),
                    })
                })
            },
        ))
    }

    #[tokio::test]
    async fn resources_and_prompts_roundtrip() {
        let cfg = child_cfg("1");
        let s = McpServer::connect(&cfg, &McpHooks::default()).await.unwrap();
        assert_eq!(s.tools.len(), 1);
        assert_eq!(s.resources.len(), 2, "pagination follows nextCursor");
        assert_eq!(s.resources[0].uri, "file:///a.txt");
        assert_eq!(s.resources[1].uri, "mem://stats");
        assert_eq!(s.prompts.len(), 1);
        assert_eq!(s.prompts[0].arguments[0].name, "lang");

        let text = s.read_resource("mem://stats").await.unwrap();
        assert_eq!(text, "hello resource");

        let msgs = s
            .get_prompt("review", &json!({"lang": "rust"}))
            .await
            .unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].0, "user");
        assert_eq!(msgs[0].1, "review the code in lang");

        assert_eq!(s.summary(), "1 tool, 2 resources, 1 prompt");
    }

    #[tokio::test]
    async fn roots_sampling_and_live_refresh() {
        let hooks = sampler_hooks();
        let cfg = child_cfg("1");
        let s = McpServer::connect(&cfg, &hooks).await.unwrap();

        let res = s
            .request_t(
                CALL_TIMEOUT,
                "tools/call",
                json!({"name": "ping", "arguments": {}}),
            )
            .await
            .unwrap();
        let dump: Value =
            serde_json::from_str(res["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(dump["caps"]["roots"]["listChanged"], true, "roots cap declared");
        assert_eq!(dump["caps"]["sampling"], json!({}), "sampling cap declared");
        assert_eq!(dump["roots"]["roots"][0]["uri"], "file:///tmp/ws");
        assert_eq!(dump["roots"]["roots"][0]["name"], "ws");
        assert_eq!(dump["sampling"]["role"], "assistant");
        assert_eq!(dump["sampling"]["model"], "test-model");
        assert_eq!(dump["sampling"]["content"]["text"], "sampled:say hi");
        assert_eq!(dump["sampling_error"], false);

        // the ping reply was preceded by notifications/tools/list_changed:
        // the next specs() must re-list and see the second tool
        let client = McpClient {
            servers: Mutex::new(vec![s]),
            hooks: hooks.clone(),
        };
        assert_eq!(client.specs().await.len(), 2, "tools refreshed after list_changed");
    }

    #[tokio::test]
    async fn sampling_disabled_replies_error() {
        let hooks = sampler_hooks();
        let cfg = child_cfg_sampling("1", Some(false));
        let s = McpServer::connect(&cfg, &hooks).await.unwrap();

        let res = s
            .request_t(
                CALL_TIMEOUT,
                "tools/call",
                json!({"name": "ping", "arguments": {}}),
            )
            .await
            .unwrap();
        let dump: Value =
            serde_json::from_str(res["content"][0]["text"].as_str().unwrap()).unwrap();
        assert!(
            dump["caps"]["sampling"].is_null(),
            "no sampling cap when disabled"
        );
        assert_eq!(dump["sampling_error"], true);
        assert_eq!(dump["sampling"]["code"], -32601);
    }

    #[tokio::test]
    async fn server_without_caps_still_connects() {
        let cfg = child_cfg("min");
        let s = McpServer::connect(&cfg, &McpHooks::default()).await.unwrap();
        assert!(s.tools.is_empty(), "tools/list error tolerated without cap");
        assert!(s.resources.is_empty());
        assert!(s.prompts.is_empty());
        assert_eq!(s.summary(), "0 tools");
        let err = s
            .subscribe("mem://x")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("does not support resource subscriptions"), "{err}");
    }

    #[tokio::test]
    async fn resource_subscriptions_push_updates() {
        let (ntx, mut nrx) = tokio::sync::mpsc::unbounded_channel();
        let hooks = McpHooks::workspace(Some(std::path::PathBuf::from("/tmp/ws"))).with_notes(ntx);
        let cfg = child_cfg("1");
        let s = McpServer::connect(&cfg, &hooks).await.unwrap();
        let client = McpClient {
            servers: Mutex::new(vec![s]),
            hooks: hooks.clone(),
        };

        client.subscribe("t", "mem://stats").await.unwrap();
        assert_eq!(
            client.subscriptions().await,
            vec![("t".to_string(), "mem://stats".to_string())]
        );

        // the fake server pushed notifications/resources/updated right after the
        // subscribe answer: the client re-reads and notes the fresh content
        let mut saw_update = false;
        for _ in 0..10 {
            match tokio::time::timeout(Duration::from_secs(2), nrx.recv()).await {
                Ok(Some(crate::provider::ApiEvent::Note(n))) => {
                    if n.contains("resource mem://stats updated")
                        && n.contains("hello resource")
                    {
                        saw_update = true;
                        break;
                    }
                }
                _ => break,
            }
        }
        assert!(saw_update, "resource update note missing");

        client.unsubscribe("t", "mem://stats").await.unwrap();
        assert!(client.subscriptions().await.is_empty());
    }

    #[tokio::test]
    async fn log_messages_buffer_and_note() {
        let (ntx, mut nrx) = tokio::sync::mpsc::unbounded_channel();
        let hooks = McpHooks::workspace(Some(std::path::PathBuf::from("/tmp/ws"))).with_notes(ntx);
        let cfg = child_cfg("1");
        let s = McpServer::connect(&cfg, &hooks).await.unwrap();
        let client = McpClient {
            servers: Mutex::new(vec![s]),
            hooks: hooks.clone(),
        };

        // the initialize-phase notifications/message is buffered, oldest first
        let logs = hooks.logs.lock().unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].server, "t");
        assert_eq!(logs[0].level, "error");
        assert_eq!(logs[0].logger, "db");
        assert_eq!(logs[0].data, "boom");
        drop(logs);
        assert_eq!(client.logs().len(), 1);

        // error rank also surfaces as a chat note
        let mut saw_log_note = false;
        for _ in 0..10 {
            match tokio::time::timeout(Duration::from_secs(2), nrx.recv()).await {
                Ok(Some(crate::provider::ApiEvent::Note(n))) => {
                    if n.contains("mcp t [error]: db: boom") {
                        saw_log_note = true;
                        break;
                    }
                }
                _ => break,
            }
        }
        assert!(saw_log_note, "error log note missing");

        assert_eq!(
            client.set_log_level("t", "debug").await,
            vec!["mcp t: log level set to debug".to_string()]
        );
        let bad = client.set_log_level("t", "nope").await;
        assert!(bad[0].starts_with("unknown log level"), "{}", bad[0]);
    }

    #[tokio::test]
    async fn logging_opt_out_drops_messages() {
        let (ntx, mut nrx) = tokio::sync::mpsc::unbounded_channel();
        let hooks = McpHooks::workspace(Some(std::path::PathBuf::from("/tmp/ws"))).with_notes(ntx);
        let cfg = child_cfg_opts("1", None, Some(false));
        let _s = McpServer::connect(&cfg, &hooks).await.unwrap();
        assert!(
            hooks.logs.lock().unwrap().is_empty(),
            "logging = false drops notifications/message"
        );
        assert!(nrx.try_recv().is_err(), "no notes when logging is off");
    }
}
