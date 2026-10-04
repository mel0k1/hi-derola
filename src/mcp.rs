use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;

use crate::config::McpConfig;
use crate::provider::ToolSpec;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

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

enum Transport {
    Stdio {
        child: Child,
        stdin: ChildStdin,
        reader: tokio::io::BufReader<ChildStdout>,
        next_id: u64,
    },
    Http {
        url: String,
        http: reqwest::Client,
        session: Option<String>,
        headers: BTreeMap<String, String>,
        next_id: u64,
    },
}

struct McpServer {
    name: String,
    transport: Transport,
    tools: Vec<McpTool>,
    resources: Vec<McpResource>,
    prompts: Vec<McpPrompt>,
    oauth: Option<crate::config::McpOAuthCfg>,
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
    async fn list_page(&mut self, method: &str, key: &str) -> Result<Vec<Value>> {
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

    async fn read_resource(&mut self, uri: &str) -> Result<String> {
        let res = self
            .request_t(CALL_TIMEOUT, "resources/read", json!({"uri": uri}))
            .await?;
        Ok(render_resource_contents(&res))
    }

    /// returns prompt messages as (role, text) pairs
    async fn get_prompt(&mut self, name: &str, args: &Value) -> Result<Vec<(String, String)>> {
        let res = self
            .request_t(
                CALL_TIMEOUT,
                "prompts/get",
                json!({"name": name, "arguments": args}),
            )
            .await?;
        Ok(parse_prompt_messages(&res))
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        if let Transport::Stdio { child, .. } = &mut self.transport {
            let _ = child.start_kill();
        }
    }
}

async fn wait_response(
    reader: &mut tokio::io::BufReader<ChildStdout>,
    id: u64,
    name: &str,
) -> Result<Value> {
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            bail!("mcp {name}: server closed");
        }
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if v.get("id").and_then(|x| x.as_u64()) == Some(id) {
            return extract_result(v, name);
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

async fn sse_response(mut resp: reqwest::Response, id: u64, name: &str) -> Result<Value> {
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
            if let Ok(v) = serde_json::from_str::<Value>(data) {
                if v.get("id").and_then(|x| x.as_u64()) == Some(id) {
                    return extract_result(v, name);
                }
            }
        }
    }
    bail!("mcp {name}: no response in sse stream")
}

