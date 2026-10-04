//! codesearch: code & docs search through the free Exa MCP endpoint,
//! the same backend opencode uses for its websearch/codesearch tools.
//! A plain JSON-RPC tools/call over HTTP, the answer comes back as SSE.

use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::time::Duration;

const EXA_MCP_URL: &str = "https://mcp.exa.ai/mcp";
const USER_AGENT: &str = "hi-derola/0.1";
const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_TOKENS: u64 = 50_000;

/// get_code_context_exa returns a ready-to-use context string (code
/// examples + docs); no results decode to this fallback
const NO_RESULTS: &str = "No code snippets or documentation found. Please try a different query, be more specific about the library or programming concept, or check the spelling of framework names.";

/// search code examples and docs through Exa; tokens_num is the requested
/// context budget (1000..50000)
pub async fn codesearch(query: &str, tokens_num: u64, timeout: u64) -> Result<String> {
    codesearch_at(EXA_MCP_URL, query, tokens_num, timeout).await
}

/// same call against an explicit endpoint; tests point this at a fake server
async fn codesearch_at(url: &str, query: &str, tokens_num: u64, timeout: u64) -> Result<String> {
    let tokens = tokens_num.clamp(1_000, MAX_TOKENS);
    let timeout = timeout.clamp(1, 120);
    let args = json!({"query": query, "tokensNum": tokens});
    let body = mcp_call(url, "get_code_context_exa", &args, timeout).await?;
    let text = body.trim();
    if text.is_empty() {
        bail!("codesearch: empty response from exa");
    }
    Ok(text.to_string())
}

/// JSON-RPC tools/call against an MCP-over-HTTP endpoint; the response is
/// either SSE (data: lines) or a bare JSON-RPC reply — both decode the same
pub async fn mcp_call(url: &str, tool: &str, args: &Value, timeout: u64) -> Result<String> {
    let http = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(15))
        .build()?;
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": tool, "arguments": args}
    });
    let resp = tokio::time::timeout(
        Duration::from_secs(timeout),
        http.post(url)
            .header("Accept", "application/json, text/event-stream")
            .json(&payload)
            .send(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("codesearch: request timed out ({timeout}s)"))??;
    let status = resp.status();
    if !status.is_success() {
        bail!("codesearch: exa returned {status}");
    }
    let ctype = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = read_body(resp).await?;
    let is_sse = ctype.contains("text/event-stream") || body.lines().any(|l| l.starts_with("data:"));
    if is_sse {
        parse_sse(&body)
    } else {
        parse_jsonrpc(&body)
    }
}

async fn read_body(mut resp: reqwest::Response) -> Result<String> {
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_BYTES {
            bail!("codesearch: response exceeds {MAX_BYTES} bytes");
        }
    }
    Ok(String::from_utf8_lossy(&body).to_string())
}

/// take the first data: line whose JSON carries result.content[0].text;
/// a JSON-RPC error inside SSE is a real failure and propagates
fn parse_sse(body: &str) -> Result<String> {
    for line in body.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        return text_from_rpc(&v);
    }
    bail!("codesearch: no result in exa response");
}

/// a bare JSON-RPC reply (no SSE framing) decodes the same way
fn parse_jsonrpc(body: &str) -> Result<String> {
    let v: Value = serde_json::from_str(body.trim())?;
    text_from_rpc(&v)
}

