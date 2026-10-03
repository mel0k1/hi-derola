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

fn conv_msgs(messages: &[Message]) -> Vec<Value> {
    let mut out: Vec<(bool, Value)> = Vec::new();
    for m in messages {
        match m.role {
            Role::Tool => {
                let block = json!({"type": "tool_result", "tool_use_id": m.tool_call_id, "content": m.content});
                match out.last_mut() {
                    Some((true, last)) => {
                        if let Some(a) = last["content"].as_array_mut() {
                            a.push(block);
                        }
                    }
                    _ => out.push((true, json!({"role": "user", "content": [block]}))),
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
                out.push((false, json!({"role": "assistant", "content": content})));
            }
            Role::User if !m.images.is_empty() => {
                let mut content = Vec::new();
                if !m.content.is_empty() {
                    content.push(json!({"type": "text", "text": m.content}));
                }
                for img in &m.images {
                    content.push(json!({
                        "type": "image",
                        "source": {"type": "base64", "media_type": img.mime, "data": img.data}
                    }));
                }
                out.push((false, json!({"role": "user", "content": content})));
            }
            _ => out.push((
                false,
                json!({"role": m.role.as_str(), "content": [{"type": "text", "text": m.content}]}),
            )),
        }
    }
    if let Some((_, last)) = out.last_mut() {
        // mark the tail of the conversation for prompt caching
        if let Some(a) = last["content"].as_array_mut() {
            if let Some(b) = a.last_mut() {
                b["cache_control"] = json!({"type": "ephemeral"});
            }
        }
    }
    out.into_iter().map(|(_, v)| v).collect()
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
            "system": [{"type": "text", "text": req.system, "cache_control": {"type": "ephemeral"}}],
            "messages": conv_msgs(&req.messages),
            "stream": req.stream,
        });
        if let Some(v) = req.temperature {
            body["temperature"] = json!(v);
        } else if let Some(v) = req.top_p {
            body["top_p"] = json!(v);
        }
        if !req.tools.is_empty() {
            let mut tools: Vec<Value> = req
                .tools
                .iter()
                .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.parameters}))
                .collect();
            if let Some(last) = tools.last_mut() {
                last["cache_control"] = json!({"type": "ephemeral"});
            }
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
        let mut stop = String::new();
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
                        let u = &v["message"]["usage"];
                        let cache_read = u["cache_read_input_tokens"].as_u64().unwrap_or(0);
                        let cache_write = u["cache_creation_input_tokens"].as_u64().unwrap_or(0);
                        tx.send(ApiEvent::Usage {
                            input: u["input_tokens"].as_u64().unwrap_or(0) + cache_read + cache_write,
                            output: 0,
                            cached: cache_read,
                        })
                        .map_err(|_| anyhow::anyhow!("closed"))?;
                    }
                    Some("message_delta") => {
                        if let Some(sr) = v["delta"]["stop_reason"].as_str() {
                            if !sr.is_empty() {
                                stop = sr.to_string();
                            }
                        }
                        tx.send(ApiEvent::Usage {
                            input: 0,
                            output: v["usage"]["output_tokens"].as_u64().unwrap_or(0),
                            cached: 0,
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
            stop = v["stop_reason"].as_str().unwrap_or("").to_string();
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
            let cache_read = v["usage"]["cache_read_input_tokens"].as_u64().unwrap_or(0);
            let cache_write = v["usage"]["cache_creation_input_tokens"].as_u64().unwrap_or(0);
            tx.send(ApiEvent::Usage {
                input: v["usage"]["input_tokens"].as_u64().unwrap_or(0) + cache_read + cache_write,
                output: v["usage"]["output_tokens"].as_u64().unwrap_or(0),
                cached: cache_read,
            })
            .map_err(|_| anyhow::anyhow!("closed"))?;
        }
        Ok(Reply {
            text: full,
            calls,
            truncated: stop == "max_tokens",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_control_placement() {
        let msgs = vec![
            Message::new(Role::User, "hi"),
            Message::new(Role::Assistant, "").with_calls(vec![ToolCall {
                id: "t1".into(),
                name: "bash".into(),
                args: "{}".into(),
            }]),
            Message::tool("t1", "out"),
        ];
        let conv = conv_msgs(&msgs);
        assert_eq!(conv.len(), 3);
        let last = conv.last().unwrap();
        assert_eq!(last["role"], "user");
        let blocks = last["content"].as_array().unwrap();
        assert_eq!(blocks[0]["type"], "tool_result");
        assert_eq!(blocks[0]["cache_control"]["type"], "ephemeral");

        let plain = conv_msgs(&[Message::new(Role::User, "hello")]);
        assert_eq!(
            plain[0]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
        assert_eq!(plain[0]["content"][0]["text"], "hello");
    }
}