impl McpServer {
    async fn connect(cfg: &McpConfig) -> Result<Self> {
        let remote = cfg.r#type.as_deref() == Some("remote")
            || (cfg.command.is_empty() && cfg.url.is_some());
        let mut s = if remote {
            let url = cfg.url.clone().context("mcp: url required")?;
            let http = reqwest::Client::builder().user_agent("hi-derola").build()?;
            Self {
                name: cfg.name.clone(),
                transport: Transport::Http {
                    url,
                    http,
                    session: None,
                    headers: cfg.headers.clone(),
                    next_id: 0,
                },
                oauth: cfg.oauth_cfg(),
                tools: Vec::new(),
                resources: Vec::new(),
                prompts: Vec::new(),
            }
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
            Self {
                name: cfg.name.clone(),
                transport: Transport::Stdio {
                    child,
                    stdin,
                    reader: tokio::io::BufReader::new(stdout),
                    next_id: 0,
                },
                oauth: None,
                tools: Vec::new(),
                resources: Vec::new(),
                prompts: Vec::new(),
            }
        };
        let proto = match s.transport {
            Transport::Http { .. } => "2025-03-26",
            Transport::Stdio { .. } => "2024-11-05",
        };
        let init = s
            .request_t(
                REQUEST_TIMEOUT,
                "initialize",
                json!({
                    "protocolVersion": proto,
                    "capabilities": {},
                    "clientInfo": {"name": "hi-derola", "version": "0.1.0"}
                }),
            )
            .await?;
        let caps = init["capabilities"].clone();
        s.notify_t("notifications/initialized").await?;
        // tools: strict when the server declares the capability (keeps the oauth 401
        // hints), tolerated otherwise so tools-less servers still connect
        let tools_cap = has_cap(&caps, "tools");
        match s.list_page("tools/list", "tools").await {
            Ok(items) => {
                for t in &items {
                    s.tools.push(McpTool {
                        name: t["name"].as_str().unwrap_or("").to_string(),
                        description: t["description"].as_str().unwrap_or("").to_string(),
                        schema: t["inputSchema"].clone(),
                    });
                }
            }
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

    async fn request_t(&mut self, timeout: Duration, method: &str, params: Value) -> Result<Value> {
        match &mut self.transport {
            Transport::Stdio { stdin, reader, next_id, .. } => {
                *next_id += 1;
                let id = *next_id;
                let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
                let mut line = msg.to_string();
                line.push('\n');
                stdin.write_all(line.as_bytes()).await?;
                stdin.flush().await?;
                match tokio::time::timeout(timeout, wait_response(reader, id, &self.name)).await {
                    Ok(r) => r,
                    Err(_) => bail!("mcp {}: {method} timeout", self.name),
                }
            }
            Transport::Http { url, http, session, headers, next_id } => {
                *next_id += 1;
                let id = *next_id;
                let body =
                    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
                let mut resp = None;
                for attempt in 0..2 {
                    let token = crate::mcpauth::bearer(
                        &self.name,
                        url,
                        self.oauth.as_ref(),
                        http,
                        attempt > 0,
                    )
                    .await?;
                    let mut req = http
                        .post(url.as_str())
                        .header("Content-Type", "application/json")
                        .header("Accept", "application/json, text/event-stream");
                    if let Some(t) = &token {
                        req = req.header("Authorization", format!("Bearer {t}"));
                    }
                    if let Some(sid) = session.as_ref() {
                        req = req.header("mcp-session-id", sid);
                    }
                    for (k, v) in headers.iter() {
                        req = req.header(k.as_str(), v.as_str());
                    }
                    let fut = req.body(body.to_string()).send();
                    let r = match tokio::time::timeout(timeout, fut).await {
                        Err(_) => bail!("mcp {}: {method} timeout", self.name),
                        Ok(r) => r?,
                    };
                    if r.status().as_u16() == 401 && attempt == 0 {
                        continue;
                    }
                    if r.status().as_u16() == 401 {
                        let hint = if self.oauth.is_some() {
                            format!(" — run /mcpauth {}", self.name)
                        } else {
                            String::new()
                        };
                        bail!("mcp {}: 401 unauthorized{hint}", self.name);
                    }
                    resp = Some(r);
                    break;
                }
                let resp = resp.context("mcp: no response")?;
                let status = resp.status();
                if !status.is_success() {
                    let text = resp.text().await.unwrap_or_default();
                    bail!(
                        "mcp {}: {} {}",
                        self.name,
                        status,
                        crate::provider::truncate(&text).trim()
                    );
                }
                if method == "initialize" {
                    if let Some(sid) = resp
                        .headers()
                        .get("mcp-session-id")
                        .and_then(|v| v.to_str().ok())
                    {
                        *session = Some(sid.to_string());
                    }
                }
                let ctype = resp
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                if ctype.contains("text/event-stream") {
                    sse_response(resp, id, &self.name).await
                } else {
                    let text = resp.text().await?;
                    let v: Value = serde_json::from_str(&text)
                        .map_err(|e| anyhow::anyhow!("mcp {}: bad json: {e}", self.name))?;
                    extract_result(v, &self.name)
                }
            }
        }
    }

    async fn notify_t(&mut self, method: &str) -> Result<()> {
        match &mut self.transport {
            Transport::Stdio { stdin, .. } => {
                let msg = json!({"jsonrpc": "2.0", "method": method});
                let mut line = msg.to_string();
                line.push('\n');
                stdin.write_all(line.as_bytes()).await?;
                stdin.flush().await?;
                Ok(())
            }
            Transport::Http { url, http, session, headers, .. } => {
                let body = json!({"jsonrpc": "2.0", "method": method});
                let token = crate::mcpauth::bearer(&self.name, url, self.oauth.as_ref(), http, false)
                    .await
                    .ok()
                    .flatten();
                let mut req = http
                    .post(url.as_str())
                    .header("Content-Type", "application/json")
                    .header("Accept", "application/json, text/event-stream");
                if let Some(t) = &token {
                    req = req.header("Authorization", format!("Bearer {t}"));
                }
                if let Some(sid) = session.as_ref() {
                    req = req.header("mcp-session-id", sid);
                }
                for (k, v) in headers.iter() {
                    req = req.header(k.as_str(), v.as_str());
                }
                let fut = req.body(body.to_string()).send();
                let _ = tokio::time::timeout(REQUEST_TIMEOUT, fut).await;
                Ok(())
            }
        }
    }
}

pub struct McpClient {
    servers: Mutex<Vec<McpServer>>,
}

pub type McpSlot = std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<McpClient>>>>;

pub async fn connect_all(cfgs: &[McpConfig]) -> (Option<std::sync::Arc<McpClient>>, Vec<String>) {
    let mut servers = Vec::new();
    let mut logs = Vec::new();
    for c in cfgs {
        match tokio::time::timeout(CONNECT_TIMEOUT, McpServer::connect(c)).await {
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
        }))
    };
    (client, logs)
}

