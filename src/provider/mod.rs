pub mod anthropic;
pub mod openai;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use std::sync::Arc;
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

pub async fn send(
    http: &reqwest::Client,
    url: String,
    headers: Vec<(&'static str, String)>,
    body: serde_json::Value,
) -> Result<reqwest::Response> {
    let mut req = http.post(url).json(&body);
    for (k, v) in headers {
        req = req.header(k, v);
    }
    let resp = req.send().await.context("request failed")?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        bail!("{} {}", status, truncate(&text));
    }
    Ok(resp)
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
