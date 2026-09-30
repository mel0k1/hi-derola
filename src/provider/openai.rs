use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use super::{sse_lines, send, ApiEvent, ChatRequest, Provider};

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

#[async_trait]
impl Provider for OpenAi {
    fn name(&self) -> &'static str {
        "openai"
    }

    async fn chat(&self, req: ChatRequest, tx: UnboundedSender<ApiEvent>) -> Result<()> {
        let mut msgs = vec![json!({"role": "system", "content": req.system})];
        for m in &req.messages {
            msgs.push(json!({"role": m.role.as_str(), "content": m.content}));
        }
        let mut body = json!({"model": req.model, "messages": msgs, "stream": req.stream});
        if let Some(t) = req.max_tokens {
            body["max_tokens"] = json!(t);
        }
        if req.stream {
            body["stream_options"] = json!({"include_usage": true});
        }
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let resp = send(
            &self.http,
            url,
            vec![("Authorization", format!("Bearer {}", self.api_key))],
            body,
            &tx,
        )
        .await?;

        let mut full = String::new();
        if req.stream {
            sse_lines(resp, |line| {
                let Some(data) = line.strip_prefix("data:") else {
                    return Ok(());
                };
                let data = data.trim();
                if data == "[DONE]" {
                    return Ok(());
                }
                let Ok(v) = serde_json::from_str::<Value>(data) else {
                    return Ok(());
                };
                if let Some(c) = v["choices"][0]["delta"]["content"].as_str() {
                    if !c.is_empty() {
                        full.push_str(c);
                        tx.send(ApiEvent::Chunk(c.to_string()))
                            .map_err(|_| anyhow::anyhow!("closed"))?;
                    }
                }
                let d = &v["choices"][0]["delta"];
                if let Some(r) = d["reasoning_content"]
                    .as_str()
                    .or_else(|| d["reasoning"].as_str())
                {
                    if !r.is_empty() {
                        tx.send(ApiEvent::Reasoning(r.to_string()))
                            .map_err(|_| anyhow::anyhow!("closed"))?;
                    }
                }
                if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
                    tx.send(ApiEvent::Usage {
                        input: u["prompt_tokens"].as_u64().unwrap_or(0),
                        output: u["completion_tokens"].as_u64().unwrap_or(0),
                    })
                    .map_err(|_| anyhow::anyhow!("closed"))?;
                }
                Ok(())
            })
            .await?;
        } else {
            let text = resp.text().await?;
            let v: Value = serde_json::from_str(&text).map_err(|e| anyhow::anyhow!("bad response: {e}"))?;
            if let Some(c) = v["choices"][0]["message"]["content"].as_str() {
                full = c.to_string();
                let _ = tx.send(ApiEvent::Chunk(full.clone()));
            }
            if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
                tx.send(ApiEvent::Usage {
                    input: u["prompt_tokens"].as_u64().unwrap_or(0),
                    output: u["completion_tokens"].as_u64().unwrap_or(0),
                })
                .map_err(|_| anyhow::anyhow!("closed"))?;
            }
        }
        let _ = tx.send(ApiEvent::Done(full));
        Ok(())
    }
}
