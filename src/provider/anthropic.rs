use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use super::{sse_lines, send, ApiEvent, ChatRequest, Provider};

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

#[async_trait]
impl Provider for Anthropic {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    async fn chat(&self, req: ChatRequest, tx: UnboundedSender<ApiEvent>) -> Result<()> {
        let mut msgs = Vec::new();
        for m in &req.messages {
            msgs.push(json!({"role": m.role.as_str(), "content": m.content}));
        }
        let body = json!({
            "model": req.model,
            "max_tokens": req.max_tokens.unwrap_or(4096),
            "system": req.system,
            "messages": msgs,
            "stream": req.stream,
        });
        let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        let resp = send(
            &self.http,
            url,
            vec![
                ("x-api-key", self.api_key.clone()),
                ("anthropic-version", "2023-06-01".into()),
            ],
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
                let Ok(v) = serde_json::from_str::<Value>(data) else {
                    return Ok(());
                };
                match v["type"].as_str() {
                    Some("content_block_delta") => {
                        match v["delta"]["type"].as_str() {
                            Some("text_delta") => {
                                if let Some(c) = v["delta"]["text"].as_str() {
                                    if !c.is_empty() {
                                        full.push_str(c);
                                        tx.send(ApiEvent::Chunk(c.to_string()))
                                            .map_err(|_| anyhow::anyhow!("closed"))?;
                                    }
                                }
                            }
                            Some("thinking_delta") => {
                                if let Some(t) = v["delta"]["thinking"].as_str() {
                                    if !t.is_empty() {
                                        tx.send(ApiEvent::Reasoning(t.to_string()))
                                            .map_err(|_| anyhow::anyhow!("closed"))?;
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    Some("message_start") => {
                        tx.send(ApiEvent::Usage {
                            input: v["message"]["usage"]["input_tokens"].as_u64().unwrap_or(0),
                            output: 0,
                        })
                        .map_err(|_| anyhow::anyhow!("closed"))?;
                    }
                    Some("message_delta") => {
                        tx.send(ApiEvent::Usage {
                            input: 0,
                            output: v["usage"]["output_tokens"].as_u64().unwrap_or(0),
                        })
                        .map_err(|_| anyhow::anyhow!("closed"))?;
                    }
                    _ => {}
                }
                Ok(())
            })
            .await?;
        } else {
            let text = resp.text().await?;
            let v: Value = serde_json::from_str(&text).map_err(|e| anyhow::anyhow!("bad response: {e}"))?;
            for block in v["content"].as_array().into_iter().flatten() {
                if let Some(t) = block["text"].as_str() {
                    full.push_str(t);
                }
            }
            if !full.is_empty() {
                let _ = tx.send(ApiEvent::Chunk(full.clone()));
            }
            tx.send(ApiEvent::Usage {
                input: v["usage"]["input_tokens"].as_u64().unwrap_or(0),
                output: v["usage"]["output_tokens"].as_u64().unwrap_or(0),
            })
            .map_err(|_| anyhow::anyhow!("closed"))?;
        }
        let _ = tx.send(ApiEvent::Done(full));
        Ok(())
    }
}
