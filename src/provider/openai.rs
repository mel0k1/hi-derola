use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use super::{sse_lines, send, ApiEvent, ChatRequest, Provider, Reply};
use crate::chat::{Message, Role, ToolCall};

pub struct OpenAi {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

fn is_reasoning(model: &str) -> bool {
    let m = model.rsplit('/').next().unwrap_or(model);
    m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4") || m.starts_with("gpt-5")
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

fn msg_json(m: &Message) -> Value {
    match m.role {
        Role::Tool => json!({"role": "tool", "tool_call_id": m.tool_call_id, "content": m.content}),
        Role::Assistant if !m.tool_calls.is_empty() => {
            let content = if m.content.is_empty() {
                Value::Null
            } else {
                json!(m.content)
            };
            let calls: Vec<Value> = m
                .tool_calls
                .iter()
                .map(|c| json!({"id": c.id, "type": "function", "function": {"name": c.name, "arguments": c.args}}))
                .collect();
            json!({"role": "assistant", "content": content, "tool_calls": calls})
        }
        Role::User if !m.images.is_empty() => {
            let mut parts = Vec::with_capacity(1 + m.images.len());
            if !m.content.is_empty() {
                parts.push(json!({"type": "text", "text": m.content}));
            }
            for img in &m.images {
                parts.push(json!({
                    "type": "image_url",
                    "image_url": {"url": format!("data:{};base64,{}", img.mime, img.data)}
                }));
            }
            json!({"role": "user", "content": parts})
        }
        _ => json!({"role": m.role.as_str(), "content": m.content}),
    }
}

fn clean_args(s: &str) -> String {
    if s.trim().is_empty() {
        "{}".into()
    } else {
        s.to_string()
    }
}

#[async_trait]
impl Provider for OpenAi {
    fn name(&self) -> &'static str {
        "openai"
    }

    async fn chat(&self, req: &ChatRequest, tx: &UnboundedSender<ApiEvent>) -> Result<Reply> {
        let mut msgs = vec![json!({"role": "system", "content": req.system})];
        for m in &req.messages {
            msgs.push(msg_json(m));
        }
        let reasoning_model = is_reasoning(&req.model);
        let mut body = json!({"model": req.model, "messages": msgs, "stream": req.stream});
        if let Some(t) = req.max_tokens {
            if reasoning_model {
                body["max_completion_tokens"] = json!(t);
            } else {
                body["max_tokens"] = json!(t);
            }
        }
        if !reasoning_model {
            if let Some(v) = req.temperature {
                body["temperature"] = json!(v);
            }
            if let Some(v) = req.top_p {
                body["top_p"] = json!(v);
            }
        }
        if !req.tools.is_empty() {
            let tools: Vec<Value> = req
                .tools
                .iter()
                .map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.parameters}}))
                .collect();
            body["tools"] = tools.into();
            body["tool_choice"] = json!("auto");
        }
        if self.base_url.contains("openai.com") {
            body["prompt_cache_key"] = json!("hi-derola");
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
            tx,
        )
        .await?;

        let mut full = String::new();
        let mut calls: Vec<ToolCall> = Vec::new();
        if req.stream {
            let mut pending: Vec<Value> = Vec::new();
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
                let d = &v["choices"][0]["delta"];
                if let Some(c) = d["content"].as_str() {
                    if !c.is_empty() {
                        full.push_str(c);
                        tx.send(ApiEvent::Chunk(c.to_string()))
                            .map_err(|_| anyhow::anyhow!("closed"))?;
                    }
                }
                if let Some(r) = d["reasoning_content"].as_str().or_else(|| d["reasoning"].as_str()) {
                    if !r.is_empty() {
                        tx.send(ApiEvent::Reasoning(r.to_string()))
                            .map_err(|_| anyhow::anyhow!("closed"))?;
                    }
                }
                if let Some(tcs) = d["tool_calls"].as_array() {
                    for tc in tcs {
                        let idx = tc["index"].as_u64().unwrap_or(0) as usize;
                        while pending.len() <= idx {
                            pending.push(json!({"id": "", "name": "", "args": ""}));
                        }
                        if let Some(id) = tc["id"].as_str() {
                            pending[idx]["id"] = json!(id);
                        }
                        if let Some(n) = tc["function"]["name"].as_str() {
                            pending[idx]["name"] = json!(n);
                        }
                        if let Some(a) = tc["function"]["arguments"].as_str() {
                            let acc = format!("{}{}", pending[idx]["args"].as_str().unwrap_or(""), a);
                            pending[idx]["args"] = json!(acc);
                        }
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
            calls = pending
                .into_iter()
                .filter(|p| !p["name"].as_str().unwrap_or("").is_empty())
                .map(|p| ToolCall {
                    id: p["id"].as_str().unwrap_or("").to_string(),
                    name: p["name"].as_str().unwrap_or("").to_string(),
                    args: clean_args(p["args"].as_str().unwrap_or("")),
                })
                .collect();
        } else {
            let text = resp.text().await?;
            let v: Value = serde_json::from_str(&text).map_err(|e| anyhow::anyhow!("bad response: {e}"))?;
            let msg = &v["choices"][0]["message"];
            if let Some(c) = msg["content"].as_str() {
                full = c.to_string();
                if !full.is_empty() {
                    let _ = tx.send(ApiEvent::Chunk(full.clone()));
                }
            }
            if let Some(tcs) = msg["tool_calls"].as_array() {
                for tc in tcs {
                    calls.push(ToolCall {
                        id: tc["id"].as_str().unwrap_or("").to_string(),
                        name: tc["function"]["name"].as_str().unwrap_or("").to_string(),
                        args: clean_args(tc["function"]["arguments"].as_str().unwrap_or("")),
                    });
                }
            }
            if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
                tx.send(ApiEvent::Usage {
                    input: u["prompt_tokens"].as_u64().unwrap_or(0),
                    output: u["completion_tokens"].as_u64().unwrap_or(0),
                })
                .map_err(|_| anyhow::anyhow!("closed"))?;
            }
        }
        Ok(Reply { text: full, calls })
    }
}
