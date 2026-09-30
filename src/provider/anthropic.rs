use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use super::{sse_lines, send, ApiEvent, ChatRequest, Provider, Reply};
use crate::chat::{Message, Role, ToolCall};

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

fn is_tool_result_user(v: &Value) -> bool {
    v["role"].as_str() == Some("user")
        && v["content"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|b| b["type"].as_str())
            == Some("tool_result")
}

fn conv_msgs(messages: &[Message]) -> Vec<Value> {
    let mut out = Vec::new();
    for m in messages {
        match m.role {
            Role::Tool => {
                let block = json!({"type": "tool_result", "tool_use_id": m.tool_call_id, "content": m.content});
                match out.last_mut() {
                    Some(last) if is_tool_result_user(last) => {
                        if let Some(a) = last["content"].as_array_mut() {
                            a.push(block);
                        }
                    }
                    _ => out.push(json!({"role": "user", "content": [block]})),
                }
            }
            Role::Assistant if !m.tool_calls.is_empty() => {
                let mut content = Vec::new();
                if !m.content.is_empty() {
                    content.push(json!({"type": "text", "text": m.content}));
                }
                for c in &m.tool_calls {
                    let input: Value = serde_json::from_str(&c.args).unwrap_or(json!({}));
                    content.push(json!({"type": "tool_use", "id": c.id, "name": c.name, "input": input}));
                }
                out.push(json!({"role": "assistant", "content": content}));
            }
            _ => out.push(json!({"role": m.role.as_str(), "content": m.content})),
        }
    }
    out
}

#[async_trait]
impl Provider for Anthropic {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    async fn chat(&self, req: &ChatRequest, tx: &UnboundedSender<ApiEvent>) -> Result<Reply> {
        let mut body = json!({
            "model": req.model,
            "max_tokens": req.max_tokens.unwrap_or(4096),
            "system": req.system,
            "messages": conv_msgs(&req.messages),
            "stream": req.stream,
        });
        if let Some(v) = req.temperature {
            body["temperature"] = json!(v);
        } else if let Some(v) = req.top_p {
            body["top_p"] = json!(v);
        }
        if !req.tools.is_empty() {
            let tools: Vec<Value> = req
                .tools
                .iter()
                .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.parameters}))
                .collect();
            body["tools"] = tools.into();
        }
        let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        let resp = send(
            &self.http,
            url,
            vec![
                ("x-api-key", self.api_key.clone()),
                ("anthropic-version", "2023-06-01".into()),
            ],
            body,
            tx,
        )
        .await?;

        let mut full = String::new();
        let mut calls: Vec<ToolCall> = Vec::new();
        if req.stream {
            let mut pending: Vec<(u64, String, String, String)> = Vec::new();
            sse_lines(resp, |line| {
                let Some(data) = line.strip_prefix("data:") else {
                    return Ok(());
                };
                let data = data.trim();
                let Ok(v) = serde_json::from_str::<Value>(data) else {
                    return Ok(());
                };
                match v["type"].as_str() {
                    Some("content_block_start") => {
                        let b = &v["content_block"];
                        if b["type"].as_str() == Some("tool_use") {
                            pending.push((
                                v["index"].as_u64().unwrap_or(0),
                                b["id"].as_str().unwrap_or("").into(),
                                b["name"].as_str().unwrap_or("").into(),
                                String::new(),
                            ));
                        }
                    }
                    Some("content_block_delta") => match v["delta"]["type"].as_str() {
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
                        Some("input_json_delta") => {
                            let idx = v["index"].as_u64().unwrap_or(0);
                            if let Some(p) = v["delta"]["partial_json"].as_str() {
                                if let Some(e) = pending.iter_mut().find(|e| e.0 == idx) {
                                    e.3.push_str(p);
                                }
                            }
                        }
                        _ => {}
                    },
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
            calls = pending
                .into_iter()
                .filter(|e| !e.2.is_empty())
                .map(|e| ToolCall {
                    id: e.1,
                    name: e.2,
                    args: if e.3.trim().is_empty() { "{}".into() } else { e.3 },
                })
                .collect();
        } else {
            let text = resp.text().await?;
            let v: Value = serde_json::from_str(&text).map_err(|e| anyhow::anyhow!("bad response: {e}"))?;
            for block in v["content"].as_array().into_iter().flatten() {
                match block["type"].as_str() {
                    Some("text") => {
                        if let Some(t) = block["text"].as_str() {
                            full.push_str(t);
                        }
                    }
                    Some("tool_use") => {
                        calls.push(ToolCall {
                            id: block["id"].as_str().unwrap_or("").to_string(),
                            name: block["name"].as_str().unwrap_or("").to_string(),
                            args: if block["input"].is_object() {
                                block["input"].to_string()
                            } else {
                                "{}".into()
                            },
                        });
                    }
                    _ => {}
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
        Ok(Reply { text: full, calls })
    }
}