pub async fn reconnect_one(slot: &McpSlot, cfgs: &[McpConfig], name: &str) -> Vec<String> {
    let Some(cfg) = cfgs.iter().find(|c| c.name == name) else {
        return vec![format!("mcp {name}: not in config")];
    };
    let existing = slot.lock().unwrap().clone();
    if let Some(client) = existing {
        let mut logs = Vec::new();
        match tokio::time::timeout(CONNECT_TIMEOUT, client.replace(cfg)).await {
            Ok(Ok(sum)) => logs.push(format!("mcp {name}: connected ({sum})")),
            Ok(Err(e)) => logs.push(format!("mcp {name}: {e:#}")),
            Err(_) => logs.push(format!("mcp {name}: connect timeout")),
        }
        logs
    } else {
        let (client, logs) = connect_all(cfgs).await;
        *slot.lock().unwrap() = client;
        logs
    }
}

impl McpClient {
    async fn replace(&self, cfg: &McpConfig) -> Result<String> {
        let mut servers = self.servers.lock().await;
        servers.retain(|s| s.name != cfg.name);
        let s = McpServer::connect(cfg).await?;
        let sum = s.summary();
        servers.push(s);
        Ok(sum)
    }

    pub async fn specs(&self) -> Vec<ToolSpec> {
        let servers = self.servers.lock().await;
        let mut out = Vec::new();
        for s in servers.iter() {
            for t in &s.tools {
                out.push(ToolSpec {
                    name: format!("mcp__{}__{}", s.name, t.name),
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
        let Some(s) = servers.iter_mut().find(|s| s.name == server) else {
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
        let servers = self.servers.lock().await;
        let mut out = Vec::new();
        for s in servers.iter() {
            for r in &s.resources {
                out.push(McpResourceInfo {
                    server: s.name.clone(),
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
        let Some(s) = servers.iter_mut().find(|s| s.name == server) else {
            bail!("mcp server not found: {server}");
        };
        s.read_resource(uri).await
    }

    /// all prompts across servers, in connect order
    pub async fn prompts(&self) -> Vec<McpPromptInfo> {
        let servers = self.servers.lock().await;
        let mut out = Vec::new();
        for s in servers.iter() {
            for p in &s.prompts {
                out.push(McpPromptInfo {
                    server: s.name.clone(),
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
    pub async fn get_prompt(&self, server: &str, name: &str, args: &Value) -> Result<Vec<(String, String)>> {
        let mut servers = self.servers.lock().await;
        let Some(s) = servers.iter_mut().find(|s| s.name == server) else {
            bail!("mcp server not found: {server}");
        };
        s.get_prompt(name, args).await
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

    /// spawned as a child process by resources_and_prompts_roundtrip; acts as a
    /// fake stdio mcp server. plain test run: no-op.
    #[test]
    fn fake_mcp_child() {
        if std::env::var("HI_DEROLA_FAKE_MCP").is_err() {
            return;
        }
        let mode = std::env::var("HI_DEROLA_FAKE_MCP").unwrap_or_default();
        use std::io::{BufRead, Write};
        let stdin = std::io::stdin();
        let mut out = std::io::stdout().lock();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let Some(id) = v.get("id").and_then(|x| x.as_u64()) else {
                continue;
            };
            let method = v["method"].as_str().unwrap_or("");
            let result = match method {
                "initialize" => {
                    if mode == "min" {
                        json!({"capabilities": {}})
                    } else {
                        json!({"capabilities": {"tools": {}, "resources": {}, "prompts": {}}})
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
                    json!({"tools": [{"name": "ping", "description": "d", "inputSchema": {"type": "object"}}]})
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
                _ => json!({}),
            };
            let resp = json!({"jsonrpc": "2.0", "id": id, "result": result});
            writeln!(out, "{resp}").unwrap();
            out.flush().unwrap();
        }
    }

    fn child_cfg(mode: &str) -> McpConfig {
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
        }
    }

    #[tokio::test]
    async fn resources_and_prompts_roundtrip() {
        let cfg = child_cfg("1");
        let mut s = McpServer::connect(&cfg).await.unwrap();
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
    async fn server_without_caps_still_connects() {
        let cfg = child_cfg("min");
        let s = McpServer::connect(&cfg).await.unwrap();
        assert!(s.tools.is_empty(), "tools/list error tolerated without cap");
        assert!(s.resources.is_empty());
        assert!(s.prompts.is_empty());
        assert_eq!(s.summary(), "0 tools");
    }
}
