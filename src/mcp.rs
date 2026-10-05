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

/// a resource template: a uri scheme with placeholders the caller fills in
/// (resources/templates/list)
struct McpTemplate {
    uri_template: String,
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

/// a tool schema the providers can work with: some servers ship a null or
/// non-object inputSchema (and outputSchema with unresolvable $refs) — coerce
/// the input schema to a valid object schema; outputSchema is ignored here,
/// structured results are handled at call time
fn sane_schema(v: &Value) -> Value {
    if v.is_object() {
        v.clone()
    } else {
        json!({"type": "object", "properties": {}})
    }
}

fn parse_tools(items: &[Value]) -> Vec<McpTool> {
    items
        .iter()
        .map(|t| McpTool {
            name: t["name"].as_str().unwrap_or("").to_string(),
            description: t["description"].as_str().unwrap_or("").to_string(),
            schema: sane_schema(&t["inputSchema"]),
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

fn parse_templates(items: &[Value]) -> Vec<McpTemplate> {
    items
        .iter()
        .map(|r| McpTemplate {
            uri_template: r["uriTemplate"].as_str().unwrap_or("").to_string(),
            name: r["name"].as_str().unwrap_or("").to_string(),
            description: r["description"].as_str().unwrap_or("").to_string(),
            mime: r["mimeType"].as_str().map(|s| s.to_string()),
        })
        .filter(|r| !r.uri_template.is_empty())
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

/// one elicitation/create request from a server: it wants structured input
/// from the user, described by a flat schema (primitive or enum properties)
pub struct ElicitReq {
    pub server: String,
    pub message: String,
    /// requestedSchema from the server
    pub schema: Value,
}

pub struct ElicitOut {
    /// accept | decline | cancel
    pub action: String,
    pub content: Value,
}

pub type ElicitFut = Pin<Box<dyn Future<Output = Result<ElicitOut>> + Send>>;
pub type Eliciter = Arc<dyn Fn(ElicitReq) -> ElicitFut + Send + Sync>;

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
    pub eliciter: Option<Eliciter>,
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
            eliciter: None,
            notes: None,
            logs: Default::default(),
        }
    }

    pub fn with_sampler(mut self, sampler: Sampler) -> Self {
        self.sampler = Some(sampler);
        self
    }

    pub fn with_eliciter(mut self, eliciter: Eliciter) -> Self {
        self.eliciter = Some(eliciter);
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

/// coerce one user-provided value to the schema property's type; enum picks
/// work by 1-based index, exact or case-insensitive text match
fn coerce_prop(def: &Value, val: &Value) -> Value {
    let s = val.as_str().map(|s| s.trim().to_string());
    match def["type"].as_str().unwrap_or("") {
        "boolean" => match &s {
            Some(t) => json!(matches!(
                t.to_lowercase().as_str(),
                "y" | "yes" | "true" | "1" | "on"
            )),
            None => val.clone(),
        },
        "integer" => match &s {
            Some(t) => t.parse::<i64>().map(|n| json!(n)).unwrap_or_else(|_| val.clone()),
            None => val.clone(),
        },
        "number" => match &s {
            Some(t) => t.parse::<f64>().map(|n| json!(n)).unwrap_or_else(|_| val.clone()),
            None => val.clone(),
        },
        _ => match def["enum"].as_array() {
            Some(vals) if !vals.is_empty() => {
                let t = s.unwrap_or_default();
                if let Ok(n) = t.parse::<usize>() {
                    if (1..=vals.len()).contains(&n) {
                        return vals[n - 1].clone();
                    }
                }
                vals.iter()
                    .find(|v| v.as_str().map(|vs| vs.eq_ignore_ascii_case(&t)).unwrap_or(false))
                    .cloned()
                    .unwrap_or_else(|| val.clone())
            }
            _ => val.clone(),
        },
    }
}

/// build the accept content from a free-text answer, guided by the requested
/// schema: a JSON object wins, one-property schemas take the raw text coerced
/// to the property type, anything else cancels
pub fn elicit_content(schema: &Value, answer: &str) -> Option<Value> {
    let answer = answer.trim();
    if answer.is_empty() {
        return None;
    }
    let props = schema["properties"].as_object()?;
    if let Some(obj) = serde_json::from_str::<Value>(answer)
        .ok()
        .and_then(|v| v.as_object().cloned())
    {
        let mut out = serde_json::Map::new();
        for (k, val) in obj {
            let def = props.get(&k).cloned().unwrap_or(json!({}));
            out.insert(k, coerce_prop(&def, &val));
        }
        return Some(Value::Object(out));
    }
    if props.len() == 1 {
        let (k, def) = props.iter().next().unwrap();
        return Some(json!({ k: coerce_prop(def, &json!(answer)) }));
    }
    None
}

/// format an elicitation request as a question-tool payload: returns the
/// question text and option buttons (only for one-property schemas)
fn elicit_question(message: &str, schema: &Value) -> (String, Vec<Value>) {
    let mut hints = Vec::new();
    let mut opts = Vec::new();
    let props = schema["properties"].as_object();
    let prop_count = props.map(|p| p.len()).unwrap_or(0);
    if let Some(props) = props {
        for (name, def) in props {
            let ty = def["type"].as_str().unwrap_or("string");
            let desc = def["description"].as_str().unwrap_or("");
            let mut h = format!("{name} ({ty})");
            if !desc.is_empty() {
                h.push_str(&format!(": {desc}"));
            }
            hints.push(h);
            if prop_count == 1 {
                if let Some(vals) = def["enum"].as_array() {
                    for v in vals {
                        let label = v.as_str().map(|s| s.to_string()).unwrap_or(v.to_string());
                        opts.push(json!({"label": label}));
                    }
                }
            }
        }
    }
    let mut q = format!("{message}\nfields: {}", hints.join("; "));
    if prop_count == 1 {
        q.push_str("\nanswer with the value; esc cancels");
    } else {
        q.push_str("\nanswer as a json object like {\"field\": value}; esc cancels");
    }
    (q, opts)
}

/// default eliciter: surface the server's request through the ask flow (TUI
/// question prompt / GUI dialog), build the reply from the schema; empty
/// answer cancels
pub fn default_eliciter(
    notes: tokio::sync::mpsc::UnboundedSender<crate::provider::ApiEvent>,
) -> Eliciter {
    Arc::new(move |req| {
        let notes = notes.clone();
        Box::pin(async move {
            let (question, opts) = elicit_question(&req.message, &req.schema);
            let args = json!({
                "questions": [{
                    "header": format!("mcp {}", req.server),
                    "question": question,
                    "options": opts,
                    // raw requestedSchema: the gui renders it as a form, the
                    // tui ignores it and keeps the free-text answer
                    "schema": req.schema,
                }]
            })
            .to_string();
            let (otx, orx) = tokio::sync::oneshot::channel();
            notes
                .send(crate::provider::ApiEvent::Ask {
                    name: "mcp elicit".to_string(),
                    args,
                    rx: otx,
                })
                .map_err(|_| anyhow::anyhow!("ui closed"))?;
            let answer = orx.await.unwrap_or_default();
            match elicit_content(&req.schema, &answer) {
                Some(content) => Ok(ElicitOut {
                    action: "accept".to_string(),
                    content,
                }),
                None => Ok(ElicitOut {
                    action: "cancel".to_string(),
                    content: Value::Null,
                }),
            }
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

/// normalize a progressToken (number or string per spec) to a registry key
fn token_key(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

/// display a progress number without a trailing .0
fn trim_num(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
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

/// legacy http + server-sent-events transport: one long GET stream carries
/// every server message, outgoing messages go to the endpoint the server
/// announces with an `endpoint` event
struct SseCtx {
    url: String,
    http: reqwest::Client,
    headers: BTreeMap<String, String>,
    oauth: Option<crate::config::McpOAuthCfg>,
    /// message endpoint announced via the `endpoint` event
    endpoint: std::sync::Mutex<Option<String>>,
}

/// how outgoing messages are delivered and how replies to server->client
/// requests are sent back
enum Reply {
    Stdio { stdin: Arc<Mutex<ChildStdin>> },
    Http(Arc<HttpCtx>),
    Sse(Arc<SseCtx>),
}

/// state shared between the request path and the background reader task
struct Shared {
    name: String,
    reply: Reply,
    hooks: McpHooks,
    sampling: bool,
    /// elicitation/create requests are routed to the hooks' eliciter
    elicitation: bool,
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
    /// in-flight progress tokens from tools/call _meta.progressToken:
    /// normalized token -> tool label; unknown tokens are ignored
    progress: std::sync::Mutex<BTreeMap<String, String>>,
    /// keepalive state: false while pings keep failing (transition notes fire
    /// only on flips, so a dead server never spams the chat)
    alive: AtomicBool,
    /// per-server timeout override (timeout = <seconds> in config)
    timeout: Option<Duration>,
}

fn req_to(shared: &Shared) -> Duration {
    shared.timeout.unwrap_or(REQUEST_TIMEOUT)
}

fn call_to(shared: &Shared) -> Duration {
    shared.timeout.unwrap_or(CALL_TIMEOUT)
}

fn cfg_connect_timeout(cfg: &McpConfig) -> Duration {
    cfg.timeout.map(Duration::from_secs).unwrap_or(CONNECT_TIMEOUT)
}

type Pending = Arc<Mutex<BTreeMap<u64, tokio::sync::oneshot::Sender<Result<Value>>>>>;

/// build the per-server shared state for one transport choice
fn shared_for(
    cfg: &McpConfig,
    hooks: &McpHooks,
    sampling: bool,
    elicitation: bool,
    reply: Reply,
) -> (Arc<Shared>, Pending) {
    let shared = Arc::new(Shared {
        name: cfg.name.clone(),
        reply,
        hooks: hooks.clone(),
        sampling,
        elicitation,
        next_id: AtomicU64::new(0),
        stale_tools: AtomicBool::new(false),
        stale_resources: AtomicBool::new(false),
        stale_prompts: AtomicBool::new(false),
        closed: AtomicBool::new(false),
        res_sub: AtomicBool::new(false),
        subs: std::sync::Mutex::new(Vec::new()),
        logging: AtomicBool::new(false),
        logging_on: cfg.logging != Some(false),
        progress: std::sync::Mutex::new(BTreeMap::new()),
        alive: AtomicBool::new(true),
        timeout: cfg.timeout.map(Duration::from_secs),
    });
    (shared, Arc::new(Mutex::new(BTreeMap::new())))
}

struct McpServer {
    shared: Arc<Shared>,
    pending: Pending,
    child: Option<Child>,
    /// background tasks (reader/live stream + optional keepalive); aborted on drop
    tasks: Vec<tokio::task::JoinHandle<()>>,
    tools: Vec<McpTool>,
    resources: Vec<McpResource>,
    /// resource templates ride the resources cache (same list_changed)
    templates: Vec<McpTemplate>,
    prompts: Vec<McpPrompt>,
    /// server instructions from initialize, surfaced into the system prompt
    instructions: Option<String>,
}

/// format per-server instructions as a system-prompt block; empty -> empty
pub fn instructions_block(pairs: &[(String, String)]) -> String {
    if pairs.is_empty() {
        return String::new();
    }
    let mut out = String::from("<mcp_instructions>");
    for (name, text) in pairs {
        out.push_str(&format!("\n<server name=\"{name}\">\n{}\n</server>", text.trim()));
    }
    out.push_str("\n</mcp_instructions>");
    out
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
            let res = self.request_t(req_to(&self.shared), method, params).await?;
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
            .request_t(call_to(&self.shared), "resources/read", json!({"uri": uri}))
            .await?;
        Ok(render_resource_contents(&res))
    }

    /// returns prompt messages as (role, text) pairs
    async fn get_prompt(&self, name: &str, args: &Value) -> Result<Vec<(String, String)>> {
        let res = self
            .request_t(
                call_to(&self.shared),
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
        // templates share the resources/list_changed trigger; a failed or
        // unsupported re-list keeps the old cache
        if let Ok(items) = self
            .list_page("resources/templates/list", "resourceTemplates")
            .await
        {
            self.templates = parse_templates(&items);
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
        for t in self.tasks.drain(..) {
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
                    call_to(&shared),
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
        "notifications/progress" => {
            let token = token_key(&params["progressToken"]);
            if token.is_empty() {
                return;
            }
            let label = shared
                .progress
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&token)
                .cloned();
            let Some(label) = label else {
                return;
            };
            let p = params["progress"].as_f64().unwrap_or(0.0);
            let pos = params["total"]
                .as_f64()
                .filter(|t| *t > 0.0)
                .map(|t| format!(" {}/{}", trim_num(p), trim_num(t)))
                .unwrap_or_default();
            let msg = params["message"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(|s| format!(" — {s}"))
                .unwrap_or_default();
            if let Some(tx) = &shared.hooks.notes {
                let _ = tx.send(crate::provider::ApiEvent::Note(format!(
                    "mcp {}: {label}{pos}{msg}",
                    shared.name
                )));
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
                            match tokio::time::timeout(call_to(&shared), sampler(req)).await {
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
        "elicitation/create" => {
            // a decline (not an error) keeps the server's flow well-defined
            if !shared.elicitation {
                Ok(json!({"action": "decline"}))
            } else {
                match &shared.hooks.eliciter {
                    None => Ok(json!({"action": "decline"})),
                    Some(el) => {
                        let req = ElicitReq {
                            server: shared.name.clone(),
                            message: params["message"].as_str().unwrap_or("").to_string(),
                            schema: params["requestedSchema"].clone(),
                        };
                        match tokio::time::timeout(call_to(&shared), el(req)).await {
                            Ok(Ok(out)) => {
                                let mut r = json!({"action": out.action});
                                if out.action == "accept" && !out.content.is_null() {
                                    r["content"] = out.content;
                                }
                                Ok(r)
                            }
                            // a failed or timed-out prompt cancels instead of erroring
                            Ok(Err(_)) | Err(_) => Ok(json!({"action": "cancel"})),
                        }
                    }
                }
            }
        }
        "ping" => Ok(json!({})),
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
        Reply::Sse(ctx) => {
            // answers ride the endpoint announced on the stream; if it is
            // still unknown there is nowhere to send
            let ep = ctx.endpoint.lock().unwrap().clone();
            if let Some(ep) = ep {
                let _ = sse_post(ctx, &shared.name, &ep, &msg).await;
            }
        }
    }
}

/// periodic keepalive: ping the server on an interval, note the alive <->
/// unresponsive transitions only (a dead server must not spam the chat)
async fn keepalive_loop(shared: Arc<Shared>, pending: Pending, interval: Duration) {
    loop {
        tokio::time::sleep(interval).await;
        if shared.closed.load(Ordering::Relaxed) {
            return;
        }
        let ok = request(
            &shared,
            &pending,
            interval.min(REQUEST_TIMEOUT),
            "ping",
            json!({}),
        )
        .await
        .is_ok();
        let was = shared.alive.swap(ok, Ordering::Relaxed);
        if was == ok {
            continue;
        }
        if let Some(tx) = &shared.hooks.notes {
            let _ = tx.send(crate::provider::ApiEvent::Note(if ok {
                format!("mcp {}: keepalive recovered", shared.name)
            } else {
                format!(
                    "mcp {}: keepalive ping failed — server unresponsive",
                    shared.name
                )
            }));
        }
    }
}

/// drain a stdio server's stderr into the shared log buffer as info entries
/// with logger "stderr" (visible via /mcplog, never pops into the chat);
/// skipped entirely when logging = false for the server
async fn stderr_drain(err: impl tokio::io::AsyncRead + Unpin, shared: Arc<Shared>) {
    let mut lines = tokio::io::BufReader::new(err).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                let line = line.trim_end();
                if line.is_empty() || !shared.logging_on {
                    continue;
                }
                let data: String = line.chars().take(2000).collect();
                let mut g = shared
                    .hooks
                    .logs
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                g.push_back(McpLogEntry {
                    server: shared.name.clone(),
                    level: "info".to_string(),
                    logger: "stderr".to_string(),
                    data,
                });
                while g.len() > MAX_LOGS {
                    g.pop_front();
                }
            }
            _ => break,
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

/// connect a remote server: streamable http first, the legacy http + sse
/// transport as fallback (opencode-style). initialize rides the transport
/// selection — a success pins it. auth failures (401) are not transport
/// mismatches, so they surface without a fallback attempt.
async fn connect_remote(
    cfg: &McpConfig,
    hooks: &McpHooks,
    sampling: bool,
    elicitation: bool,
) -> Result<(Arc<Shared>, Pending, Value, Vec<tokio::task::JoinHandle<()>>)> {
    let url = cfg.url.clone().context("mcp: url required")?;
    let http = reqwest::Client::builder().user_agent("hi-derola").build()?;
    let mut caps = json!({"roots": {"listChanged": true}});
    if sampling {
        caps["sampling"] = json!({});
    }
    if elicitation {
        caps["elicitation"] = json!({});
    }
    let init_params = json!({
        "protocolVersion": "2025-03-26",
        "capabilities": caps,
        "clientInfo": {"name": "hi-derola", "version": "0.1.0"}
    });

    // --- streamable http attempt ---
    let (http_shared, pending) = shared_for(
        cfg,
        hooks,
        sampling,
        elicitation,
        Reply::Http(Arc::new(HttpCtx {
            url: url.clone(),
            http: http.clone(),
            headers: cfg.headers.clone(),
            oauth: cfg.oauth_cfg(),
            session: std::sync::Mutex::new(None),
        })),
    );
    let to = req_to(&http_shared);
    let first_err = match request(&http_shared, &pending, to, "initialize", init_params.clone()).await {
        Ok(v) => return Ok((http_shared, pending, v, Vec::new())),
        Err(e) => e,
    };
    if format!("{first_err:#}").contains("401 unauthorized") {
        return Err(first_err);
    }

    // --- legacy http + sse attempt; on failure the original error is the
    // meaningful one for every common case (dead server, real jsonrpc error)
    let (sse_shared, sse_pending) = shared_for(
        cfg,
        hooks,
        sampling,
        elicitation,
        Reply::Sse(Arc::new(SseCtx {
            url: url.clone(),
            http,
            headers: cfg.headers.clone(),
            oauth: cfg.oauth_cfg(),
            endpoint: std::sync::Mutex::new(None),
        })),
    );
    let mut tasks = Vec::new();
    tasks.push(tokio::spawn(sse_live(sse_shared.clone(), sse_pending.clone())));
    match request(&sse_shared, &sse_pending, to, "initialize", init_params).await {
        Ok(v) => Ok((sse_shared, sse_pending, v, tasks)),
        Err(_) => Err(first_err),
    }
}

/// background reader for the legacy sse transport: opens the event stream,
/// learns the message endpoint from the `endpoint` event, routes every json
/// event through pending/dispatch; reconnects with backoff until closed
async fn sse_live(shared: Arc<Shared>, pending: Pending) {
    let Reply::Sse(ctx) = &shared.reply else {
        return;
    };
    let mut backoff = 1u64;
    loop {
        if shared.closed.load(Ordering::Relaxed) {
            return;
        }
        match sse_get_stream(ctx, &shared.name).await {
            Ok(r) if r.status().is_success() => {
                backoff = 1;
                let _ = sse_consume(r, ctx, &shared, &pending).await;
            }
            _ => {}
        }
        if shared.closed.load(Ordering::Relaxed) {
            return;
        }
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(30);
    }
}

async fn sse_get_stream(ctx: &SseCtx, name: &str) -> Result<reqwest::Response> {
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
    for (k, v) in ctx.headers.iter() {
        req = req.header(k.as_str(), v.as_str());
    }
    Ok(req.send().await?)
}

/// resolve a possibly relative endpoint announcement against the server url
fn absolute_url(base: &str, target: &str) -> String {
    if target.starts_with("http://") || target.starts_with("https://") {
        return target.to_string();
    }
    match reqwest::Url::parse(base).and_then(|u| u.join(target)) {
        Ok(u) => u.to_string(),
        Err(_) => target.to_string(),
    }
}

/// POST one message to the announced endpoint
async fn sse_post(ctx: &SseCtx, name: &str, ep: &str, body: &Value) -> Result<()> {
    let token = crate::mcpauth::bearer(name, &ctx.url, ctx.oauth.as_ref(), &ctx.http, false)
        .await
        .unwrap_or(None);
    let mut req = ctx.http.post(ep).header("Content-Type", "application/json");
    if let Some(t) = &token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    for (k, v) in ctx.headers.iter() {
        req = req.header(k.as_str(), v.as_str());
    }
    let fut = req.body(body.to_string()).send();
    match tokio::time::timeout(REQUEST_TIMEOUT, fut).await {
        Err(_) => bail!("mcp {name}: sse post timeout"),
        Ok(r) => {
            let r = r?;
            if !r.status().is_success() {
                bail!("mcp {name}: sse post {}", r.status());
            }
        }
    }
    Ok(())
}

/// wait until the server announces its message endpoint (sent immediately
/// after the stream opens; the cap keeps a wrong-transport fallback fast)
async fn sse_wait_endpoint(ctx: &SseCtx, timeout: Duration) -> Result<String> {
    let deadline = tokio::time::Instant::now() + timeout.min(Duration::from_secs(2));
    loop {
        if let Some(ep) = ctx.endpoint.lock().unwrap().clone() {
            return Ok(ep);
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("mcp: sse endpoint was not announced");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// consume the legacy sse stream: `endpoint` events pin the message endpoint,
/// `message` events carry the jsonrpc traffic
async fn sse_consume(
    mut resp: reqwest::Response,
    ctx: &Arc<SseCtx>,
    shared: &Arc<Shared>,
    pending: &Pending,
) {
    let mut event = String::new();
    let mut buf = String::new();
    loop {
        let Ok(Some(bytes)) = resp.chunk().await else {
            break;
        };
        buf.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(pos) = buf.find('\n') {
            let line: String = buf.drain(..pos + 1).collect();
            let line = line.trim_end();
            if let Some(e) = line.strip_prefix("event:") {
                event = e.trim().to_string();
                continue;
            }
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if event == "endpoint" {
                *ctx.endpoint.lock().unwrap() = Some(absolute_url(&ctx.url, data));
                event.clear();
                continue;
            }
            event.clear();
            if data.is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            dispatch_incoming(shared, pending, v).await;
        }
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
        Reply::Sse(ctx) => {
            // the reply arrives on the event stream, routed through pending
            let ep = sse_wait_endpoint(ctx, req_to(shared)).await?;
            sse_post(ctx, &shared.name, &ep, msg).await?;
            Ok(json!({}))
        }
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
            .request_t(req_to(&self.shared), "resources/subscribe", json!({"uri": uri}))
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
            .request_t(req_to(&self.shared), "resources/unsubscribe", json!({"uri": uri}))
            .await;
        Ok(())
    }

    async fn connect(cfg: &McpConfig, hooks: &McpHooks) -> Result<Self> {
        let remote = cfg.r#type.as_deref() == Some("remote")
            || (cfg.command.is_empty() && cfg.url.is_some());
        let sampling = hooks.sampler.is_some() && cfg.sampling != Some(false);
        let elicitation = hooks.eliciter.is_some() && cfg.elicitation != Some(false);
        let (shared, pending, child, reader, stderr, pre_tasks, init) = if remote {
            let (sh, pd, i, tasks) = connect_remote(cfg, hooks, sampling, elicitation).await?;
            (sh, pd, None, None, None, tasks, Some(i))
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
                // stderr is piped so a chatty server cannot block on a full
                // pipe; the drain task logs it (or discards it)
                .stderr(std::process::Stdio::piped())
                .spawn()
                .with_context(|| format!("mcp {}: spawn {}", cfg.name, cfg.command))?;
            let stdin = child.stdin.take().context("mcp: no stdin")?;
            let stdout = child.stdout.take().context("mcp: no stdout")?;
            let stderr = child.stderr.take();
            let (sh, pd) = shared_for(
                cfg,
                hooks,
                sampling,
                elicitation,
                Reply::Stdio {
                    stdin: Arc::new(Mutex::new(stdin)),
                },
            );
            (sh, pd, Some(child), Some(tokio::io::BufReader::new(stdout)), stderr, Vec::new(), None)
        };
        let mut s = Self {
            shared,
            pending,
            child,
            tasks: Vec::new(),
            tools: Vec::new(),
            resources: Vec::new(),
            templates: Vec::new(),
            prompts: Vec::new(),
            instructions: None,
        };
        // stderr of a stdio server lands in the shared log buffer (logger
        // "stderr", info level — /mcplog shows it, the chat is not spammed);
        // always drained so the child never blocks on a full pipe
        if let Some(err) = stderr {
            s.tasks
                .push(tokio::spawn(stderr_drain(err, s.shared.clone())));
        }
        // background traffic: stdio reader or the http live stream (a no-op
        // for the legacy sse transport, which runs its own stream task)
        if let Some(reader) = reader {
            s.tasks.push(tokio::spawn(stdio_reader(
                reader,
                s.shared.clone(),
                s.pending.clone(),
            )));
        } else {
            s.tasks.push(tokio::spawn(http_live(
                s.shared.clone(),
                s.pending.clone(),
            )));
        }
        s.tasks.extend(pre_tasks);
        // optional keepalive: periodic pings note alive <-> unresponsive flips
        let ka_secs = cfg.keepalive.unwrap_or(0);
        if ka_secs > 0 {
            s.tasks.push(tokio::spawn(keepalive_loop(
                s.shared.clone(),
                s.pending.clone(),
                Duration::from_secs(ka_secs),
            )));
        }
        let proto = match s.shared.reply {
            Reply::Http(_) => "2025-03-26",
            Reply::Sse(_) | Reply::Stdio { .. } => "2024-11-05",
        };
        let mut client_caps = json!({"roots": {"listChanged": true}});
        if sampling {
            client_caps["sampling"] = json!({});
        }
        if elicitation {
            client_caps["elicitation"] = json!({});
        }
        // remote transports already answered initialize during transport
        // selection; stdio sends it here
        let init = match init {
            Some(v) => v,
            None => {
                s.request_t(
                    req_to(&s.shared),
                    "initialize",
                    json!({
                        "protocolVersion": proto,
                        "capabilities": client_caps,
                        "clientInfo": {"name": "hi-derola", "version": "0.1.0"}
                    }),
                )
                .await?
            }
        };
        let caps = init["capabilities"].clone();
        s.instructions = init["instructions"]
            .as_str()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(String::from);
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
            // templates are optional too: a method-not-found just means an
            // older server with plain resources
            if let Ok(items) = s
                .list_page("resources/templates/list", "resourceTemplates")
                .await
            {
                s.templates = parse_templates(&items);
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
        let to = cfg_connect_timeout(c);
        match tokio::time::timeout(to, McpServer::connect(c, hooks)).await {
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
        let to = cfg_connect_timeout(cfg);
        match tokio::time::timeout(to, client.replace(cfg, hooks)).await {
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

    /// per-server instructions from initialize, paired with the server name;
    /// a server whose every tool is denied by the permission rules stays quiet
    pub async fn instructions(&self, perm: &crate::perm::PermCfg) -> Vec<(String, String)> {
        let servers = self.servers.lock().await;
        servers
            .iter()
            .filter(|s| s.instructions.is_some())
            .filter(|s| {
                let denied = |tool: &str| perm.check(tool, "{}") == crate::perm::Perm::Deny;
                s.tools.is_empty()
                    || s.tools
                        .iter()
                        .any(|t| !denied(&format!("mcp__{}__{}", s.shared.name, t.name)))
            })
            .map(|s| (s.shared.name.clone(), s.instructions.clone().unwrap()))
            .collect()
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
        // the token lets the server report live progress via notifications/progress
        let token = s.shared.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let params = json!({
            "name": tool,
            "arguments": arguments,
            "_meta": {"progressToken": token}
        });
        s.shared
            .progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(token.to_string(), tool.to_string());
        let res = s
            .request_t(call_to(&s.shared), "tools/call", params)
            .await;
        s.shared
            .progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&token.to_string());
        let res = res?;
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
        // structuredContent fallback: a server may answer with only
        // structuredContent and no text blocks
        if text.is_empty() {
            if let Some(sc) = res.get("structuredContent") {
                if !sc.is_null() {
                    text = serde_json::to_string(sc).unwrap_or_default();
                }
            }
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

    /// all resource templates across servers, in connect order
    pub async fn templates(&self) -> Vec<McpTemplateInfo> {
        let mut servers = self.servers.lock().await;
        let mut out = Vec::new();
        for s in servers.iter_mut() {
            s.refresh_resources().await;
            for t in &s.templates {
                out.push(McpTemplateInfo {
                    server: s.shared.name.clone(),
                    uri_template: t.uri_template.clone(),
                    name: t.name.clone(),
                    description: t.description.clone(),
                    mime: t.mime.clone(),
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
                req_to(&s.shared),
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

pub struct McpTemplateInfo {
    pub server: String,
    pub uri_template: String,
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

        // outputSchema tolerance: exotic outputSchema ignored, broken
        // inputSchema coerced to a valid object schema
        let weird = parse_tools(&[json!({
            "name": "w",
            "description": "d",
            "inputSchema": Value::Null,
            "outputSchema": {"type": "object", "$ref": "#/definitions/missing"}
        })]);
        assert_eq!(weird.len(), 1);
        assert_eq!(weird[0].schema, json!({"type": "object", "properties": {}}));
        let missing = parse_tools(&[json!({"name": "noschema"})]);
        assert_eq!(missing[0].schema, json!({"type": "object", "properties": {}}));

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
        // stderr probe: the client must pipe it into the log buffer
        eprintln!("fake-mcp stderr line");
        let stdin = std::io::stdin();
        let mut out = std::io::stdout().lock();
        let mut caps_seen = Value::Null;
        let mut roots_reply = Value::Null;
        let mut sampling_reply = Value::Null;
        let mut sampling_error = false;
        let mut elicit_reply = Value::Null;
        let mut elicit_error = false;
        let mut ping_reply = Value::Null;
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
                        json!({"jsonrpc": "2.0", "id": 503, "method": "elicitation/create", "params": {
                            "message": "pick a color",
                            "requestedSchema": {"type": "object", "properties": {
                                "color": {"type": "string", "enum": ["red", "green"]},
                                "count": {"type": "integer"}
                            }}
                        }}),
                        json!({"jsonrpc": "2.0", "id": 504, "method": "ping", "params": {}}),
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
            // nopong mode keeps pings hanging: keepalive must time out
            if method == "ping" && mode == "nopong" {
                continue;
            }
            let Some(id) = v.get("id").and_then(|x| x.as_u64()) else {
                continue;
            };
            // replies to the server-initiated requests above
            if (501..=504).contains(&id) {
                if v.get("error").is_some() {
                    if id == 503 {
                        elicit_error = true;
                        elicit_reply = v["error"].clone();
                    } else {
                        sampling_error = true;
                        sampling_reply = v["error"].clone();
                    }
                } else if id == 501 {
                    roots_reply = v["result"].clone();
                } else if id == 502 {
                    sampling_reply = v["result"].clone();
                } else if id == 503 {
                    elicit_reply = v["result"].clone();
                } else {
                    ping_reply = v["result"].clone();
                }
                continue;
            }
            let result = match method.as_str() {
                "initialize" => {
                    caps_seen = v["params"]["capabilities"].clone();
                    if mode == "min" {
                        json!({"capabilities": {}})
                    } else {
                        json!({"capabilities": {"tools": {}, "resources": {"subscribe": true}, "prompts": {}, "logging": {}}, "instructions": "always call ping twice"})
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
                            {"name": "ping2", "description": "added", "inputSchema": {"type": "object"}},
                            {"name": "weird", "description": "d", "inputSchema": null,
                             "outputSchema": {"type": "object", "properties": {"x": {"$ref": "#/definitions/none"}}}}
                        ]})
                    }
                }
                "tools/call" => {
                    if mode == "struct" {
                        // no content blocks: the client must fall back to
                        // structuredContent
                        json!({"structuredContent": {"answer": 42}})
                    } else {
                    // progress for the in-flight token, then a stale token
                    // the client must ignore; live refresh trigger last
                    let tok = v["params"]["_meta"]["progressToken"].clone();
                    let notes = [
                        json!({"jsonrpc": "2.0", "method": "notifications/progress",
                            "params": {"progressToken": tok, "progress": 1, "total": 2, "message": "halfway"}}),
                        json!({"jsonrpc": "2.0", "method": "notifications/progress",
                            "params": {"progressToken": tok, "progress": 1.5, "total": 2}}),
                        json!({"jsonrpc": "2.0", "method": "notifications/progress",
                            "params": {"progressToken": 987654, "progress": 1, "total": 1}}),
                    ];
                    for n in notes {
                        writeln!(out, "{n}").unwrap();
                    }
                    out.flush().unwrap();
                    // live refresh trigger: notify before answering
                    let n = json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"});
                    writeln!(out, "{n}").unwrap();
                    out.flush().unwrap();
                    json!({"content": [{"type": "text", "text": json!({
                        "caps": caps_seen,
                        "roots": roots_reply,
                        "sampling": sampling_reply,
                        "sampling_error": sampling_error,
                        "elicitation": elicit_reply,
                        "elicit_error": elicit_error,
                        "ping_reply": ping_reply,
                    }).to_string()}]})
                    }
                }
                "resources/list" => {
                    if v["params"]["cursor"].as_str().is_none() {
                        json!({"resources": [{"uri": "file:///a.txt", "name": "a", "description": "file a", "mimeType": "text/plain"}], "nextCursor": "p2"})
                    } else {
                        json!({"resources": [{"uri": "mem://stats", "name": "stats", "description": "live stats"}]})
                    }
                }
                "resources/templates/list" => json!({"resourceTemplates": [
                    {"uriTemplate": "file:///{path}", "name": "files", "description": "file by path", "mimeType": "text/plain"}
                ]}),
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
        child_cfg_full(mode, sampling, logging, None)
    }

    fn child_cfg_full(
        mode: &str,
        sampling: Option<bool>,
        logging: Option<bool>,
        elicitation: Option<bool>,
    ) -> McpConfig {
        child_cfg_ka(mode, sampling, logging, elicitation, None)
    }

    fn child_cfg_ka(
        mode: &str,
        sampling: Option<bool>,
        logging: Option<bool>,
        elicitation: Option<bool>,
        keepalive: Option<u64>,
    ) -> McpConfig {
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
            elicitation,
            logging,
            keepalive,
            timeout: None,
        }
    }

    fn child_cfg_timeout(mode: &str, timeout: Option<u64>) -> McpConfig {
        let mut c = child_cfg(mode);
        c.timeout = timeout;
        c
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
        assert_eq!(s.templates.len(), 1, "templates listed alongside resources");
        assert_eq!(s.templates[0].uri_template, "file:///{path}");
        assert_eq!(s.templates[0].name, "files");
        assert_eq!(s.templates[0].mime.as_deref(), Some("text/plain"));
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
        assert_eq!(dump["ping_reply"], json!({}), "server ping answered with empty result");

        // the ping reply was preceded by notifications/tools/list_changed:
        // the next specs() must re-list and see the second tool
        let client = McpClient {
            servers: Mutex::new(vec![s]),
            hooks: hooks.clone(),
        };
        let tpls = client.templates().await;
        assert_eq!(tpls.len(), 1, "templates ride the resources cache");
        assert_eq!(tpls[0].server, "t");
        assert_eq!(tpls[0].uri_template, "file:///{path}");
        let specs = client.specs().await;
        assert_eq!(specs.len(), 3, "tools refreshed after list_changed");
        // outputSchema tolerance: the unresolvable $ref is ignored and the
        // broken (null) inputSchema is coerced to a valid object schema
        let weird = specs.iter().find(|t| t.name == "mcp__t__weird").unwrap();
        assert_eq!(
            weird.parameters,
            json!({"type": "object", "properties": {}}),
            "broken inputSchema coerced, exotic outputSchema ignored"
        );
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
    async fn elicitation_roundtrip() {
        let hooks = McpHooks::workspace(Some(std::path::PathBuf::from("/tmp/ws"))).with_eliciter(
            Arc::new(|_req: ElicitReq| {
                Box::pin(async move {
                    Ok(ElicitOut {
                        action: "accept".into(),
                        content: json!({"color": "green", "count": 3}),
                    })
                })
            }),
        );
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
        assert_eq!(dump["caps"]["elicitation"], json!({}), "elicitation cap declared");
        assert_eq!(dump["elicit_error"], false);
        assert_eq!(dump["elicitation"]["action"], "accept");
        assert_eq!(dump["elicitation"]["content"]["color"], "green");
        assert_eq!(dump["elicitation"]["content"]["count"], 3);
    }

    #[tokio::test]
    async fn elicitation_opt_out_declines() {
        let hooks = McpHooks::workspace(Some(std::path::PathBuf::from("/tmp/ws"))).with_eliciter(
            Arc::new(|_req: ElicitReq| {
                Box::pin(async move {
                    Ok(ElicitOut {
                        action: "accept".into(),
                        content: json!({"color": "green"}),
                    })
                })
            }),
        );
        let cfg = child_cfg_full("1", None, None, Some(false));
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
        assert!(dump["caps"]["elicitation"].is_null(), "no cap when disabled");
        assert_eq!(dump["elicitation"]["action"], "decline");
    }

    #[test]
    fn elicit_answer_parsing() {
        let schema = json!({"type": "object", "properties": {
            "color": {"type": "string", "enum": ["red", "green", "blue"]},
            "count": {"type": "integer"},
            "ok": {"type": "boolean"}
        }});
        let c = elicit_content(&schema, r#" {"color": "2", "count": "7", "ok": "y"} "#).unwrap();
        assert_eq!(c["color"], "green", "enum pick by 1-based index");
        assert_eq!(c["count"], 7, "integer coerced from text");
        assert_eq!(c["ok"], true, "boolean coerced from text");

        let single = json!({"type": "object", "properties": {"name": {"type": "string"}}});
        assert_eq!(
            elicit_content(&single, "hello").unwrap(),
            json!({"name": "hello"}),
            "one-property schema takes the raw text"
        );
        let enum1 = json!({"type": "object", "properties": {"c": {"enum": ["Red", "Green"]}}});
        assert_eq!(
            elicit_content(&enum1, "green").unwrap(),
            json!({"c": "Green"}),
            "enum case-insensitive match"
        );
        let enum2 = json!({"type": "object", "properties": {"c": {"enum": ["red", "green"]}}});
        assert_eq!(
            elicit_content(&enum2, "2").unwrap(),
            json!({"c": "green"}),
            "enum pick by index"
        );
        assert!(elicit_content(&schema, "").is_none(), "empty cancels");
        assert!(elicit_content(&schema, "  ").is_none(), "blank cancels");
        assert!(elicit_content(&schema, "green").is_none(), "multi-property raw text cancels");
        assert!(elicit_content(&json!({}), "x").is_none(), "no properties cancels");
    }

    #[tokio::test]
    async fn default_eliciter_uses_ask_flow() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let el = default_eliciter(tx);
        let schema = json!({"type": "object", "properties": {
            "color": {"type": "string", "enum": ["red", "green"]}
        }});
        let expected = schema.clone();
        tokio::spawn(async move {
            if let Some(crate::provider::ApiEvent::Ask { args, rx: arx, .. }) = rx.recv().await {
                let v: Value = serde_json::from_str(&args).unwrap();
                assert_eq!(
                    v["questions"][0]["schema"], expected,
                    "gui gets the raw schema to render a form"
                );
                let _ = arx.send("green".to_string());
            }
        });
        let out = el(ElicitReq {
            server: "t".into(),
            message: "pick".into(),
            schema,
        })
        .await
        .unwrap();
        assert_eq!(out.action, "accept");
        assert_eq!(out.content["color"], "green");

        // empty answer (esc in the tui, skip in the gui) cancels
        let (tx2, mut rx2) = tokio::sync::mpsc::unbounded_channel();
        let el2 = default_eliciter(tx2);
        tokio::spawn(async move {
            if let Some(crate::provider::ApiEvent::Ask { rx: arx, .. }) = rx2.recv().await {
                let _ = arx.send(String::new());
            }
        });
        let out = el2(ElicitReq {
            server: "t".into(),
            message: "pick".into(),
            schema: json!({"type": "object", "properties": {"color": {"type": "string"}}}),
        })
        .await
        .unwrap();
        assert_eq!(out.action, "cancel");
    }

    #[tokio::test]
    async fn progress_notes_for_live_tool_calls() {
        let (ntx, mut nrx) = tokio::sync::mpsc::unbounded_channel();
        let hooks = McpHooks::workspace(Some(std::path::PathBuf::from("/tmp/ws"))).with_notes(ntx);
        let cfg = child_cfg("1");
        let s = McpServer::connect(&cfg, &hooks).await.unwrap();
        let client = McpClient {
            servers: Mutex::new(vec![s]),
            hooks: hooks.clone(),
        };

        client.call("t__ping", "{}").await.unwrap();

        // the two progress notifications for the in-flight token surfaced,
        // the stale unknown token did not
        let mut saw = Vec::new();
        while let Ok(crate::provider::ApiEvent::Note(n)) = nrx.try_recv() {
            if n.starts_with("mcp t: ping") {
                saw.push(n);
            }
        }
        assert_eq!(
            saw,
            vec![
                "mcp t: ping 1/2 — halfway".to_string(),
                "mcp t: ping 1.5/2".to_string(),
            ],
            "live progress notes, fractional progress kept, unknown token ignored"
        );

        // the token registry is cleaned up after the call
        let servers = client.servers.lock().await;
        assert!(servers[0]
            .shared
            .progress
            .lock()
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn keepalive_stays_silent_on_healthy_server() {
        let (ntx, mut nrx) = tokio::sync::mpsc::unbounded_channel();
        let hooks = McpHooks::workspace(Some(std::path::PathBuf::from("/tmp/ws"))).with_notes(ntx);
        let cfg = child_cfg_ka("1", None, None, None, Some(1));
        let _s = McpServer::connect(&cfg, &hooks).await.unwrap();
        // two keepalive intervals with prompt ping answers: no keepalive notes
        // (the mode-1 child does note its error log message, that one is fine)
        tokio::time::sleep(Duration::from_millis(2500)).await;
        while let Ok(crate::provider::ApiEvent::Note(n)) = nrx.try_recv() {
            assert!(!n.contains("keepalive"), "healthy server must not note: {n}");
        }
    }

    #[tokio::test]
    async fn keepalive_notes_unresponsive_server_once() {
        let (ntx, mut nrx) = tokio::sync::mpsc::unbounded_channel();
        let hooks = McpHooks::workspace(Some(std::path::PathBuf::from("/tmp/ws"))).with_notes(ntx);
        let cfg = child_cfg_ka("nopong", None, None, None, Some(1));
        let _s = McpServer::connect(&cfg, &hooks).await.unwrap();
        // pings hang, every ping times out after ~1s: exactly one transition note
        tokio::time::sleep(Duration::from_millis(3500)).await;
        let mut notes = Vec::new();
        while let Ok(crate::provider::ApiEvent::Note(n)) = nrx.try_recv() {
            notes.push(n);
        }
        assert_eq!(
            notes
                .iter()
                .filter(|n| n.contains("keepalive ping failed"))
                .count(),
            1,
            "one unresponsive note despite repeated failures, got {notes:?}"
        );
        assert!(notes.iter().all(|n| !n.contains("keepalive recovered")));
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

        // the initialize-phase notifications/message is buffered alongside
        // the child's stderr probe line (drained into the same buffer)
        let mut ready = false;
        for _ in 0..100 {
            let g = hooks.logs.lock().unwrap();
            ready = g.iter().any(|e| e.logger == "stderr")
                && g.iter().any(|e| e.logger == "db");
            drop(g);
            if ready {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(ready, "stderr probe and log entry must both drain");
        let logs = hooks.logs.lock().unwrap();
        let boom = logs.iter().find(|e| e.logger == "db").expect("log entry buffered");
        assert_eq!(boom.server, "t");
        assert_eq!(boom.level, "error");
        assert_eq!(boom.data, "boom");
        assert!(logs.len() >= 2, "stderr entry rides the same buffer");
        drop(logs);
        assert!(client.logs().len() >= 2);

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
        // the child's stderr probe must be dropped too, not just notifications/message
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            hooks.logs.lock().unwrap().is_empty(),
            "logging = false drops notifications/message and stderr"
        );
        assert!(nrx.try_recv().is_err(), "no notes when logging is off");
    }

    #[tokio::test]
    async fn server_instructions_captured_and_formatted() {
        let hooks = McpHooks::workspace(Some(std::path::PathBuf::from("/tmp/ws")));
        let s = McpServer::connect(&child_cfg("1"), &hooks).await.unwrap();
        assert_eq!(s.instructions.as_deref(), Some("always call ping twice"));

        // min mode: initialize carries no instructions
        let s = McpServer::connect(&child_cfg("min"), &hooks).await.unwrap();
        assert!(s.instructions.is_none());

        let block = instructions_block(&[("srv".into(), "  doc line\n".into())]);
        assert!(block.starts_with("<mcp_instructions>"));
        assert!(block.contains("<server name=\"srv\">\ndoc line\n</server>"));
        assert!(block.ends_with("</mcp_instructions>"));
        assert_eq!(instructions_block(&[]), "");
    }

    #[tokio::test]
    async fn instructions_filtered_by_deny_rules() {
        let hooks = McpHooks::workspace(Some(std::path::PathBuf::from("/tmp/ws")));
        let s = McpServer::connect(&child_cfg("1"), &hooks).await.unwrap();
        let client = McpClient {
            servers: Mutex::new(vec![s]),
            hooks: hooks.clone(),
        };

        // default permissions: instructions ride the prompt
        let pairs = client.instructions(&crate::perm::PermCfg::default()).await;
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, "t");

        // mcp = deny silences every server that has tools
        let perm = crate::perm::PermCfg {
            mcp: Some("deny".into()),
            ..Default::default()
        };
        assert!(client.instructions(&perm).await.is_empty());
    }

    #[tokio::test]
    async fn structured_content_fallback() {
        let hooks = McpHooks::workspace(None);
        let (client, _) = connect_all(&[child_cfg("struct")], &hooks).await;
        let client = client.expect("struct server connects");
        let out = client.call("t__ping", "{}").await.unwrap();
        assert_eq!(out, r#"{"answer":42}"#);
    }

    #[tokio::test]
    async fn per_server_timeout_applies() {
        let hooks = McpHooks::workspace(None);
        let s = McpServer::connect(&child_cfg_timeout("1", Some(5)), &hooks)
            .await
            .unwrap();
        assert_eq!(s.shared.timeout, Some(Duration::from_secs(5)));
        assert_eq!(req_to(&s.shared), Duration::from_secs(5));
        assert_eq!(call_to(&s.shared), Duration::from_secs(5));
        assert_eq!(cfg_connect_timeout(&child_cfg_timeout("1", Some(5))), Duration::from_secs(5));

        // defaults stay untouched without the override
        let s = McpServer::connect(&child_cfg("1"), &hooks).await.unwrap();
        assert_eq!(s.shared.timeout, None);
        assert_eq!(req_to(&s.shared), REQUEST_TIMEOUT);
        assert_eq!(call_to(&s.shared), CALL_TIMEOUT);
        assert_eq!(cfg_connect_timeout(&child_cfg("1")), CONNECT_TIMEOUT);
    }

    /// read one http request (headers + content-length body) from a raw stream
    fn read_http_req(s: &mut std::net::TcpStream) -> String {
        use std::io::Read as _;
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        let head_end = loop {
            match s.read(&mut tmp) {
                Ok(0) => break buf.len(),
                Ok(n) => {
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break pos + 4;
                    }
                }
                Err(_) => break buf.len(),
            }
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
        let len: usize = head
            .lines()
            .find_map(|l| {
                let (k, v) = l.split_once(':')?;
                if k.trim().eq_ignore_ascii_case("content-length") {
                    v.trim().parse().ok()
                } else {
                    None
                }
            })
            .unwrap_or(0);
        while buf.len() < head_end + len {
            match s.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(&tmp[..n]),
                Err(_) => break,
            }
        }
        String::from_utf8_lossy(&buf).to_string()
    }

    #[tokio::test]
    async fn sse_fallback_connects_legacy_servers() {
        use std::io::Write as _;
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        // writer handle: the post handlers answer over the get stream
        let writer: Arc<std::sync::Mutex<Option<std::net::TcpStream>>> = Arc::default();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                let w = writer.clone();
                std::thread::spawn(move || {
                    let req = read_http_req(&mut s);
                    let head = req.lines().next().unwrap_or("").to_string();
                    let mut parts = head.split_whitespace();
                    let method = parts.next().unwrap_or("").to_string();
                    let target = parts.next().unwrap_or("/").to_string();
                    let path = target.split('?').next().unwrap_or("/").to_string();
                    let body = req.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
                    if method == "POST" && path == "/mcp" {
                        // streamable http unsupported: a legacy sse-only server
                        let _ = s.write_all(
                            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                        return;
                    }
                    if method == "GET" && path == "/mcp" {
                        let _ = s.write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: keep-alive\r\n\r\n",
                        );
                        // share the writer before the announcement: posts may
                        // arrive as soon as the client sees the endpoint
                        *w.lock().unwrap() = Some(s.try_clone().unwrap());
                        let _ = s.write_all(b"event: endpoint\ndata: /messages?sessionId=s1\n\n");
                        let _ = s.flush();
                        // hold the stream open; post handlers write the replies
                        loop {
                            std::thread::park();
                        }
                    }
                    if method == "POST" && path == "/messages" {
                        let _ = s.write_all(
                            b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                        let Ok(v) = serde_json::from_str::<Value>(&body) else {
                            return;
                        };
                        let Some(id) = v.get("id").cloned() else {
                            return;
                        };
                        let result = match v["method"].as_str().unwrap_or("") {
                            "initialize" => {
                                json!({"capabilities": {"tools": {}}, "instructions": "via legacy sse"})
                            }
                            "tools/list" => json!({"tools": [
                                {"name": "sse_tool", "description": "d", "inputSchema": {"type": "object"}}
                            ]}),
                            _ => json!({}),
                        };
                        let reply = json!({"jsonrpc": "2.0", "id": id, "result": result});
                        let msg = format!("event: message\ndata: {reply}\n\n");
                        let g = w.lock().unwrap();
                        if let Some(wr) = g.as_ref() {
                            let mut wr = wr;
                            let _ = wr.write_all(msg.as_bytes());
                            let _ = wr.flush();
                        }
                    }
                });
            }
        });

        let cfg = McpConfig {
            name: "t".to_string(),
            r#type: Some("remote".to_string()),
            command: String::new(),
            args: Vec::new(),
            env: BTreeMap::new(),
            url: Some(format!("http://127.0.0.1:{port}/mcp")),
            headers: BTreeMap::new(),
            oauth: None,
            sampling: None,
            elicitation: None,
            logging: None,
            keepalive: None,
            timeout: None,
        };
        let (client, logs) = connect_all(&[cfg], &McpHooks::workspace(None)).await;
        let client = client.expect("sse fallback must connect");
        let specs = client.specs().await;
        assert_eq!(specs.len(), 1, "tools listed over the sse transport");
        assert_eq!(specs[0].name, "mcp__t__sse_tool");
        assert!(logs.iter().any(|l| l.contains("connected")), "{logs:?}");
        // initialize rode the sse transport: instructions arrived over the stream
        assert_eq!(
            client
                .instructions(&crate::perm::PermCfg::default())
                .await,
            vec![("t".to_string(), "via legacy sse".to_string())]
        );
    }
}
