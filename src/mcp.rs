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
                tools: Vec::new(),
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
                tools: Vec::new(),
            }
        };
        let proto = match s.transport {
            Transport::Http { .. } => "2025-03-26",
            Transport::Stdio { .. } => "2024-11-05",
        };
        s.request_t(
            REQUEST_TIMEOUT,
            "initialize",
            json!({
                "protocolVersion": proto,
                "capabilities": {},
                "clientInfo": {"name": "hi-derola", "version": "0.1.0"}
            }),
        )
        .await?;
        s.notify_t("notifications/initialized").await?;
        let res = s.request_t(REQUEST_TIMEOUT, "tools/list", json!({})).await?;
        for t in res["tools"].as_array().into_iter().flatten() {
            s.tools.push(McpTool {
                name: t["name"].as_str().unwrap_or("").to_string(),
                description: t["description"].as_str().unwrap_or("").to_string(),
                schema: t["inputSchema"].clone(),
            });
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
                let mut req = http
                    .post(url.as_str())
                    .header("Content-Type", "application/json")
                    .header("Accept", "application/json, text/event-stream");
                if let Some(sid) = session.as_ref() {
                    req = req.header("mcp-session-id", sid);
                }
                for (k, v) in headers.iter() {
                    req = req.header(k.as_str(), v.as_str());
                }
                let fut = req.body(body.to_string()).send();
                let resp = match tokio::time::timeout(timeout, fut).await {
                    Err(_) => bail!("mcp {}: {method} timeout", self.name),
                    Ok(r) => r?,
                };
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
                let mut req = http
                    .post(url.as_str())
                    .header("Content-Type", "application/json")
                    .header("Accept", "application/json, text/event-stream");
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
                logs.push(format!("mcp {}: connected ({} tools)", c.name, s.tools.len()));
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

impl McpClient {
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
}
