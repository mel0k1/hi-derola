use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use super::{send_json, Provider};
use crate::chat::Message;

pub struct OpenAi {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl OpenAi {
    pub fn new(base_url: Option<String>, api_key: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.unwrap_or_else(|| "https://api.openai.com/v1".into()),
            api_key,
        }
    }
}

#[derive(Deserialize)]
struct Response {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Deserialize)]
struct ChoiceMessage {
    #[serde(default)]
    content: Option<String>,
}

#[async_trait]
impl Provider for OpenAi {
    fn name(&self) -> &'static str {
        "openai"
    }

    async fn complete(
        &self,
        system: &str,
        messages: &[Message],
        model: &str,
        max_tokens: Option<u32>,
    ) -> Result<String> {
        let mut msgs = vec![json!({"role": "system", "content": system})];
        for m in messages {
            msgs.push(json!({"role": m.role.as_str(), "content": m.content}));
        }
        let mut body = json!({"model": model, "messages": msgs});
        if let Some(t) = max_tokens {
            body["max_tokens"] = json!(t);
        }
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let text = send_json(
            &self.http,
            url,
            vec![("Authorization", format!("Bearer {}", self.api_key))],
            body,
        )
        .await?;
        let data: Response = serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("bad response: {e}"))?;
        Ok(data
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .unwrap_or_default())
    }
}
