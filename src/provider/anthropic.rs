use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use super::{send_json, Provider};
use crate::chat::Message;

pub struct Anthropic {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl Anthropic {
    pub fn new(base_url: Option<String>, api_key: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.unwrap_or_else(|| "https://api.anthropic.com".into()),
            api_key,
        }
    }
}

#[derive(Deserialize)]
struct Response {
    content: Vec<Block>,
}

#[derive(Deserialize)]
struct Block {
    #[serde(default)]
    text: Option<String>,
}

#[async_trait]
impl Provider for Anthropic {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    async fn complete(
        &self,
        system: &str,
        messages: &[Message],
        model: &str,
        max_tokens: Option<u32>,
    ) -> Result<String> {
        let mut msgs = Vec::new();
        for m in messages {
            msgs.push(json!({"role": m.role.as_str(), "content": m.content}));
        }
        let body = json!({
            "model": model,
            "max_tokens": max_tokens.unwrap_or(4096),
            "system": system,
            "messages": msgs,
        });
        let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        let text = send_json(
            &self.http,
            url,
            vec![
                ("x-api-key", self.api_key.clone()),
                ("anthropic-version", "2023-06-01".into()),
            ],
            body,
        )
        .await?;
        let data: Response = serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("bad response: {e}"))?;
        let out: String = data
            .content
            .into_iter()
            .filter_map(|b| b.text)
            .collect::<Vec<_>>()
            .join("");
        Ok(out)
    }
}
