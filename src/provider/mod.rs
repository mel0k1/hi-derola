pub mod anthropic;
pub mod openai;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use std::sync::Arc;

use crate::chat::Message;

#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &'static str;

    async fn complete(
        &self,
        system: &str,
        messages: &[Message],
        model: &str,
        max_tokens: Option<u32>,
    ) -> Result<String>;
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

pub async fn send_json(
    http: &reqwest::Client,
    url: String,
    headers: Vec<(&'static str, String)>,
    body: serde_json::Value,
) -> Result<String> {
    let mut req = http.post(url).json(&body);
    for (k, v) in headers {
        req = req.header(k, v);
    }
    let resp = req.send().await.context("request failed")?;
    let status = resp.status();
    let text = resp.text().await?;
    if !status.is_success() {
        bail!("{} {}", status, truncate(&text));
    }
    Ok(text)
}
