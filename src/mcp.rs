use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;

use crate::config::McpConfig;
use crate::provider::ToolSpec;

const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

struct McpTool {
    name: String,
    description: String,
    schema: Value,
}

pub struct McpServer {
    name: String,
    child: Child,
    stdin: ChildStdin,
    reader: tokio::io::BufReader<ChildStdout>,
    next_id: u64,
    tools: Vec<McpTool>,
}

impl Drop for McpServer {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

impl McpServer {
    async fn connect(cfg: &McpConfig) -> Result<Self> {
        let mut child = tokio::process::Command::new(&cfg.command)
            .args(&cfg.args)
            .envs(&cfg.env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .with_context(|| format!("mcp {}: spawn {}", cfg.name, cfg.command))?;
        let stdin = child.stdin.take().context("mcp: no stdin")?;
        let stdout = child.stdout.take().context("mcp: no stdout")?;
        let mut s = Self {
            name: cfg.name.clone(),
            child,
            stdin,
            reader: tokio::io::BufReader::new(stdout),
            next_id: 1,
            tools: Vec::new(),
        };
        s.request("initialize", json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "hi-derola", "version": "0.1.0"}
        }))
        .await?;
        s.notify("notifications/initialized").await?;
        let res = s.request("tools/list", json!({})).await?;
        for t in res["tools"].as_array().into_iter().flatten() {
            s.tools.push(McpTool {
                name: t["name"].as_str().unwrap_or("").to_string(),
                description: t["description"].as_str().unwrap_or("").to_string(),
                schema: t["inputSchema"].clone(),
            });
        }
        s.tools.retain(|t| !t.name.is_empty());
        Ok(s)
    }

    async fn send(&mut self, msg: &Value) -> Result<()> {
        let mut line = msg.to_string();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn notify(&mut self, method: &str) -> Result<()> {
        self.send(&json!({"jsonrpc": "2.0", "method": method})).await
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await?;
        match tokio::time::timeout(REQUEST_TIMEOUT, self.wait_response(id)).await {
            Ok(r) => r,
            Err(_) => bail!("mcp {}: {method} timeout", self.name),
        }
    }

    async fn wait_response(&mut self, id: u64) -> Result<Value> {
        loop {
            let mut line = String::new();
            let n = self.reader.read_line(&mut line).await?;
            if n == 0 {
                bail!("mcp {}: server closed", self.name);
            }
            let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            if v.get("id").and_then(|x| x.as_u64()) == Some(id) {
                if let Some(e) = v.get("error") {
                    bail!(
                        "mcp {}: {}",
                        self.name,
                        e["message"].as_str().unwrap_or("error")
                    );
                }
                return Ok(v["result"].clone());
            }
        }
    }
}

pub struct McpClient {
    servers: Mutex<Vec<McpServer>>,
}

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
            .request("tools/call", json!({"name": tool, "arguments": arguments}))
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