/// extract result.content[0].text; map JSON-RPC errors and "no content" to
/// friendly errors, an explicitly empty payload to the no-results hint
fn text_from_rpc(v: &Value) -> Result<String> {
    if let Some(err) = v.get("error") {
        let msg = err["message"]
            .as_str()
            .unwrap_or("unknown exa error")
            .to_string();
        bail!("codesearch: {msg}");
    }
    let content = &v["result"]["content"];
    let Some(items) = content.as_array() else {
        bail!("codesearch: unexpected exa response shape");
    };
    for item in items {
        if item["type"].as_str() == Some("text") {
            if let Some(t) = item["text"].as_str() {
                return if t.trim().is_empty() {
                    Ok(NO_RESULTS.to_string())
                } else {
                    Ok(t.to_string())
                };
            }
        }
    }
    if v["result"].get("content").is_some() {
        return Ok(NO_RESULTS.into());
    }
    bail!("codesearch: unexpected exa response shape");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use std::io::{Read, Write};

    /// routes: path -> (status, content-type, body); captures "path :: body"
    /// per request; the routes closure receives the real bound port
    fn spawn_fake_http(
        routes: impl FnOnce(u16) -> BTreeMap<String, (u16, String, String)>,
    ) -> (u16, Arc<Mutex<Vec<String>>>) {
        let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        let routes = routes(port);
        let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let log2 = log.clone();
        std::thread::spawn(move || {
            for stream in l.incoming() {
                let Ok(mut s) = stream else { break };
                let mut req = String::new();
                let mut buf = [0u8; 4096];
                loop {
                    let n = s.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    req.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if req.contains("\r\n\r\n") {
                        break;
                    }
                }
                let mut parts = req.split_whitespace();
                let method = parts.next().unwrap_or("").to_string();
                let path = parts.next().unwrap_or("/").to_string();
                let body = req
                    .split("\r\n\r\n")
                    .nth(1)
                    .unwrap_or("")
                    .to_string();
                log2.lock().unwrap().push(format!("{method} {path} :: {body}"));
                let Some((status, ctype, text)) = routes.get(&path) else {
                    let _ = write_all(
                        &mut s,
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                    continue;
                };
                let head = format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    status,
                    if *status < 400 { "OK" } else { "Error" },
                    ctype,
                    text.len()
                );
                let _ = write_all(&mut s, &head);
                let _ = write_all(&mut s, text);
            }
        });
        (port, log)
    }

    fn write_all(s: &mut std::net::TcpStream, text: &str) -> std::io::Result<()> {
        s.write_all(text.as_bytes())
    }

    fn base(port: u16) -> String {
        format!("http://127.0.0.1:{port}/mcp")
    }

    #[tokio::test]
    async fn sse_result_passthrough() {
        let sse = "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"const x = 1; // useful docs\"}]}}\n\n";
        let (port, log) = spawn_fake_http(|_p| {
            BTreeMap::from([(
                "/mcp".into(),
                (200, "text/event-stream".into(), sse.into()),
            )])
        });
        let out = codesearch_at(&base(port), "react hooks", 5_000, 10)
            .await
            .unwrap();
        assert_eq!(out, "const x = 1; // useful docs");
        let req = log.lock().unwrap().join("\n");
        assert!(req.contains("POST /mcp"), "{req}");
        assert!(req.contains("\"method\":\"tools/call\""), "{req}");
        assert!(req.contains("\"name\":\"get_code_context_exa\""), "{req}");
        assert!(req.contains("\"query\":\"react hooks\""), "{req}");
        assert!(req.contains("\"tokensNum\":5000"), "{req}");
    }

    #[tokio::test]
    async fn tokens_clamped_and_bare_json_reply() {
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"docs body"}]}}"#;
        let (port, _log) = spawn_fake_http(|_p| {
            BTreeMap::from([(
                "/mcp".into(),
                (200, "application/json".into(), body.into()),
            )])
        });
        let out = codesearch_at(&base(port), "q", 99_999, 10).await.unwrap();
        assert_eq!(out, "docs body");
    }

    #[tokio::test]
    async fn empty_content_means_no_results() {
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"   "}]}}"#;
        let (port, _log) = spawn_fake_http(|_p| {
            BTreeMap::from([(
                "/mcp".into(),
                (200, "text/event-stream".into(), format!("data: {body}\n\n")),
            )])
        });
        let out = codesearch_at(&base(port), "q", 2_000, 10).await.unwrap();
        assert_eq!(out, NO_RESULTS);
    }

    #[tokio::test]
    async fn rpc_error_surfaces_message() {
        let (port, _log) = spawn_fake_http(|_p| {
            BTreeMap::from([(
                "/mcp".into(),
                (
                    200,
                    "text/event-stream".into(),
                    "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"error\":{\"code\":-32000,\"message\":\"rate limited\"}}\n\n".into(),
                ),
            )])
        });
        let err = codesearch_at(&base(port), "q", 5_000, 10)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("rate limited"), "{err}");
    }

    #[tokio::test]
    async fn http_error_and_garbage() {
        let (port, _log) = spawn_fake_http(|_p| {
            BTreeMap::from([
                ("/err".into(), (500, "text/plain".into(), "boom".into())),
                ("/junk".into(), (200, "application/json".into(), "{not json".into())),
                (
                    "/shape".into(),
                    (200, "application/json".into(), r#"{"result":{}}"#.into()),
                ),
            ])
        });
        let b = |p: &str| format!("http://127.0.0.1:{port}{p}");
        let err = codesearch_at(&b("/err"), "q", 5_000, 10).await.unwrap_err();
        assert!(err.to_string().contains("500"), "{err}");
        assert!(codesearch_at(&b("/junk"), "q", 5_000, 10).await.is_err());
        assert!(codesearch_at(&b("/shape"), "q", 5_000, 10).await.is_err());
    }

    #[test]
    fn sse_and_rpc_parsers() {
        let sse = "event: message\ndata: {\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"abc\"}]}}\n\ndata: {\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"second\"}]}}";
        assert_eq!(parse_sse(sse).unwrap(), "abc");
        assert!(parse_sse("data: [DONE]").is_err());
        assert!(parse_sse(": keepalive").is_err());
        let rpc = r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"t"}]}}"#;
        assert_eq!(parse_jsonrpc(rpc).unwrap(), "t");
        assert!(parse_jsonrpc("{\"result\":{\"content\":[]}}").is_ok());
        assert!(parse_jsonrpc("[]").is_err());
    }
}
