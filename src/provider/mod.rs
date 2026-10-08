pub mod anthropic;
pub mod openai;

use anyhow::{bail, Result};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::UnboundedSender;

use crate::chat::{Message, ToolCall};

#[derive(Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

pub struct Reply {
    pub text: String,
    pub calls: Vec<ToolCall>,
    /// the provider cut the response short (finish_reason length / stop_reason max_tokens)
    pub truncated: bool,
}

pub enum ApiEvent {
    Chunk(String),
    Reasoning(String),
    Usage {
        input: u64,
        output: u64,
        cached: u64,
    },
    Note(String),
    Tool {
        name: String,
        detail: String,
        diff: Vec<crate::diff::Row>,
        paths: Vec<String>,
    },
    Confirm {
        name: String,
        args: String,
        rx: tokio::sync::oneshot::Sender<ConfirmReply>,
    },
    Ask {
        name: String,
        args: String,
        rx: tokio::sync::oneshot::Sender<String>,
    },
    Todo(String),
    /// plan mode flipped at runtime (plan_exit approval): true = on, false = off
    Plan(bool),
    BgOut {
        id: String,
        chunk: String,
    },
    /// a crew member spoke (multi-agent session); the frontends route it to
    /// the crew panel / log
    Crew {
        id: String,
        author: String,
        role: String,
        content: String,
    },
    /// streaming delta from a crew member's in-progress reply
    CrewChunk {
        id: String,
        author: String,
        delta: String,
    },
    /// crew finished (status done) — the frontends ring a bell / notify
    Bell,
    Done {
        text: String,
        messages: Vec<Message>,
    },
    Failed(String),
    Wake,
    /// submit text as a user message and start a run (e.g. an mcp prompt)
    Submit(String),
}

#[derive(Debug, Clone, Default)]
pub struct ConfirmReply {
    pub approved: bool,
    pub feedback: String,
    /// approve and save a wildcard allow-rule for this tool into the config
    pub always: bool,
}

#[derive(Clone)]
pub struct ChatRequest {
    pub system: String,
    pub messages: Vec<Message>,
    pub model: String,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub stream: bool,
    pub tools: Vec<ToolSpec>,
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &'static str;

    async fn chat(&self, req: &ChatRequest, tx: &UnboundedSender<ApiEvent>) -> Result<Reply>;
}

pub fn build(kind: &str, base_url: Option<String>, api_key: String) -> Result<Arc<dyn Provider>> {
    match kind {
        "openai" => Ok(Arc::new(openai::OpenAi::new(base_url, api_key))),
        "anthropic" => Ok(Arc::new(anthropic::Anthropic::new(base_url, api_key))),
        other => bail!("unknown provider type: {other}"),
    }
}

