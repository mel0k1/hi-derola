pub mod anthropic;
pub mod openai;

use anyhow::{bail, Result};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::UnboundedSender;

use crate::chat::Message;

pub enum ApiEvent {
    Chunk(String),
    Reasoning(String),
    Usage { input: u64, output: u64 },
    Note(String),
    Done(String),
    Failed(String),
}

pub struct ChatRequest {
    pub system: String,
    pub messages: Vec<Message>,
    pub model: String,
    pub max_tokens: Option<u32>,
    pub stream: bool,
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &'static str;

    async fn chat(&self, req: ChatRequest, tx: UnboundedSender<ApiEvent>) -> Result<()>;
}

pub fn build(kind: &str, base_url: Option<String>, api_key: String) -> Result<Arc<dyn Provider>> {
    match kind {
        "openai" => Ok(Arc::new(openai::OpenAi::new(base_url, api_key))),
        "anthropic" => Ok(Arc::new(anthropic::Anthropic::new(base_url, api_key))),
        other => bail!("unknown provider type: {other}"),
    }
}

pub fn truncate(s: &str) -> String {
    let end = s
        .char_indices()
        .nth(300)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    let mut out = s[..end].to_string();
    out.push('\n');
    out
}

const RETRY_MAX_ATTEMPTS: usize = 3;
const RETRY_INITIAL_MS: u64 = 2000;
const RETRY_CAP_MS: u64 = 10_000;

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
            Ok(resp) if attempt < RETRY_MAX_ATTEMPTS && retryable_status(resp.status().as_u16()) => {
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
