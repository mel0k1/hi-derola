use anyhow::{bail, Context, Result};
use base64::Engine;
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::config::{McpConfig, McpOAuthCfg};

const CALLBACK_PORT: u16 = 19876;
const CALLBACK_PATH: &str = "/mcp/oauth/callback";
const AUTH_TIMEOUT: Duration = Duration::from_secs(300);

static STORE_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClientInfo {
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret_expires_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Entry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<Tokens>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_info: Option<ClientInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_verifier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_endpoint: Option<String>,
    /// redirect_uri pinned when the flow started so the exchange matches
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_uri: Option<String>,
    /// RFC 8707 resource indicator pinned at flow start; rides the authorize
    /// url, the token exchange and every refresh
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
}

struct AuthEndpoints {
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: Option<String>,
    scope: Option<String>,
    resource: Option<String>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---- store: <data>/hi-derola/mcp-auth.json ----

pub fn set_store_file(p: Option<PathBuf>) {
    *STORE_OVERRIDE.lock().unwrap() = p;
}

fn store_path() -> Result<PathBuf> {
    if let Some(p) = STORE_OVERRIDE.lock().unwrap().clone() {
        return Ok(p);
    }
    Ok(dirs::data_dir()
        .context("no data dir")?
        .join("hi-derola")
        .join("mcp-auth.json"))
}

fn load_all() -> BTreeMap<String, Entry> {
    let Ok(path) = store_path() else {
        return BTreeMap::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn save_all(map: &BTreeMap<String, Entry>) -> Result<()> {
    let path = store_path()?;
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let mut f = std::fs::OpenOptions::new();
    f.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        f.mode(0o600);
    }
    let mut file = f.open(&path)?;
    file.write_all(serde_json::to_string_pretty(map)?.as_bytes())?;
    Ok(())
}

fn mutate(name: &str, server_url: Option<&str>, f: impl FnOnce(&mut Entry)) -> Result<()> {
    let mut all = load_all();
    let e = all.entry(name.to_string()).or_default();
    if let Some(u) = server_url {
        e.server_url = Some(u.to_string());
    }
    f(e);
    save_all(&all)
}

pub fn get(name: &str) -> Option<Entry> {
    load_all().get(name).cloned()
}

pub fn get_for_url(name: &str, server_url: &str) -> Option<Entry> {
    let e = get(name)?;
    if e.server_url.as_deref()? != server_url {
        return None;
    }
    Some(e)
}

pub fn remove(name: &str) {
    let mut all = load_all();
    all.remove(name);
    let _ = save_all(&all);
}

/// drop the stored tokens for one server (client_info and a pending pkce
/// flow stay); returns true when there were tokens to clear — the next
/// request hits 401 and the normal authorize flow starts over
pub fn logout(name: &str) -> bool {
    let had = get(name).map(|e| e.tokens.is_some()).unwrap_or(false);
    if had {
        mutate(name, None, |e| e.tokens = None).ok();
    }
    had
}

pub fn is_expired(name: &str) -> Option<bool> {
    let t = get(name)?.tokens?;
    Some(t.expires_at.map(|e| e < now_secs()).unwrap_or(false))
}

// ---- pkce / state ----

fn rand_bytes(n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    getrandom::fill(&mut b).expect("os rng");
    b
}

pub fn pkce_pair() -> (String, String) {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
    let verifier = B64URL.encode(rand_bytes(64));
    let mut h = Sha256::new();
    h.update(verifier.as_bytes());
    let challenge = B64URL.encode(h.finalize());
    (verifier, challenge)
}

pub fn gen_state() -> String {
    rand_bytes(16).iter().map(|b| format!("{b:02x}")).collect()
}

// ---- urls ----

pub fn parse_redirect(uri: Option<&str>) -> (u16, String) {
    let Some(u) = uri.and_then(|s| Url::parse(s).ok()) else {
        return (CALLBACK_PORT, CALLBACK_PATH.into());
    };
    let port = u
        .port()
        .unwrap_or(if u.scheme() == "https" { 443 } else { 80 });
    let path = if u.path().is_empty() {
        CALLBACK_PATH.into()
    } else {
        u.path().to_string()
    };
    (port, path)
}

fn origin_of(u: &str) -> Result<String> {
    let url = Url::parse(u)?;
    let host = url.host_str().context("no host")?;
    let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
    Ok(format!("{}://{host}{port}", url.scheme()))
}

fn path_of(u: &str) -> String {
    Url::parse(u)
        .map(|x| x.path().trim_end_matches('/').to_string())
        .unwrap_or_default()
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 3 <= b.len() => {
                match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn extract_resource_metadata(www_auth: &str) -> Option<String> {
    let idx = www_auth.find("resource_metadata=")?;
    let rest = &www_auth[idx + "resource_metadata=".len()..];
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

// ---- discovery ----

async fn get_json(http: &Client, url: &str) -> Result<Value> {
    let resp = http
        .get(url)
        .header("Accept", "application/json")
        .timeout(Duration::from_secs(10))
        .send()
        .await?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("{status} {}", &text.chars().take(120).collect::<String>());
    }
    Ok(serde_json::from_str(&text)?)
}

async fn fetch_auth_meta(http: &Client, issuer: &str) -> Result<Value> {
    let origin = origin_of(issuer)?;
    let path = path_of(issuer);
    let issuer = issuer.trim_end_matches('/');
    let mut cands = Vec::new();
    if !path.is_empty() {
        cands.push(format!(
            "{origin}/.well-known/oauth-authorization-server{path}"
        ));
    }
    cands.push(format!("{issuer}/.well-known/oauth-authorization-server"));
    cands.push(format!("{issuer}/.well-known/openid-configuration"));
    cands.dedup();
    for c in &cands {
        if let Ok(v) = get_json(http, c).await {
            return Ok(v);
        }
    }
    bail!("no OAuth metadata at issuer {issuer}")
}

async fn discover(
    http: &Client,
    server_url: &str,
    oauth: Option<&McpOAuthCfg>,
) -> Result<AuthEndpoints> {
    // a pinned authorization server metadata url skips every RFC 9728 probe
    // (no 401 initialize, no protected-resource fetch); the resource is
    // pinned to the mcp server url itself
    if let Some(pinned) = oauth.and_then(|o| o.auth_server_metadata_url.as_deref()) {
        let meta = get_json(http, pinned).await?;
        let (authorization_endpoint, token_endpoint, registration_endpoint, scope) =
            endpoints_from_meta(&meta)?;
        return Ok(AuthEndpoints {
            authorization_endpoint,
            token_endpoint,
            registration_endpoint,
            scope,
            resource: Some(server_url.to_string()),
        });
    }

    let mut resource_meta: Option<Value> = None;

    // probe: unauthenticated initialize -> 401 + WWW-Authenticate
    let probe = json!({
        "jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26", "capabilities": {},
            "clientInfo": {"name": "hi-derola", "version": "0.1.0"}
        }
    });
    if let Ok(resp) = http
        .post(server_url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&probe)
        .timeout(Duration::from_secs(15))
        .send()
        .await
    {
        if resp.status().as_u16() == 401 {
            if let Some(wa) = resp
                .headers()
                .get("WWW-Authenticate")
                .and_then(|v| v.to_str().ok())
            {
                if let Some(meta_url) = extract_resource_metadata(wa) {
                    resource_meta = get_json(http, &meta_url).await.ok();
                }
            }
        }
    }

    if resource_meta.is_none() {
        let origin = origin_of(server_url)?;
        let path = path_of(server_url);
        let cands = [
            format!("{origin}/.well-known/oauth-protected-resource{path}"),
            format!("{origin}/.well-known/oauth-protected-resource"),
        ];
        for c in &cands {
            if let Ok(v) = get_json(http, c).await {
                resource_meta = Some(v);
                break;
            }
        }
    }

    let (issuer, resource) = match &resource_meta {
        Some(m) => {
            let iss = m["authorization_servers"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(|v| v.as_str())
                .context("no authorization_servers in resource metadata")?
                .to_string();
            (iss, m["resource"].as_str().map(String::from))
        }
        None => (origin_of(server_url)?, Some(server_url.to_string())),
    };

    let meta = fetch_auth_meta(http, &issuer).await?;
    let (authorization_endpoint, token_endpoint, registration_endpoint, scope) =
        endpoints_from_meta(&meta)?;
    Ok(AuthEndpoints {
        authorization_endpoint,
        token_endpoint,
        registration_endpoint,
        scope,
        resource,
    })
}

/// the four fields we care about from an authorization server metadata doc
fn endpoints_from_meta(meta: &Value) -> Result<(String, String, Option<String>, Option<String>)> {
    let scopes = meta["scopes_supported"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|s| !s.is_empty());
    Ok((
        meta["authorization_endpoint"]
            .as_str()
            .context("no authorization_endpoint in metadata")?
            .to_string(),
        meta["token_endpoint"]
            .as_str()
            .context("no token_endpoint in metadata")?
            .to_string(),
        meta["registration_endpoint"].as_str().map(String::from),
        scopes,
    ))
}

// ---- registration / tokens ----

async fn register_client(
    http: &Client,
    ep: &str,
    redirect: &str,
    secret: bool,
    scope: Option<&str>,
) -> Result<ClientInfo> {
    // RFC 7591 client metadata document
    let mut body = json!({
        "client_name": "Hi!Derola",
        "client_uri": "https://github.com/mel0k1/hi-derola",
        "redirect_uris": [redirect],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": if secret { "client_secret_post" } else { "none" },
    });
    if let Some(s) = scope {
        if !s.is_empty() {
            body["scope"] = json!(s);
        }
    }
    let resp = http
        .post(ep)
        .json(&body)
        .timeout(Duration::from_secs(15))
        .send()
        .await?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!(
            "dynamic registration failed: {status} — add oauth.client_id (and client_secret) to the [[mcp]] entry in config.toml (register the client manually)"
        );
    }
    let v: Value = serde_json::from_str(&text)?;
    Ok(ClientInfo {
        client_id: v["client_id"]
            .as_str()
            .context("registration: no client_id")?
            .to_string(),
        client_secret: v["client_secret"].as_str().map(String::from),
        client_secret_expires_at: v["client_secret_expires_at"].as_u64(),
    })
}

pub fn tokens_from_json(v: &Value) -> Result<Tokens> {
    let access = v["access_token"]
        .as_str()
        .context("no access_token in token response")?;
    Ok(Tokens {
        access_token: access.to_string(),
        refresh_token: v["refresh_token"].as_str().map(String::from),
        expires_at: v["expires_in"].as_u64().map(|e| now_secs() + e),
        scope: v["scope"].as_str().map(String::from),
    })
}

async fn token_request(http: &Client, ep: &str, form: &[(&str, &str)]) -> Result<Tokens> {
    let resp = http
        .post(ep)
        .form(form)
        .timeout(Duration::from_secs(15))
        .send()
        .await?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!(
            "token endpoint: {status} {}",
            &text.chars().take(150).collect::<String>()
        );
    }
    tokens_from_json(&serde_json::from_str::<Value>(&text)?)
}

pub fn build_auth_url(
    ep: &str,
    client_id: &str,
    redirect: &str,
    scope: Option<&str>,
    state: &str,
    challenge: &str,
    resource: Option<&str>,
) -> Result<String> {
    let mut u = Url::parse(ep)?;
    {
        let mut q = u.query_pairs_mut();
        q.append_pair("response_type", "code");
        q.append_pair("client_id", client_id);
        q.append_pair("redirect_uri", redirect);
        if let Some(s) = scope {
            if !s.is_empty() {
                q.append_pair("scope", s);
            }
        }
        q.append_pair("state", state);
        q.append_pair("code_challenge", challenge);
        q.append_pair("code_challenge_method", "S256");
        if let Some(r) = resource {
            q.append_pair("resource", r);
        }
    }
    Ok(u.to_string())
}

// ---- callback server + browser ----

fn respond(stream: &mut std::net::TcpStream, status: &str, html: &str) {
    let body = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.len()
    );
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();
}

const HTML_SUCCESS: &str = "<!DOCTYPE html><html><head><title>Hi!Derola</title></head><body style='font-family:system-ui;display:flex;justify-content:center;align-items:center;height:100vh;margin:0;background:#1a1a2e;color:#eee'><h1 style='color:#4ade80'>Authorization Successful</h1></body></html>";
fn html_error(e: &str) -> String {
    format!("<!DOCTYPE html><html><head><title>Hi!Derola</title></head><body style='font-family:system-ui;display:flex;justify-content:center;align-items:center;height:100vh;margin:0;background:#1a1a2e;color:#eee'><h1 style='color:#f87171'>Authorization Failed</h1><code>{e}</code></body></html>")
}

pub fn wait_for_callback(port: u16, path: &str, state: &str, timeout: Duration) -> Result<String> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .with_context(|| format!("OAuth callback port {port} unavailable"))?;
    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + timeout;
    loop {
        if Instant::now() >= deadline {
            bail!("OAuth callback timeout — authorization took too long");
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut buf = Vec::new();
                let mut tmp = [0u8; 1024];
                loop {
                    match stream.read(&mut tmp) {
                        Ok(0) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&tmp[..n]);
                            if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 16 * 1024 {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                let req = String::from_utf8_lossy(&buf);
                let target = req
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("/");
                let (req_path, query) = match target.split_once('?') {
                    Some((p, q)) => (p, q),
                    None => (target, ""),
                };
                if req_path.trim_end_matches('/') != path.trim_end_matches('/') {
                    respond(&mut stream, "404 Not Found", "not found");
                    continue;
                }
                let params: Vec<(String, String)> = query
                    .split('&')
                    .filter_map(|kv| {
                        let (k, v) = kv.split_once('=')?;
                        Some((url_decode(k), url_decode(v)))
                    })
                    .collect();
                let p = |k: &str| params.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
                let Some(st) = p("state") else {
                    respond(
                        &mut stream,
                        "400 Bad Request",
                        &html_error("missing state parameter — potential CSRF attack"),
                    );
                    continue;
                };
                if st != state {
                    respond(
                        &mut stream,
                        "400 Bad Request",
                        &html_error("invalid or expired state — potential CSRF attack"),
                    );
                    continue;
                }
                if let Some(err) = p("error") {
                    let desc = p("error_description").unwrap_or(err);
                    respond(&mut stream, "200 OK", &html_error(&desc));
                    bail!("authorization failed: {desc}");
                }
                let Some(code) = p("code") else {
                    respond(
                        &mut stream,
                        "400 Bad Request",
                        &html_error("no authorization code provided"),
                    );
                    continue;
                };
                respond(&mut stream, "200 OK", HTML_SUCCESS);
                return Ok(code);
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

pub fn open_browser(url: &str) -> Result<()> {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/c", "start", "", url]);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("failed to open browser")?;
    Ok(())
}

// ---- bearer for the transport ----

fn client_id_for(name: &str, oauth: Option<&McpOAuthCfg>) -> Option<String> {
    if let Some(o) = oauth {
        if let Some(id) = &o.client_id {
            return Some(id.clone());
        }
    }
    get(name).and_then(|e| e.client_info).map(|c| c.client_id)
}

fn client_secret_for(name: &str, oauth: Option<&McpOAuthCfg>) -> Option<String> {
    if let Some(o) = oauth {
        if let Some(s) = &o.client_secret {
            return Some(s.clone());
        }
    }
    get(name)
        .and_then(|e| e.client_info)
        .and_then(|c| c.client_secret)
}

async fn do_refresh(
    http: &Client,
    name: &str,
    server_url: &str,
    oauth: Option<&McpOAuthCfg>,
    refresh_token: &str,
) -> Result<String> {
    let Some(client_id) = client_id_for(name, oauth) else {
        bail!("mcp {name}: no client_id for refresh — run /mcpauth {name}");
    };
    let ep = match get(name).and_then(|e| e.token_endpoint) {
        Some(e) => e,
        None => discover(http, server_url, oauth).await?.token_endpoint,
    };
    // the resource indicator pinned at flow start must ride every refresh
    let resource = get(name).and_then(|e| e.resource);
    let mut form: Vec<(&str, &str)> = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id.as_str()),
    ];
    let secret = client_secret_for(name, oauth);
    if let Some(s) = &secret {
        form.push(("client_secret", s.as_str()));
    }
    if let Some(r) = &resource {
        form.push(("resource", r.as_str()));
    }
    match token_request(http, &ep, &form).await {
        Ok(t) => {
            let access = t.access_token.clone();
            mutate(name, Some(server_url), |e| {
                let keep_refresh = t
                    .refresh_token
                    .clone()
                    .or_else(|| e.tokens.as_ref().and_then(|x| x.refresh_token.clone()));
                e.tokens = Some(Tokens {
                    refresh_token: keep_refresh,
                    ..t.clone()
                });
            })
            .ok();
            Ok(access)
        }
        Err(e) => {
            mutate(name, Some(server_url), |e| e.tokens = None).ok();
            bail!("mcp {name}: refresh failed ({e:#}) — run /mcpauth {name}")
        }
    }
}

pub async fn bearer(
    name: &str,
    server_url: &str,
    oauth: Option<&McpOAuthCfg>,
    http: &Client,
    force_refresh: bool,
) -> Result<Option<String>> {
    let Some(entry) = get_for_url(name, server_url) else {
        return Ok(None);
    };
    let Some(tokens) = entry.tokens else {
        return Ok(None);
    };
    if !force_refresh {
        if let Some(exp) = tokens.expires_at {
            if now_secs() + 30 >= exp {
                if let Some(rt) = &tokens.refresh_token {
                    return Ok(Some(do_refresh(http, name, server_url, oauth, rt).await?));
                }
            }
        }
        return Ok(Some(tokens.access_token));
    }
    match &tokens.refresh_token {
        Some(rt) => Ok(Some(do_refresh(http, name, server_url, oauth, rt).await?)),
        None => bail!("mcp {name}: 401 unauthorized and no refresh token — run /mcpauth {name}"),
    }
}

// ---- interactive flow ----

/// everything the caller needs between the two oauth phases
pub struct AuthStart {
    pub url: String,
    pub state: String,
    /// (port, path) of the local callback server
    pub callback: (u16, String),
}

/// phase 1: discover endpoints, register the client, persist the pkce pair
/// (code_verifier + oauth_state + token_endpoint + redirect_uri) and return
/// the authorization url. the flow is resumable from here at any time —
/// even after a restart, the verifier lives in the auth store
pub async fn start_auth(name: &str, cfgs: &[McpConfig]) -> Result<AuthStart> {
    let cfg = cfgs
        .iter()
        .find(|c| c.name == name)
        .with_context(|| format!("mcp {name}: not in config"))?;
    let oauth = cfg
        .oauth_cfg()
        .context("mcp: OAuth is disabled in config")?;
    let server_url = cfg
        .url
        .clone()
        .context("mcp: OAuth applies to remote servers only")?;

    let http = Client::builder().user_agent("hi-derola").build()?;
    let ep = discover(&http, &server_url, Some(&oauth)).await?;
    let redirect = oauth
        .redirect_uri
        .clone()
        .unwrap_or_else(|| format!("http://127.0.0.1:{CALLBACK_PORT}{CALLBACK_PATH}"));
    let scope = oauth.scope.clone().or_else(|| ep.scope.clone());

    let client_info = match &oauth.client_id {
        Some(id) => Some(ClientInfo {
            client_id: id.clone(),
            client_secret: oauth.client_secret.clone(),
            client_secret_expires_at: None,
        }),
        None => {
            let stored = get_for_url(name, &server_url).and_then(|e| e.client_info);
            match stored {
                Some(ci) if ci.client_secret_expires_at.map(|e| e > now_secs()).unwrap_or(true) => {
                    Some(ci)
                }
                _ => match &ep.registration_endpoint {
                    Some(r) => {
                        let ci =
                            register_client(&http, r, &redirect, oauth.client_secret.is_some(), scope.as_deref())
                                .await?;
                        mutate(name, Some(&server_url), |e| e.client_info = Some(ci.clone())).ok();
                        Some(ci)
                    }
                    None => bail!(
                        "mcp {name}: server does not support dynamic registration — register a client manually and add oauth.client_id (and client_secret) to the [[mcp]] entry in config.toml"
                    ),
                },
            }
        }
    };
    let ci = client_info.context("no client info")?;

    let (verifier, challenge) = pkce_pair();
    let state = gen_state();
    mutate(name, Some(&server_url), |e| {
        e.code_verifier = Some(verifier.clone());
        e.oauth_state = Some(state.clone());
        e.token_endpoint = Some(ep.token_endpoint.clone());
        e.redirect_uri = Some(redirect.clone());
        e.resource = ep.resource.clone();
    })
    .ok();

    let url = build_auth_url(
        &ep.authorization_endpoint,
        &ci.client_id,
        &redirect,
        scope.as_deref(),
        &state,
        &challenge,
        ep.resource.as_deref(),
    )?;
    Ok(AuthStart {
        url,
        state,
        callback: parse_redirect(Some(&redirect)),
    })
}

/// phase 2: exchange an authorization code (from the local callback server
/// or pasted manually as /mcpauth <name> <code>) using the persisted pkce
/// pair; clears the pending flow on success
pub async fn finish_auth(name: &str, cfgs: &[McpConfig], code: &str) -> Result<String> {
    let code = code.trim();
    if code.is_empty() {
        bail!("empty authorization code");
    }
    let cfg = cfgs
        .iter()
        .find(|c| c.name == name)
        .with_context(|| format!("mcp {name}: not in config"))?;
    let oauth = cfg
        .oauth_cfg()
        .context("mcp: OAuth is disabled in config")?;
    let server_url = cfg
        .url
        .clone()
        .context("mcp: OAuth applies to remote servers only")?;
    let entry = get(name)
        .with_context(|| format!("mcp {name}: nothing stored — run /mcpauth {name} first"))?;
    let verifier = entry.code_verifier.with_context(|| {
        format!("mcp {name}: no pending oauth flow — run /mcpauth {name} first")
    })?;
    let ep = entry.token_endpoint.with_context(|| {
        format!("mcp {name}: no token endpoint stored — run /mcpauth {name} first")
    })?;
    let redirect = entry
        .redirect_uri
        .or_else(|| oauth.redirect_uri.clone())
        .unwrap_or_else(|| format!("http://127.0.0.1:{CALLBACK_PORT}{CALLBACK_PATH}"));
    let client_id = oauth
        .client_id
        .clone()
        .or_else(|| entry.client_info.as_ref().map(|c| c.client_id.clone()))
        .context("no client_id for the exchange")?;
    let client_secret = oauth.client_secret.clone().or_else(|| {
        entry
            .client_info
            .as_ref()
            .and_then(|c| c.client_secret.clone())
    });
    // RFC 8707: the same resource indicator pinned at flow start rides the
    // token exchange
    let resource = entry.resource.clone();

    let http = Client::builder().user_agent("hi-derola").build()?;
    let mut form: Vec<(&str, &str)> = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect.as_str()),
        ("client_id", client_id.as_str()),
        ("code_verifier", verifier.as_str()),
    ];
    if let Some(sec) = &client_secret {
        form.push(("client_secret", sec.as_str()));
    }
    if let Some(r) = &resource {
        form.push(("resource", r.as_str()));
    }
    let tokens = token_request(&http, &ep, &form).await?;
    mutate(name, Some(&server_url), |e| {
        e.tokens = Some(tokens.clone());
        e.code_verifier = None;
        e.oauth_state = None;
    })
    .ok();

    let scope_note = tokens
        .scope
        .as_deref()
        .map(|s| format!(" (scope: {s})"))
        .unwrap_or_default();
    Ok(format!("mcp {name}: authorized{scope_note}"))
}

/// one-shot browser flow: start_auth, open the browser, wait for the local
/// callback, then finish_auth. every failure carries the url and the manual
/// resume hint — the verifier is already persisted
pub async fn authorize_flow(name: &str, cfgs: &[McpConfig]) -> Result<String> {
    let start = start_auth(name, cfgs).await?;
    if open_browser(&start.url).is_err() {
        bail!(
            "browser did not open — open this url, approve, then finish with: /mcpauth {name} <code from the redirect url>\n{}",
            start.url
        );
    }
    let (port, path) = start.callback;
    let state = start.state.clone();
    let waited =
        tokio::task::spawn_blocking(move || wait_for_callback(port, &path, &state, AUTH_TIMEOUT))
            .await
            .context("callback task failed")?;
    let code = waited.map_err(|e| {
        anyhow::anyhow!(
            "{e:#} — approve in the browser and finish with: /mcpauth {name} <code>\n{}",
            start.url
        )
    })?;
    finish_auth(name, cfgs, &code).await
}

pub fn status_line(cfg: &McpConfig) -> String {
    if cfg.url.is_none() {
        return format!("{} — local (no OAuth)", cfg.name);
    }
    if cfg.oauth_cfg().is_none() {
        return format!("{} — OAuth disabled", cfg.name);
    }
    match is_expired(&cfg.name) {
        None => format!("{} — not authenticated (/mcpauth {})", cfg.name, cfg.name),
        Some(true) => format!("{} — token expired (/mcpauth {})", cfg.name, cfg.name),
        Some(false) => format!("{} — authenticated", cfg.name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// set_store_file is a process-global override: store tests take this
    /// lock so parallel test threads do not race on it
    static STORE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn free_port() -> u16 {
        let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        port
    }

    // routes: path -> (status, extra headers, body); captures "METHOD path :: body" per request
    // routes(port) is built after bind, so test urls can reference the real port
    fn spawn_fake_http(
        routes: impl FnOnce(u16) -> BTreeMap<String, (u16, String, String)>,
    ) -> (u16, std::sync::Arc<Mutex<Vec<String>>>) {
        let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        let routes = routes(port);
        let log: std::sync::Arc<Mutex<Vec<String>>> = Default::default();
        let log2 = log.clone();
        std::thread::spawn(move || {
            for stream in l.incoming() {
                let Ok(mut s) = stream else { continue };
                let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
                let mut buf = vec![0u8; 16 * 1024];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let head = req.lines().next().unwrap_or("").to_string();
                let mut parts = head.split_whitespace();
                let method = parts.next().unwrap_or("").to_string();
                let target = parts.next().unwrap_or("/").to_string();
                let path = target.split('?').next().unwrap_or("/").to_string();
                let body = req.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
                log2.lock()
                    .unwrap()
                    .push(format!("{method} {path} :: {body}"));
                let (status, extra, resp_body) =
                    routes
                        .get(&path)
                        .cloned()
                        .unwrap_or((404, String::new(), "not found".into()));
                let reason = match status {
                    200 => "OK",
                    401 => "Unauthorized",
                    _ => "Error",
                };
                let resp = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                    resp_body.len(),
                    resp_body
                );
                let _ = s.write_all(resp.as_bytes());
                let _ = s.flush();
            }
        });
        (port, log)
    }

    #[test]
    fn pkce_challenge_matches_s256() {
        use base64::Engine as _;
        let (v, c) = pkce_pair();
        assert!((43..=128).contains(&v.len()), "verifier len {}", v.len());
        let mut h = Sha256::new();
        h.update(v.as_bytes());
        let expect = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(h.finalize());
        assert_eq!(c, expect);
        let (v2, c2) = pkce_pair();
        assert_ne!(v, v2);
        assert_ne!(c, c2);
    }

    #[test]
    fn gen_state_is_hex_and_unique() {
        let (a, b) = (gen_state(), gen_state());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn auth_url_params() {
        let u = build_auth_url(
            "https://as.example.com/authorize",
            "cid",
            "http://127.0.0.1:19876/mcp/oauth/callback",
            Some("read write"),
            "st1",
            "ch1",
            Some("https://rs.example.com/mcp"),
        )
        .unwrap();
        assert!(u.starts_with("https://as.example.com/authorize?"));
        for part in [
            "response_type=code",
            "client_id=cid",
            "redirect_uri=http%3A%2F%2F127.0.0.1%3A19876%2Fmcp%2Foauth%2Fcallback",
            "scope=read+write",
            "state=st1",
            "code_challenge=ch1",
            "code_challenge_method=S256",
            "resource=https%3A%2F%2Frs.example.com%2Fmcp",
        ] {
            assert!(u.contains(part), "missing {part} in {u}");
        }
        // empty scope omitted
        let u2 = build_auth_url("https://a/e", "c", "http://x/cb", None, "s", "ch", None).unwrap();
        assert!(!u2.contains("scope"));
    }

    #[test]
    fn redirect_parse_defaults() {
        let (p, path) = parse_redirect(None);
        assert_eq!(p, 19876);
        assert_eq!(path, "/mcp/oauth/callback");
        let (p2, path2) = parse_redirect(Some("not a url"));
        assert_eq!(p2, 19876);
        assert_eq!(path2, "/mcp/oauth/callback");
        let (p3, path3) = parse_redirect(Some("http://127.0.0.1:9999/custom/cb"));
        assert_eq!(p3, 9999);
        assert_eq!(path3, "/custom/cb");
        let (p4, _) = parse_redirect(Some("https://srv.example.com/cb"));
        assert_eq!(p4, 443);
    }

    #[test]
    fn url_decode_and_metadata_extraction() {
        assert_eq!(url_decode("abc%2Bd"), "abc+d");
        assert_eq!(url_decode("a+b%20c"), "a b c");
        assert_eq!(url_decode("bad%zz"), "bad%zz");
        let wa = r#"Bearer realm="x", resource_metadata="https://rs.example.com/.well-known/oauth-protected-resource/mcp""#;
        assert_eq!(
            extract_resource_metadata(wa).as_deref(),
            Some("https://rs.example.com/.well-known/oauth-protected-resource/mcp")
        );
        assert_eq!(extract_resource_metadata("Basic realm=\"x\""), None);
    }

    #[test]
    fn callback_flow_ok_and_csrf() {
        let (tx, rx) = std::sync::mpsc::channel();
        let port = free_port();
        let h = std::thread::spawn(move || {
            tx.send(()).unwrap();
            wait_for_callback(
                port,
                "/mcp/oauth/callback",
                "st123",
                Duration::from_secs(10),
            )
        });
        rx.recv().unwrap();
        std::thread::sleep(Duration::from_millis(150));
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(
            b"GET /mcp/oauth/callback?code=abc%2Bd&state=st123 HTTP/1.1\r\nHost: x\r\n\r\n",
        )
        .unwrap();
        let mut resp = String::new();
        let _ = s.read_to_string(&mut resp);
        assert!(resp.contains("200 OK"), "got: {resp}");
        assert_eq!(h.join().unwrap().unwrap(), "abc+d");

        // wrong state -> rejected
        let port2 = free_port();
        let h2 = std::thread::spawn(move || {
            wait_for_callback(port2, "/cb", "good", Duration::from_secs(10))
        });
        std::thread::sleep(Duration::from_millis(150));
        let mut s2 = std::net::TcpStream::connect(("127.0.0.1", port2)).unwrap();
        s2.write_all(b"GET /cb?code=z&state=evil HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        let mut resp2 = String::new();
        let _ = s2.read_to_string(&mut resp2);
        assert!(resp2.contains("400 Bad Request"), "got: {resp2}");
        assert!(h2.join().unwrap().is_err());

        // error param -> rejected with description
        let port3 = free_port();
        let h3 = std::thread::spawn(move || {
            wait_for_callback(port3, "/cb", "st", Duration::from_secs(10))
        });
        std::thread::sleep(Duration::from_millis(150));
        let mut s3 = std::net::TcpStream::connect(("127.0.0.1", port3)).unwrap();
        s3.write_all(
            b"GET /cb?error=access_denied&error_description=nope&state=st HTTP/1.1\r\nHost: x\r\n\r\n",
        )
        .unwrap();
        let mut resp3 = String::new();
        let _ = s3.read_to_string(&mut resp3);
        assert!(resp3.contains("200 OK"));
        let err = h3.join().unwrap().unwrap_err().to_string();
        assert!(
            err.contains("access_denied") || err.contains("nope"),
            "{err}"
        );
    }

    #[test]
    fn discovery_via_www_authenticate() {
        let (_p, log) = spawn_fake_http(|port| {
            let base = format!("http://127.0.0.1:{port}");
            let mut routes = BTreeMap::new();
            routes.insert(
                "/mcp".to_string(),
                (
                    401u16,
                    format!("WWW-Authenticate: Bearer resource_metadata=\"{base}/rs\"\r\n"),
                    json!({"error": "unauthorized"}).to_string(),
                ),
            );
            routes.insert(
                "/rs".to_string(),
                (
                    200,
                    String::new(),
                    json!({
                        "resource": format!("{base}/mcp"),
                        "authorization_servers": [base.clone()]
                    })
                    .to_string(),
                ),
            );
            routes.insert(
                "/.well-known/oauth-authorization-server".to_string(),
                (
                    200,
                    String::new(),
                    json!({
                        "issuer": base,
                        "authorization_endpoint": format!("{base}/authorize"),
                        "token_endpoint": format!("{base}/token"),
                        "registration_endpoint": format!("{base}/register"),
                        "scopes_supported": ["read", "write"]
                    })
                    .to_string(),
                ),
            );
            routes
        });
        let base = format!("http://127.0.0.1:{_p}");
        let http = Client::builder().build().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let ep = match rt.block_on(discover(&http, &format!("{base}/mcp"), None)) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("REQ LOG: {:?}", log.lock().unwrap());
                panic!("discover failed: {e:#}");
            }
        };
        assert_eq!(ep.authorization_endpoint, format!("{base}/authorize"));
        assert_eq!(ep.token_endpoint, format!("{base}/token"));
        assert_eq!(ep.registration_endpoint, Some(format!("{base}/register")));
        assert_eq!(ep.scope.as_deref(), Some("read write"));
        assert_eq!(ep.resource.as_deref(), Some(format!("{base}/mcp").as_str()));
        // probe POST /mcp really happened
        assert!(log
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.starts_with("POST /mcp ")));
    }

    #[test]
    fn discovery_wellknown_fallback_without_401() {
        let (_p, _log) = spawn_fake_http(|port| {
            let base = format!("http://127.0.0.1:{port}");
            let mut routes = BTreeMap::new();
            routes.insert(
                "/api/mcp".to_string(),
                (405, String::new(), json!({"detail": "method"}).to_string()),
            );
            routes.insert(
                "/.well-known/oauth-protected-resource/api/mcp".to_string(),
                (
                    200,
                    String::new(),
                    json!({
                        "resource": format!("{base}/api/mcp"),
                        "authorization_servers": [format!("{base}/as")]
                    })
                    .to_string(),
                ),
            );
            routes.insert(
                "/.well-known/oauth-authorization-server/as".to_string(),
                (
                    200,
                    String::new(),
                    json!({
                        "authorization_endpoint": format!("{base}/as/authorize"),
                        "token_endpoint": format!("{base}/as/token")
                    })
                    .to_string(),
                ),
            );
            routes
        });
        let base = format!("http://127.0.0.1:{_p}");
        let http = Client::builder().build().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let ep = rt
            .block_on(discover(&http, &format!("{base}/api/mcp"), None))
            .unwrap();
        assert_eq!(ep.authorization_endpoint, format!("{base}/as/authorize"));
        assert_eq!(ep.token_endpoint, format!("{base}/as/token"));
        assert_eq!(ep.registration_endpoint, None);
        assert_eq!(ep.scope, None);
    }

    #[test]
    fn resource_pinning_and_pinned_metadata() {
        let _g = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("mcpauth-res-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        set_store_file(Some(dir.join("mcp-auth.json")));

        // pinned metadata: endpoints come straight from the configured url,
        // no 401 probe and no well-known fetches happen
        let (_p, log) = spawn_fake_http(|port| {
            let base = format!("http://127.0.0.1:{port}");
            let mut routes = BTreeMap::new();
            routes.insert(
                "/mcp".to_string(),
                (
                    401u16,
                    format!("WWW-Authenticate: Bearer resource_metadata=\"{base}/rs\"\r\n"),
                    json!({"error": "unauthorized"}).to_string(),
                ),
            );
            routes.insert(
                "/as/meta".to_string(),
                (
                    200,
                    String::new(),
                    json!({
                        "authorization_endpoint": format!("{base}/as/authorize"),
                        "token_endpoint": format!("{base}/as/token"),
                        "registration_endpoint": format!("{base}/as/register"),
                        "scopes_supported": ["read", "write"]
                    })
                    .to_string(),
                ),
            );
            routes.insert(
                "/token".to_string(),
                (
                    200,
                    String::new(),
                    json!({"access_token": "at1", "refresh_token": "rt1", "expires_in": 3600})
                        .to_string(),
                ),
            );
            routes
        });
        let base = format!("http://127.0.0.1:{_p}");
        let http = Client::builder().build().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let oauth = McpOAuthCfg {
            client_id: Some("cid".into()),
            auth_server_metadata_url: Some(format!("{base}/as/meta")),
            ..Default::default()
        };
        let ep = rt
            .block_on(discover(&http, &format!("{base}/mcp"), Some(&oauth)))
            .unwrap();
        assert_eq!(ep.authorization_endpoint, format!("{base}/as/authorize"));
        assert_eq!(ep.token_endpoint, format!("{base}/as/token"));
        assert_eq!(ep.scope.as_deref(), Some("read write"));
        // the resource pins to the mcp server url
        assert_eq!(ep.resource.as_deref(), Some(format!("{base}/mcp").as_str()));
        let reqs = log.lock().unwrap();
        assert!(
            !reqs.iter().any(|r| r.starts_with("POST /mcp")),
            "the 401 probe must be skipped: {reqs:?}"
        );
        assert!(
            !reqs.iter().any(|r| r.contains(".well-known")),
            "well-known discovery must be skipped: {reqs:?}"
        );
        drop(reqs);

        // RFC 8707: a stored resource indicator rides every refresh
        mutate("t", Some(&format!("{base}/mcp")), |e| {
            e.tokens = Some(Tokens {
                access_token: "old".into(),
                refresh_token: Some("rt1".into()),
                ..Default::default()
            });
            e.token_endpoint = Some(format!("{base}/token"));
            e.resource = Some("https://rs.example.com/mcp".into());
        })
        .unwrap();
        let access = rt
            .block_on(do_refresh(
                &http,
                "t",
                &format!("{base}/mcp"),
                Some(&oauth),
                "rt1",
            ))
            .unwrap();
        assert_eq!(access, "at1");
        let reqs = log.lock().unwrap();
        let refresh = reqs
            .iter()
            .find(|r| r.contains("/token"))
            .expect("refresh request captured");
        for part in [
            "grant_type=refresh_token",
            "refresh_token=rt1",
            "client_id=cid",
            "resource=https%3A%2F%2Frs.example.com%2Fmcp",
        ] {
            assert!(refresh.contains(part), "{part} missing in {refresh}");
        }
    }

    #[test]
    fn registration_sends_scope_in_metadata_doc() {
        let (_p, log) = spawn_fake_http(|_| {
            let mut routes = BTreeMap::new();
            routes.insert(
                "/register".to_string(),
                (
                    200,
                    String::new(),
                    json!({"client_id": "cid-new"}).to_string(),
                ),
            );
            routes
        });
        let base = format!("http://127.0.0.1:{_p}");
        let http = Client::builder().build().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let ci = rt
            .block_on(register_client(
                &http,
                &format!("{base}/register"),
                "http://127.0.0.1:19876/mcp/oauth/callback",
                false,
                Some("read write"),
            ))
            .unwrap();
        assert_eq!(ci.client_id, "cid-new");
        let reqs = log.lock().unwrap();
        let reg = reqs
            .iter()
            .find(|r| r.contains("/register"))
            .expect("registration captured");
        for part in [
            "\"client_name\":\"Hi!Derola\"",
            "\"grant_types\":[\"authorization_code\",\"refresh_token\"]",
            "\"response_types\":[\"code\"]",
            "\"token_endpoint_auth_method\":\"none\"",
            "\"scope\":\"read write\"",
        ] {
            assert!(reg.contains(part), "{part} missing in {reg}");
        }
    }

    #[test]
    fn exchange_and_refresh_forms_and_parsing() {
        let mut routes = BTreeMap::new();
        routes.insert(
            "/token".to_string(),
            (
                200,
                String::new(),
                json!({
                    "access_token": "at1",
                    "refresh_token": "rt1",
                    "expires_in": 3600,
                    "scope": "read"
                })
                .to_string(),
            ),
        );
        let (port, log) = spawn_fake_http(|_| routes);
        let base = format!("http://127.0.0.1:{port}");
        let http = Client::builder().build().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let t = rt.block_on(token_request(
            &http,
            &format!("{base}/token"),
            &[
                ("grant_type", "authorization_code"),
                ("code", "c1"),
                ("redirect_uri", "http://127.0.0.1:19876/cb"),
                ("client_id", "cid"),
                ("code_verifier", "v1"),
            ],
        ));
        let t = match t {
            Ok(t) => t,
            Err(e) => panic!("exchange failed: {e:#}"),
        };
        assert_eq!(t.access_token, "at1");
        assert_eq!(t.refresh_token.as_deref(), Some("rt1"));
        assert!(t.expires_at.unwrap() > now_secs() + 3000);
        assert_eq!(t.scope.as_deref(), Some("read"));
        let reqs = log.lock().unwrap();
        let body = reqs
            .iter()
            .find(|r| r.contains("/token"))
            .expect("token request captured");
        assert!(body.starts_with("POST /token"));
        for part in [
            "grant_type=authorization_code",
            "code=c1",
            "client_id=cid",
            "code_verifier=v1",
        ] {
            assert!(body.contains(part), "{part} missing in {body}");
        }

        // refresh keeps old refresh token when response omits it
        let tokens = tokens_from_json(&json!({"access_token": "at2", "expires_in": 60})).unwrap();
        assert_eq!(tokens.access_token, "at2");
        assert_eq!(tokens.refresh_token, None);
    }

    #[test]
    fn store_roundtrip_expiry_and_perms() {
        let _g = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("mcpauth-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("mcp-auth.json");
        set_store_file(Some(file.clone()));

        mutate("srv", Some("http://x/mcp"), |e| {
            e.tokens = Some(Tokens {
                access_token: "a".into(),
                refresh_token: Some("r".into()),
                expires_at: Some(now_secs() + 1000),
                scope: Some("read".into()),
            });
            e.client_info = Some(ClientInfo {
                client_id: "cid".into(),
                client_secret: None,
                client_secret_expires_at: None,
            });
        })
        .unwrap();

        assert_eq!(
            get("srv").unwrap().server_url.as_deref(),
            Some("http://x/mcp")
        );
        assert!(get_for_url("srv", "http://x/mcp").is_some());
        assert!(get_for_url("srv", "http://other/mcp").is_none());
        assert_eq!(is_expired("srv"), Some(false));

        mutate("srv", None, |e| {
            e.tokens.as_mut().unwrap().expires_at = Some(now_secs() - 10);
        })
        .unwrap();
        assert_eq!(is_expired("srv"), Some(true));
        assert_eq!(is_expired("ghost"), None);

        // reload from disk survives (fresh read)
        let loaded = load_all();
        assert_eq!(loaded["srv"].client_info.as_ref().unwrap().client_id, "cid");

        remove("srv");
        assert!(get("srv").is_none());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "store must be 0600");
        }

        set_store_file(None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tokens_json_errors() {
        assert!(tokens_from_json(&json!({})).is_err());
        let t = tokens_from_json(&json!({"access_token": "x"})).unwrap();
        assert_eq!(t.expires_at, None);
        assert_eq!(t.refresh_token, None);
    }

    #[test]
    fn logout_clears_tokens_keeps_client_info() {
        let _g = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("mcpauth-logout-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("mcp-auth.json");
        set_store_file(Some(file.clone()));

        mutate("srv", Some("http://x/mcp"), |e| {
            e.tokens = Some(Tokens {
                access_token: "a".into(),
                refresh_token: Some("r".into()),
                expires_at: Some(now_secs() + 1000),
                scope: Some("read".into()),
            });
            e.client_info = Some(ClientInfo {
                client_id: "cid".into(),
                client_secret: None,
                client_secret_expires_at: None,
            });
            e.code_verifier = Some("verifier".into());
        })
        .unwrap();

        assert!(logout("srv"), "tokens existed");
        let e = get("srv").unwrap();
        assert!(e.tokens.is_none(), "tokens cleared");
        assert_eq!(
            e.client_info.as_ref().unwrap().client_id,
            "cid",
            "dynamic client registration survives a logout"
        );
        assert_eq!(
            e.code_verifier.as_deref(),
            Some("verifier"),
            "a pending pkce flow survives a logout"
        );
        assert_eq!(is_expired("srv"), None, "no tokens -> no expiry");

        assert!(!logout("srv"), "second logout is a no-op");
        assert!(!logout("ghost"), "unknown server is a no-op");

        set_store_file(None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oauth_resume_via_persisted_verifier() {
        let _g = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("mcpauth-resume-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("mcp-auth.json");
        set_store_file(Some(file.clone()));

        let (_p, log) = spawn_fake_http(|port| {
            let base = format!("http://127.0.0.1:{port}");
            let mut routes = BTreeMap::new();
            routes.insert(
                "/.well-known/oauth-protected-resource".to_string(),
                (
                    200,
                    String::new(),
                    json!({
                        "resource": format!("{base}/mcp"),
                        "authorization_servers": [base.clone()]
                    })
                    .to_string(),
                ),
            );
            routes.insert(
                "/.well-known/oauth-authorization-server".to_string(),
                (
                    200,
                    String::new(),
                    json!({
                        "authorization_endpoint": format!("{base}/authorize"),
                        "token_endpoint": format!("{base}/token"),
                        "registration_endpoint": format!("{base}/register")
                    })
                    .to_string(),
                ),
            );
            routes.insert(
                "/register".to_string(),
                (
                    200,
                    String::new(),
                    json!({"client_id": "dyn-cid", "client_secret": "dyn-sec"}).to_string(),
                ),
            );
            routes.insert(
                "/token".to_string(),
                (
                    200,
                    String::new(),
                    json!({"access_token": "at9", "refresh_token": "rt9", "expires_in": 3600, "scope": "read"})
                        .to_string(),
                ),
            );
            routes
        });
        let base = format!("http://127.0.0.1:{_p}");
        let cfgs = vec![McpConfig {
            name: "srv".into(),
            r#type: Some("remote".into()),
            command: String::new(),
            args: vec![],
            env: BTreeMap::new(),
            url: Some(format!("{base}/mcp")),
            headers: BTreeMap::new(),
            oauth: Some(crate::config::McpOAuthOpt::Off(true)),
            sampling: None,
            elicitation: None,
            logging: None,
            keepalive: None,
            timeout: None,
            startup_timeout: None,
            catalog_timeout: None,
            execution_timeout: None,
            enabled: None,
            cwd: None,
            protocol_version: None,
        }];

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        // phase 1: start the flow — the pkce pair, token endpoint and
        // redirect persist, so the flow is resumable after a restart
        let start = rt.block_on(start_auth("srv", &cfgs)).unwrap();
        let entry = get("srv").unwrap();
        assert!(entry.code_verifier.is_some(), "verifier persisted");
        assert_eq!(entry.oauth_state.as_deref(), Some(start.state.as_str()));
        assert_eq!(
            entry.redirect_uri.as_deref(),
            Some(format!("http://127.0.0.1:{CALLBACK_PORT}{CALLBACK_PATH}").as_str())
        );
        assert_eq!(
            entry.token_endpoint.as_deref(),
            Some(format!("{base}/token").as_str())
        );
        assert!(start.url.contains("client_id=dyn-cid"), "{}", start.url);
        assert!(start.url.contains(&format!("state={}", start.state)));

        // phase 2 (simulated restart + pasted code): the persisted verifier
        // completes the exchange
        let msg = rt
            .block_on(finish_auth("srv", &cfgs, " manual-code "))
            .unwrap();
        assert!(msg.contains("authorized"), "{msg}");
        let entry = get("srv").unwrap();
        assert_eq!(entry.tokens.unwrap().access_token, "at9");
        assert!(entry.code_verifier.is_none(), "flow cleared");
        assert!(entry.oauth_state.is_none());
        let body = log
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.contains("/token"))
            .expect("token request captured")
            .clone();
        for part in [
            "grant_type=authorization_code",
            "code=manual-code",
            "client_id=dyn-cid",
            "code_verifier=",
            "redirect_uri=http%3A%2F%2F127.0.0.1%3A19876",
        ] {
            assert!(body.contains(part), "{part} missing in {body}");
        }

        // no pending flow anymore -> a second resume fails cleanly
        let err = rt
            .block_on(finish_auth("srv", &cfgs, "again"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("no pending oauth flow"), "{err}");

        remove("srv");
        set_store_file(None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