pub async fn list_models(kind: &str, base_url: Option<&str>, api_key: &str) -> Result<Vec<String>> {
    let http = reqwest::Client::new();
    let (url, headers): (String, Vec<(&'static str, String)>) = match kind {
        "anthropic" => {
            let base = base_url
                .map(|s| s.trim_end_matches('/').to_string())
                .unwrap_or_else(|| "https://api.anthropic.com".into());
            (
                format!("{base}/v1/models?limit=1000"),
                vec![
                    ("x-api-key", api_key.to_string()),
                    ("anthropic-version", "2023-06-01".into()),
                ],
            )
        }
        _ => {
            let base = base_url
                .map(|s| s.trim_end_matches('/').to_string())
                .unwrap_or_else(|| "https://api.openai.com/v1".into());
            (
                format!("{base}/models"),
                vec![("Authorization", format!("Bearer {api_key}"))],
            )
        }
    };
    let mut req = http.get(&url);
    for (k, v) in headers {
        req = req.header(k, v);
    }
    req = req.header("User-Agent", "hi-derola");
    let resp = req.send().await?;
    let status = resp.status();
    let text = resp.text().await?;
    if !status.is_success() {
        bail!("{} {}", status, truncate(&text));
    }
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| anyhow::anyhow!("bad response: {e}"))?;
    let mut out = Vec::new();
    if let Some(items) = v["data"].as_array() {
        for it in items {
            let id = it["id"]
                .as_str()
                .map(|s| s.to_string())
                .or_else(|| it.as_str().map(|s| s.to_string()));
            if let Some(id) = id {
                if !id.is_empty() {
                    out.push(id);
                }
            }
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

pub fn truncate(s: &str) -> String {
    let end = s.char_indices().nth(300).map(|(i, _)| i).unwrap_or(s.len());
    let mut out = s[..end].to_string();
    out.push('\n');
    out
}

const RETRY_MAX_ATTEMPTS: usize = 10;
const RETRY_INITIAL_MS: u64 = 2000;
const RETRY_CAP_MS: u64 = 20_000;

fn retryable_status(code: u16) -> bool {
    matches!(code, 408 | 429 | 500 | 502 | 503 | 504 | 529)
}

fn retryable_err(e: &reqwest::Error) -> bool {
    e.is_connect() || e.is_timeout() || e.is_request()
}

fn retry_after_ms(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    if let Some(v) = headers
        .get("retry-after-ms")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<f64>().ok())
    {
        return Some(v.max(0.0) as u64);
    }
    if let Some(v) = headers.get("retry-after").and_then(|v| v.to_str().ok()) {
        if let Ok(s) = v.parse::<f64>() {
            return Some((s.max(0.0) * 1000.0) as u64);
        }
    }
    None
}

fn backoff_ms(attempt: usize) -> u64 {
    let base = RETRY_INITIAL_MS << (attempt - 1).min(3);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let jitter = base as f64 * 0.25 * (nanos % 1000) as f64 / 1000.0;
    ((base as f64 + jitter) as u64).min(RETRY_CAP_MS)
}

pub async fn send(
    http: &reqwest::Client,
    url: String,
    headers: Vec<(&'static str, String)>,
    body: serde_json::Value,
    tx: &UnboundedSender<ApiEvent>,
) -> Result<reqwest::Response> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let mut req = http.post(&url).json(&body);
        for (k, v) in &headers {
            req = req.header(*k, v);
        }
        let outcome = req.send().await;
        match outcome {
            Ok(resp) if resp.status().is_success() => return Ok(resp),
            Ok(resp)
                if attempt < RETRY_MAX_ATTEMPTS && retryable_status(resp.status().as_u16()) =>
            {
                let status = resp.status();
                let wait = retry_after_ms(resp.headers()).unwrap_or_else(|| backoff_ms(attempt));
                let _ = tx.send(ApiEvent::Note(format!(
                    "retrying {} in {:.1}s (attempt {}/{})",
                    status,
                    wait as f64 / 1000.0,
                    attempt + 1,
                    RETRY_MAX_ATTEMPTS
                )));
                tokio::time::sleep(Duration::from_millis(wait)).await;
            }
            Ok(resp) => {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                bail!("{} {}", status, truncate(&text));
            }
            Err(e) if attempt < RETRY_MAX_ATTEMPTS && retryable_err(&e) => {
                let wait = backoff_ms(attempt);
                let _ = tx.send(ApiEvent::Note(format!(
                    "retrying {e} in {:.1}s (attempt {}/{})",
                    wait as f64 / 1000.0,
                    attempt + 1,
                    RETRY_MAX_ATTEMPTS
                )));
                tokio::time::sleep(Duration::from_millis(wait)).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
}

pub async fn sse_lines(
    mut resp: reqwest::Response,
    mut on_line: impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    let mut buf = String::new();
    loop {
        let Some(bytes) = resp.chunk().await? else {
            break;
        };
        buf.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(pos) = buf.find('\n') {
            let line: String = buf.drain(..pos + 1).collect();
            on_line(line.trim_end())?;
        }
    }
    if !buf.is_empty() {
        on_line(buf.trim_end())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use tokio::sync::mpsc::unbounded_channel;

    /// one-shot HTTP/1.1 server: serves one canned response per connection
    fn spawn_server(responses: Vec<String>) -> std::net::SocketAddr {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for resp in responses {
                let Ok((mut sock, _)) = listener.accept() else {
                    break;
                };
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf);
                let _ = sock.write_all(resp.as_bytes());
                let _ = sock.flush();
            }
        });
        addr
    }

    #[tokio::test]
    async fn send_retries_retryable_status_then_succeeds() {
        let body = "{\"ok\":true}";
        let addr = spawn_server(vec![
            "HTTP/1.1 429 Too Many Requests\r\nconnection: close\r\nretry-after: 0.05\r\ncontent-length: 0\r\n\r\n"
                .into(),
            "HTTP/1.1 503 Service Unavailable\r\nconnection: close\r\nretry-after-ms: 50\r\ncontent-length: 0\r\n\r\n"
                .into(),
            format!(
                "HTTP/1.1 200 OK\r\nconnection: close\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            ),
        ]);
        let http = reqwest::Client::new();
        let (tx, mut rx) = unbounded_channel();
        let started = std::time::Instant::now();
        let resp = send(
            &http,
            format!("http://{addr}/chat"),
            vec![],
            serde_json::json!({"x": 1}),
            &tx,
        )
        .await
        .unwrap();
        assert!(resp.status().is_success());
        // both retry-after hints (50 ms each) were honored
        assert!(started.elapsed() >= std::time::Duration::from_millis(100));
        drop(tx);
        let mut notes = Vec::new();
        while let Some(ev) = rx.recv().await {
            if let ApiEvent::Note(n) = ev {
                notes.push(n);
            }
        }
        assert_eq!(notes.len(), 2);
        assert!(notes[0].contains("429"), "first note: {}", notes[0]);
        assert!(notes[1].contains("503"), "second note: {}", notes[1]);
    }

    #[tokio::test]
    async fn send_does_not_retry_client_errors() {
        let addr = spawn_server(vec![
            "HTTP/1.1 400 Bad Request\r\nconnection: close\r\ncontent-length: 11\r\n\r\nbad request".into(),
        ]);
        let http = reqwest::Client::new();
        let (tx, _rx) = unbounded_channel();
        let err = send(
            &http,
            format!("http://{addr}/chat"),
            vec![],
            serde_json::json!({}),
            &tx,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("400"), "error: {err}");
    }
}
