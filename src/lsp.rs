use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use serde_json::json;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;

static ENABLED: AtomicBool = AtomicBool::new(true);

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

const INIT_TIMEOUT: Duration = Duration::from_secs(10);
const DIAG_WAIT: Duration = Duration::from_millis(2500);
const DIAG_QUIET_WINDOW: Duration = Duration::from_millis(1200);
const QUIET: Duration = Duration::from_millis(400);
const MAX_DIAGS: usize = 20;
const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// (binaries to try, supported extensions, extra args)
const REGISTRY: &[(&str, &[&str], &[&str])] = &[
    ("rust-analyzer", &["rs"], &[]),
    ("pyright-langserver", &["py", "pyi"], &["--stdio"]),
    ("basedpyright-langserver", &["py", "pyi"], &["--stdio"]),
    ("pylsp", &["py", "pyi"], &[]),
    ("typescript-language-server", &["ts", "tsx", "js", "jsx", "mjs", "cjs"], &["--stdio"]),
    ("gopls", &["go"], &[]),
    ("clangd", &["c", "h", "cpp", "hpp", "cc", "cxx"], &[]),
];

struct Server {
    stdin: tokio::sync::Mutex<tokio::process::ChildStdin>,
    next_id: AtomicI64,
    pending: Mutex<HashMap<i64, oneshot::Sender<Value>>>,
    diags: Mutex<HashMap<String, (Instant, Vec<Value>)>>,
    open_docs: Mutex<HashSet<String>>,
    /// when this server was spawned; empty nav results from a freshly
    /// spawned server are retried while it is still analyzing
    born: Instant,
}

type Value = serde_json::Value;

static SERVERS: OnceLock<Mutex<HashMap<String, Arc<Server>>>> = OnceLock::new();

fn servers() -> &'static Mutex<HashMap<String, Arc<Server>>> {
    SERVERS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn server_name_for(path: &str) -> Option<&'static str> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    REGISTRY
        .iter()
        .find(|(_, exts, _)| exts.contains(&ext))
        .map(|(name, _, _)| *name)
}

fn path_to_uri(path: &str) -> String {
    let abs = std::fs::canonicalize(path)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| path.to_string());
    let abs = abs.replace('\\', "/");
    let encoded = abs
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"/_-.,~:@!$&'()*+;=".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect::<String>();
    format!("file://{encoded}")
}

/// byte-correct %XX decoding: multi-byte UTF-8 must reassemble from bytes,
/// not be pushed as one char per byte
fn uri_to_path(uri: &str) -> String {
    let rest = uri.strip_prefix("file://").unwrap_or(uri);
    let mut raw: Vec<u8> = Vec::with_capacity(rest.len());
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                raw.push(b);
                i += 3;
                continue;
            }
        }
        raw.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&raw).to_string()
}

/// Parse one LSP frame from the buffer. Returns (bytes consumed, message).
pub fn parse_frame(buf: &[u8]) -> Option<(usize, Value)> {
    let sep = find_subslice(buf, b"\r\n\r\n")?;
    let head = std::str::from_utf8(&buf[..sep]).ok()?;
    let len = head
        .lines()
        .find_map(|l| {
            let v = l.strip_prefix("Content-Length:")?;
            v.trim().parse::<usize>().ok()
        })?;
    let start = sep + 4;
    if buf.len() < start + len {
        return None;
    }
    let body = std::str::from_utf8(&buf[start..start + len]).ok()?;
    let v: Value = serde_json::from_str(body).ok()?;
    Some((start + len, v))
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

async fn get_server(name: &'static str, args: &[&str], root: &str) -> Option<Arc<Server>> {
    {
        let g = servers().lock().unwrap();
        if let Some(s) = g.get(name) {
            return Some(s.clone());
        }
    }
    let bin = crate::fmt::find_on_path(name)?;
    let mut cmd = tokio::process::Command::new(&bin);
    cmd.args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped());
    if std::env::var_os("HI_DEROLA_LSP_DEBUG").is_some() {
        cmd.stderr(std::process::Stdio::inherit());
    } else {
        cmd.stderr(std::process::Stdio::null());
    }
    if std::path::Path::new(root).is_dir() {
        cmd.current_dir(root);
    }
    let mut child = cmd.spawn().ok()?;
    let stdin = child.stdin.take()?;
    let stdout = child.stdout.take()?;

    let srv = Arc::new(Server {
        stdin: tokio::sync::Mutex::new(stdin),
        next_id: AtomicI64::new(1),
        pending: Mutex::new(HashMap::new()),
        diags: Mutex::new(HashMap::new()),
        open_docs: Mutex::new(HashSet::new()),
        born: Instant::now(),
    });

    // reader: resolve pending requests, track publishDiagnostics
    let r = srv.clone();
    tokio::spawn(async move {
        let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
        let mut chunk = [0u8; 8192];
        let mut rx = stdout;
        loop {
            match rx.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
            while let Some((used, msg)) = parse_frame(&buf) {
                buf.drain(..used);
                if std::env::var_os("HI_DEROLA_LSP_DEBUG").is_some() {
                    eprintln!("[lsp <-] {}", serde_json::to_string(&msg).unwrap_or_default().chars().take(300).collect::<String>());
                }
                let id = msg.get("id").and_then(|v| v.as_i64());
                let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("");
                if method == "textDocument/publishDiagnostics" {
                    let uri = msg["params"]["uri"].as_str().unwrap_or("").to_string();
                    let diags = msg["params"]["diagnostics"].as_array().cloned().unwrap_or_default();
                    r.diags
                        .lock()
                        .unwrap()
                        .insert(uri, (Instant::now(), diags));
                } else if let (Some(id), true) = (id, !method.is_empty()) {
                    // server -> client request: answer with an empty result
                    let resp = serde_json::json!({"jsonrpc": "2.0", "id": id, "result": Value::Null});
                    let mut g = r.stdin.lock().await;
                    let _ = write_frame(&mut *g, &resp).await;
                } else if let Some(id) = id {
                    if let Some(tx) = r.pending.lock().unwrap().remove(&id) {
                        let _ = tx.send(msg);
                    }
                }
            }
            if buf.len() > 16 * 1024 * 1024 {
                break;
            }
        }
        let mut g = servers().lock().unwrap();
        if g.get(name).map(|s| Arc::ptr_eq(s, &r)).unwrap_or(false) {
            g.remove(name);
        }
    });

    // server-to-client requests we ignore; answer them to keep some servers happy
    // (handled implicitly: requests without a handler time out on the server side)

    let root_uri = path_to_uri(root);
    let init_result = request(
        &srv,
        "initialize",
        serde_json::json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "rootPath": root,
            "capabilities": {},
            "workspaceFolders": [{"uri": root_uri, "name": "root"}]
        }),
        INIT_TIMEOUT,
    )
    .await;
    if init_result.is_none() {
        let _ = srv.stdin.lock().await.shutdown().await;
        return None;
    }
    let _ = notify(&srv, "initialized", serde_json::json!({})).await;
    servers().lock().unwrap().insert(name.to_string(), srv.clone());
    Some(srv)
}

async fn request(srv: &Server, method: &str, params: Value, timeout: Duration) -> Option<Value> {
    let id = srv.next_id.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = oneshot::channel();
    srv.pending.lock().unwrap().insert(id, tx);
    let msg = serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    {
        let mut g = srv.stdin.lock().await;
        write_frame(&mut *g, &msg).await.ok()?;
    }
    tokio::time::timeout(timeout, rx).await.ok()?.ok()
}

async fn notify(srv: &Server, method: &str, params: Value) -> std::io::Result<()> {
    let msg = serde_json::json!({"jsonrpc": "2.0", "method": method, "params": params});
    let mut g = srv.stdin.lock().await;
    write_frame(&mut *g, &msg).await
}

async fn write_frame(
    w: &mut (impl tokio::io::AsyncWrite + Unpin),
    msg: &Value,
) -> std::io::Result<()> {
    let body = serde_json::to_string(msg)?;
    if std::env::var_os("HI_DEROLA_LSP_DEBUG").is_some() {
        eprintln!("[lsp ->] {} {}", body.get(..120).unwrap_or(&body), body.len());
    }
    w.write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
        .await?;
    w.write_all(body.as_bytes()).await?;
    w.flush().await
}

fn language_id(path: &str) -> &'static str {
    match std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
    {
        "rs" => "rust",
        "py" | "pyi" => "python",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "go" => "go",
        "c" | "h" => "c",
        "cpp" | "hpp" | "cc" | "cxx" => "cpp",
        _ => "plaintext",
    }
}

/// Ensure the document is open/updated in the server with the given text.
async fn sync_doc(srv: &Server, uri: &str, path: &str, text: &str) {
    let was_open = {
        let mut open = srv.open_docs.lock().unwrap();
        let w = open.contains(uri);
        open.insert(uri.to_string());
        w
    };
    if was_open {
        if let Err(e) = notify(
            srv,
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": {"uri": uri, "version": 0},
                "contentChanges": [{"text": text}]
            }),
        )
        .await
        {
            if std::env::var_os("HI_DEROLA_LSP_DEBUG").is_some() {
                eprintln!("[lsp] didChange failed: {e}");
            }
        }
    } else {
        if let Err(e) = notify(
            srv,
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {"uri": uri, "languageId": language_id(path), "version": 0, "text": text}
            }),
        )
        .await
        {
            if std::env::var_os("HI_DEROLA_LSP_DEBUG").is_some() {
                eprintln!("[lsp] didOpen failed: {e}");
            }
        }
    }
}

/// Run diagnostics for a file after an edit. Returns None when nothing useful was collected.
pub async fn diagnose(path: &str) -> Option<String> {
    if !enabled() {
        return None;
    }
    let name = server_name_for(path)?;
    let args: Vec<&str> = REGISTRY
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, _, a)| a.to_vec())
        .unwrap_or_default();
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_FILE_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let root = std::env::current_dir().ok()?;
    let root_s = root.display().to_string();
    let srv = get_server(name, &args, &root_s).await?;

    let uri = path_to_uri(path);
    let started = Instant::now();
    sync_doc(&srv, &uri, path, &text).await;

    let deadline = started + DIAG_WAIT;
    let grace = started + DIAG_QUIET_WINDOW;
    let fresh = loop {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let now = Instant::now();
        let snapshot = {
            let g = srv.diags.lock().unwrap();
            g.get(&uri).map(|(at, list)| (*at, list.clone()))
        };
        match snapshot {
            Some((at, list)) if at >= started && now.duration_since(at) >= QUIET => break list,
            Some((at, list)) if at < started && now >= grace => break list,
            _ if now >= deadline => break snapshot.map(|(_, l)| l).unwrap_or_default(),
            _ => {}
        }
    };

    let lines: Vec<String> = fresh
        .iter()
        .filter(|d| d["severity"].as_i64().unwrap_or(1) <= 2)
        .take(MAX_DIAGS)
        .map(|d| {
            let sev = match d["severity"].as_i64() {
                Some(2) => "warning",
                _ => "error",
            };
            let line = d["range"]["start"]["line"].as_i64().unwrap_or(0) + 1;
            let col = d["range"]["start"]["character"].as_i64().unwrap_or(0) + 1;
            let msg = d["message"].as_str().unwrap_or("").lines().next().unwrap_or("");
            format!("{sev}: {msg} (line {line}, col {col})")
        })
        .collect();
    if lines.is_empty() {
        return None;
    }
    let extra = if lines.len() == MAX_DIAGS { " (more may follow)" } else { "" };
    Some(format!(
        "--- lsp diagnostics ({name}) ---\n{}{extra}",
        lines.join("\n")
    ))
}

// --- navigation (hover / definition / references / symbols) ---

const NAV_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_NAV_RESULTS: usize = 200;
/// a freshly spawned server may not have analyzed the workspace yet: empty
/// navigation answers are retried until either a result shows up, the server
/// is considered warm, or this budget runs out
const NAV_WARMUP: Duration = Duration::from_secs(10);
const WARM_SERVER_AFTER: Duration = Duration::from_secs(90);

/// `request()` resolves with the full JSON-RPC message; look at `result`
/// when present, otherwise at the value itself (bare arrays/objects)
fn nav_result_empty(v: &Value) -> bool {
    let r = v.get("result").unwrap_or(v);
    match r {
        Value::Null => true,
        Value::Array(a) => a.is_empty(),
        // hover answers "no hover" as {"contents": null}
        Value::Object(o) => o.get("contents").map(Value::is_null).unwrap_or(false),
        _ => false,
    }
}

/// send a request; while the server is still cold, retry empty answers and
/// "content modified" style errors (the server is still indexing); returns
/// the bare `result` value
async fn request_warm(
    srv: &Server,
    method: &str,
    params: Value,
    timeout: Duration,
) -> anyhow::Result<Value> {
    use anyhow::Context;
    let deadline = Instant::now() + NAV_WARMUP;
    loop {
        let res = request(srv, method, params.clone(), timeout)
            .await
            .context("LSP request timed out")?;
        let cold = srv.born.elapsed() < WARM_SERVER_AFTER;
        if let Some(err) = res.get("error") {
            if cold && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(150)).await;
                continue;
            }
            anyhow::bail!(
                "LSP error: {}",
                err["message"].as_str().unwrap_or("unknown")
            );
        }
        let result = res.get("result").cloned().unwrap_or(Value::Null);
        if !cold || !nav_result_empty(&result) || Instant::now() >= deadline {
            return Ok(result);
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

/// 0-based line, 0-based character in UTF-16 code units (the LSP wire format)
fn to_lsp_position(text: &str, line1: u32, col1: u32) -> Value {
    let line1 = line1.max(1);
    let col1 = col1.max(1);
    let lines: Vec<&str> = text.split('\n').collect();
    let line0 = (line1 as usize - 1).min(lines.len().saturating_sub(1));
    let units: u32 = lines[line0]
        .chars()
        .take(col1 as usize - 1)
        .map(|c| c.len_utf16() as u32)
        .sum();
    serde_json::json!({"line": line0, "character": units})
}

/// UTF-16 code units on a line -> 1-based character index (what the model passes)
fn from_lsp_units(line_text: &str, units: i64) -> usize {
    let units = units.max(0) as u32;
    let mut seen = 0u32;
    for (i, c) in line_text.chars().enumerate() {
        if seen >= units {
            return i + 1;
        }
        seen += c.len_utf16() as u32;
    }
    line_text.chars().count() + 1
}

struct Prepared {
    srv: Arc<Server>,
    uri: String,
    text: String,
}

async fn prepare(path: &str) -> anyhow::Result<Prepared> {
    use anyhow::Context;
    if !enabled() {
        anyhow::bail!("LSP is disabled (config: [lsp] enabled = false)");
    }
    let name = server_name_for(path)
        .context("no LSP server available for this file type")?;
    let meta = std::fs::metadata(path).ok().context("file not found")?;
    if meta.len() > MAX_FILE_BYTES {
        anyhow::bail!("file too large for LSP ({} bytes)", meta.len());
    }
    let text = std::fs::read_to_string(path).context("file is not valid UTF-8")?;
    let args: Vec<&str> = REGISTRY
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, _, a)| a.to_vec())
        .unwrap_or_default();
    let root = std::env::current_dir().context("no working directory")?;
    let root_s = root.display().to_string();
    let srv = get_server(name, &args, &root_s)
        .await
        .context(format!("{name} failed to start (not installed?)"))?;
    let uri = path_to_uri(path);
    sync_doc(&srv, &uri, path, &text).await;
    Ok(Prepared { srv, uri, text })
}

/// run a position-based LSP request and format the response as compact text
async fn position_nav(
    path: &str,
    line: u32,
    col: u32,
    method: &str,
    extra: Value,
    fmt: fn(&Value, &str, &mut HashMap<String, String>) -> Vec<String>,
) -> anyhow::Result<String> {
    let p = prepare(path).await?;
    let pos = to_lsp_position(&p.text, line, col);
    let mut params = serde_json::json!({
        "textDocument": {"uri": p.uri},
        "position": pos
    });
    if let (Some(obj), Some(src)) = (params.as_object_mut(), extra.as_object()) {
        for (k, v) in src {
            obj.insert(k.clone(), v.clone());
        }
    }
    let res = request_warm(&p.srv, method, params, NAV_TIMEOUT).await?;
    let rel = relative_root();
    let mut cache = HashMap::new();
    let lines = fmt(&res, &rel, &mut cache);
    if lines.is_empty() {
        return Ok(format!("{method}: no results"));
    }
    let mut out = lines.join("\n");
    if lines.len() >= MAX_NAV_RESULTS {
        out.push_str(&format!("\n... truncated at {MAX_NAV_RESULTS} results"));
    }
    Ok(out)
}

/// project-relative display path when the file lives under `base`, else as-is
fn display_path(abs: &str, base: &str) -> String {
    let p = std::path::Path::new(abs);
    let b = std::path::Path::new(base);
    match p.strip_prefix(b) {
        Ok(rel) => rel.display().to_string().replace('\\', "/"),
        Err(_) => abs.replace('\\', "/"),
    }
}

fn relative_root() -> String {
    std::env::current_dir()
        .map(|d| d.display().to_string())
        .unwrap_or_default()
}

/// hover -> the rendered markup, or an empty string
fn fmt_hover(v: &Value) -> String {
    let content = &v["contents"];
    let mut parts: Vec<String> = Vec::new();
    match content {
        Value::String(s) => parts.push(s.clone()),
        Value::Array(a) => {
            for c in a {
                parts.push(hover_part(c));
            }
        }
        Value::Object(_) => parts.push(hover_part(content)),
        _ => {}
    }
    let text: Vec<&str> = parts
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    text.join("\n\n")
}

fn hover_part(c: &Value) -> String {
    if let Some(s) = c.as_str() {
        return s.to_string();
    }
    match c["value"].as_str() {
        Some(v) => {
            let lang = c["language"].as_str().unwrap_or("");
            if lang.is_empty() {
                v.to_string()
            } else {
                format!("```{lang}\n{v}\n```")
            }
        }
        None => String::new(),
    }
}

/// Location | Location[] | LocationLink[] | null -> "path:line:col" lines;
/// the column is converted from UTF-16 units back to 1-based characters using
/// the referenced file's text when it can be read
fn fmt_locations(v: &Value, base: &str, cache: &mut HashMap<String, String>) -> Vec<String> {
    let mut out = Vec::new();
    let items: Vec<&Value> = match v {
        Value::Null => Vec::new(),
        Value::Array(a) => a.iter().collect(),
        Value::Object(_) => vec![v],
        _ => Vec::new(),
    };
    for it in items {
        let Some(uri) = it["uri"].as_str().or_else(|| it["targetUri"].as_str()) else {
            continue;
        };
        // LocationLink uses targetSelectionRange for what was actually pointed at
        let range = if it["targetSelectionRange"].is_object() {
            &it["targetSelectionRange"]
        } else {
            &it["range"]
        };
        let line0 = range["start"]["line"].as_i64().unwrap_or(0);
        let units = range["start"]["character"].as_i64().unwrap_or(0);
        let abs = uri_to_path(uri);
        let col = read_cached(&abs, cache)
            .and_then(|t| line_text_of(&t, line0))
            .map(|l| from_lsp_units(&l, units))
            .unwrap_or(units.max(0) as usize + 1);
        out.push(format!("{}:{}:{col}", display_path(&abs, base), line0 + 1));
    }
    out.dedup();
    out
}

fn read_cached(path: &str, cache: &mut HashMap<String, String>) -> Option<String> {
    if let Some(t) = cache.get(path) {
        return Some(t.clone());
    }
    let t = std::fs::read_to_string(path).ok();
    cache.insert(path.to_string(), t.clone().unwrap_or_default());
    t
}

fn line_text_of(text: &str, line0: i64) -> Option<String> {
    text.split('\n')
        .nth(line0.max(0) as usize)
        .map(|l| l.trim_end_matches('\r').to_string())
}

const SYMBOL_KINDS: &[&str] = &[
    "file", "module", "namespace", "package", "class", "method", "property", "field",
    "constructor", "enum", "interface", "function", "variable", "constant", "string",
    "number", "boolean", "array", "object", "key", "null", "enum member", "struct",
    "event", "operator", "type parameter",
];

fn symbol_kind(k: i64) -> &'static str {
    SYMBOL_KINDS
        .get(k.saturating_sub(1) as usize)
        .copied()
        .unwrap_or("symbol")
}

/// DocumentSymbol[] (hierarchical) or SymbolInformation[] -> flat labeled lines
fn fmt_symbols(v: &Value, base: &str, cache: &mut HashMap<String, String>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(items) = v.as_array() {
        let hierarchical = items
            .first()
            .map(|s| s["range"].is_object() && s["location"].is_null())
            .unwrap_or(false);
        if hierarchical {
            for s in items {
                walk_symbol(s, 0, &mut out);
            }
        } else {
            for s in items {
                let name = s["name"].as_str().unwrap_or("?");
                let kind = symbol_kind(s["kind"].as_i64().unwrap_or(0));
                let container = s["containerName"].as_str().unwrap_or("");
                let loc = &s["location"];
                let line0 = loc["range"]["start"]["line"].as_i64().unwrap_or(0);
                let units = loc["range"]["start"]["character"].as_i64().unwrap_or(0);
                let abs = uri_to_path(loc["uri"].as_str().unwrap_or(""));
                let col = read_cached(&abs, cache)
                    .and_then(|t| line_text_of(&t, line0))
                    .map(|l| from_lsp_units(&l, units))
                    .unwrap_or(units.max(0) as usize + 1);
                let prefix = if container.is_empty() {
                    String::new()
                } else {
                    format!("{container}::")
                };
                out.push(format!(
                    "{kind} {prefix}{name} {}:{}:{col}",
                    display_path(&abs, base),
                    line0 + 1
                ));
            }
        }
    }
    out
}

fn walk_symbol(s: &Value, depth: usize, out: &mut Vec<String>) {
    let name = s["name"].as_str().unwrap_or("?");
    let kind = symbol_kind(s["kind"].as_i64().unwrap_or(0));
    let detail = s["detail"].as_str().unwrap_or("");
    let line = s["selectionRange"]["start"]["line"].as_i64().unwrap_or(0) + 1;
    let detail_part = if detail.trim().is_empty() || detail.contains(name) {
        String::new()
    } else {
        let d: String = detail.lines().next().unwrap_or("").to_string();
        if d.chars().count() > 60 {
            format!(" {}", d.chars().take(57).collect::<String>() + "...")
        } else {
            format!(" {d}")
        }
    };
    let indent = "  ".repeat(depth.min(6));
    out.push(format!("{indent}{kind} {name}{detail_part} :{line}"));
    if out.len() >= MAX_NAV_RESULTS {
        return;
    }
    for c in s["children"].as_array().into_iter().flatten() {
        walk_symbol(c, depth + 1, out);
        if out.len() >= MAX_NAV_RESULTS {
            return;
        }
    }
}

/// LSP hover for a 1-based line/column
pub async fn hover(path: &str, line: u32, col: u32) -> anyhow::Result<String> {
    let p = prepare(path).await?;
    let params = serde_json::json!({
        "textDocument": {"uri": p.uri},
        "position": to_lsp_position(&p.text, line, col)
    });
    let res = request_warm(&p.srv, "textDocument/hover", params, NAV_TIMEOUT).await?;
    let text = fmt_hover(&res);
    if text.is_empty() {
        Ok("hover: no hover information".into())
    } else {
        Ok(text)
    }
}

pub async fn definition(path: &str, line: u32, col: u32) -> anyhow::Result<String> {
    position_nav(path, line, col, "textDocument/definition", json!({}), fmt_locations)
        .await
        .map(|s| s.replace("textDocument/definition", "definition"))
}

pub async fn implementation(path: &str, line: u32, col: u32) -> anyhow::Result<String> {
    position_nav(path, line, col, "textDocument/implementation", json!({}), fmt_locations)
        .await
        .map(|s| s.replace("textDocument/implementation", "implementation"))
}

pub async fn references(
    path: &str,
    line: u32,
    col: u32,
    include_decl: bool,
) -> anyhow::Result<String> {
    let extra = serde_json::json!({"context": {"includeDeclaration": include_decl}});
    position_nav(path, line, col, "textDocument/references", extra, fmt_locations)
        .await
        .map(|s| s.replace("textDocument/references", "references"))
}

pub async fn document_symbols(path: &str) -> anyhow::Result<String> {
    let p = prepare(path).await?;
    let params = serde_json::json!({"textDocument": {"uri": p.uri}});
    let res = request_warm(&p.srv, "textDocument/documentSymbol", params, NAV_TIMEOUT).await?;
    let rel = relative_root();
    let mut cache = HashMap::new();
    let lines = fmt_symbols(&res, &rel, &mut cache);
    if lines.is_empty() {
        return Ok("document_symbols: no symbols".into());
    }
    let mut out = lines.join("\n");
    if lines.len() >= MAX_NAV_RESULTS {
        out.push_str(&format!("\n... truncated at {MAX_NAV_RESULTS} symbols"));
    }
    Ok(out)
}

pub async fn workspace_symbols(query: &str) -> anyhow::Result<String> {
    if query.trim().is_empty() {
        anyhow::bail!("workspace_symbols: query required");
    }
    let root = std::env::current_dir().context("no working directory")?;
    let root_s = root.display().to_string();
    // any language server can answer; prefer rust-analyzer when present
    let mut chosen: Option<(&'static str, Vec<&str>)> = None;
    for (name, _, args) in REGISTRY {
        if crate::fmt::find_on_path(name).is_some() {
            chosen = Some((name, args.to_vec()));
            break;
        }
    }
    let Some((name, args)) = chosen else {
        anyhow::bail!("no LSP server installed for workspace symbols");
    };
    let srv = get_server(name, &args, &root_s)
        .await
        .context(format!("{name} failed to start"))?;
    let params = serde_json::json!({"query": query});
    let res = request_warm(&srv, "workspace/symbol", params, NAV_TIMEOUT).await?;
    let base = root_s.clone();
    let mut cache = HashMap::new();
    let lines = fmt_symbols(&res, &base, &mut cache);
    if lines.is_empty() {
        return Ok(format!("workspace_symbols {query}: no results"));
    }
    let mut out = lines.join("\n");
    if lines.len() >= MAX_NAV_RESULTS {
        out.push_str(&format!("\n... truncated at {MAX_NAV_RESULTS} symbols"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames() {
        let msg = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"x"});
        let body = serde_json::to_string(&msg).unwrap();
        let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let buf = frame.as_bytes();
        let (used, v) = parse_frame(buf).unwrap();
        assert_eq!(used, buf.len());
        assert_eq!(v["id"], 1);
        assert!(parse_frame(b"Content-Length: 99\r\n\r\n{}").is_none());
        assert!(parse_frame(b"garbage").is_none());
    }

    #[test]
    fn uri_roundtrip() {
        let p = "/tmp/some dir/file.rs";
        let uri = path_to_uri(p);
        assert!(uri.starts_with("file://"));
        assert!(uri.contains("%20"));
        assert_eq!(uri_to_path(&uri), p);
    }

    #[test]
    fn registry_lookup() {
        assert_eq!(server_name_for("a/b/main.rs"), Some("rust-analyzer"));
        assert_eq!(server_name_for("x.py"), Some("pyright-langserver"));
        assert_eq!(server_name_for("nope.txt"), None);
    }

    #[test]
    fn uri_non_ascii_roundtrip() {
        let p = "/tmp/данные папка/file.rs";
        let uri = path_to_uri(p);
        assert_eq!(uri_to_path(&uri), p);
    }

    #[test]
    fn position_conversion_roundtrip() {
        // BMP unicode before the column still counts 1 UTF-16 unit per char
        let text = "日本語abc\ndef";
        let pos = to_lsp_position(text, 1, 4);
        assert_eq!(pos["line"], 0);
        assert_eq!(pos["character"], 3);
        let line = line_text_of(text, 0).unwrap();
        assert_eq!(from_lsp_units(&line, pos["character"].as_i64().unwrap()), 4);
        // non-BMP chars (emoji) take 2 UTF-16 units each; col 6 points at 'x'
        let crab = "let 🦀x = 1;";
        let pos = to_lsp_position(crab, 1, 6);
        assert_eq!(pos["character"], 6, "l,e,t,space=4 units + 2 units for the crab");
        let line = line_text_of(crab, 0).unwrap();
        assert_eq!(from_lsp_units(&line, pos["character"].as_i64().unwrap()), 6);
        // plain ascii
        let pos = to_lsp_position("hello\nworld", 2, 3);
        assert_eq!(pos["line"], 1);
        assert_eq!(pos["character"], 2);
        // out-of-range line clamps to the last line
        let pos = to_lsp_position("one\ntwo", 99, 1);
        assert_eq!(pos["line"], 1);
        assert_eq!(pos["character"], 0);
        // 1-based always: line 0 / col 0 behave as 1/1
        let pos = to_lsp_position("hello", 0, 0);
        assert_eq!(pos["line"], 0);
        assert_eq!(pos["character"], 0);
    }

    #[test]
    fn hover_formatting() {
        let markup = serde_json::json!({"contents": {"kind": "markdown", "value": "fn **main**\n\ndocs"}});
        assert_eq!(fmt_hover(&markup), "fn **main**\n\ndocs");
        let plain = serde_json::json!({"contents": {"language": "rust", "value": "let x: i32"}});
        assert_eq!(fmt_hover(&plain), "```rust\nlet x: i32\n```");
        let arr = serde_json::json!({"contents": ["a", {"language": "py", "value": "b"}]});
        assert_eq!(fmt_hover(&arr), "a\n\n```py\nb\n```");
        assert_eq!(fmt_hover(&serde_json::json!(null)), "");
    }

    #[test]
    fn locations_formatting() {
        let dir = std::env::temp_dir().join(format!("hiderola-lsp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("t.rs");
        std::fs::write(&f, "fn main() {}\nlet 🦀И = 1;\n").unwrap();
        let base = dir.display().to_string();
        let mut cache = HashMap::new();

        // single Location
        let one = serde_json::json!({"uri": path_to_uri(&f.display().to_string()), "range": {"start": {"line": 0, "character": 3}}});
        let lines = fmt_locations(&one, &base, &mut cache);
        assert_eq!(lines, vec!["t.rs:1:4"]);

        // LocationLink (targetSelectionRange wins) + array + dedup + null;
        // the crab emoji takes 2 UTF-16 units: unit offset 6 lands on char 6
        let link = serde_json::json!({"targetUri": path_to_uri(&f.display().to_string()), "targetSelectionRange": {"start": {"line": 1, "character": 6}}, "targetRange": {"start": {"line": 1, "character": 0}}});
        let lines = fmt_locations(&serde_json::json!([link, one, one]), &base, &mut cache);
        assert_eq!(lines, vec!["t.rs:2:6", "t.rs:1:4"]);

        assert!(fmt_locations(&serde_json::json!(null), &base, &mut cache).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn symbols_formatting() {
        let dir = std::env::temp_dir().join(format!("hiderola-lsps-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("t.rs");
        std::fs::write(&f, "struct S;\nfn go() {}\n").unwrap();
        let base = dir.display().to_string();
        let mut cache = HashMap::new();

        let hierarchical = serde_json::json!([
            {"name": "S", "kind": 23, "detail": "struct S", "range": {"start": {"line": 0}}, "selectionRange": {"start": {"line": 0, "character": 7}}, "children": [
                {"name": "new", "kind": 9, "range": {"start": {"line": 0}}, "selectionRange": {"start": {"line": 0, "character": 8}}}
            ]}
        ]);
        let lines = fmt_symbols(&hierarchical, &base, &mut cache);
        assert_eq!(lines[0], "struct S :1");
        assert_eq!(lines[1], "  constructor new :1");

        let flat = serde_json::json!([
            {"name": "go", "kind": 12, "containerName": "", "location": {"uri": path_to_uri(&f.display().to_string()), "range": {"start": {"line": 1, "character": 3}}}},
            {"name": "Д", "kind": 13, "containerName": "go", "location": {"uri": path_to_uri(&f.display().to_string()), "range": {"start": {"line": 9, "character": 6}}}}
        ]);
        let lines = fmt_symbols(&flat, &base, &mut cache);
        assert_eq!(lines[0], "function go t.rs:2:4");
        // line 9 does not exist -> no conversion, raw units+1
        assert_eq!(lines[1], "variable go::Д t.rs:10:7");

        assert!(fmt_symbols(&serde_json::json!(null), &base, &mut cache).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn symbol_kind_names() {
        assert_eq!(symbol_kind(5), "class");
        assert_eq!(symbol_kind(12), "function");
        assert_eq!(symbol_kind(999), "symbol");
    }

    /// end-to-end navigation against the real workspace; skipped silently
    /// when rust-analyzer is not installed
    #[tokio::test]
    async fn nav_smoke_with_rust_analyzer() {
        if crate::fmt::find_on_path("rust-analyzer").is_none() {
            eprintln!("rust-analyzer not on PATH, skipping");
            return;
        }
        let repo = std::env::current_dir().unwrap();
        let app = repo.join("src").join("app.rs");
        let snap = repo.join("src").join("snapshot.rs");

        // locate `crate::snapshot::begin_turn();` in app.rs and point at the
        // function name (a module-path segment may not resolve)
        let app_text = std::fs::read_to_string(&app).unwrap();
        let (line, col) = find(&app_text, "begin_turn();").expect("call site");
        let Ok(def) = definition(&app.display().to_string(), line, col).await else {
            // the PATH probe may hit a rustup shim without the component
            eprintln!("rust-analyzer failed to start, skipping");
            return;
        };
        eprintln!("definition -> {def}");
        assert!(def.contains("snapshot.rs:2"), "must point at the definition: {def}");

        let hover_text = hover(&app.display().to_string(), line, col).await.unwrap();
        eprintln!("hover -> {hover_text}");
        assert!(hover_text.contains("fn begin_turn"), "{hover_text}");

        // references from the definition site in snapshot.rs (line 25, col 13)
        let refs = references(&snap.display().to_string(), 25, 13, false).await.unwrap();
        eprintln!("references -> {refs}");
        assert!(refs.contains("app.rs:"), "{refs}");

        let syms = document_symbols(&snap.display().to_string()).await.unwrap();
        eprintln!("document_symbols -> {syms}");
        assert!(syms.contains("function begin_turn"), "{syms}");
    }

    /// 1-based line/column of the first occurrence of `needle`
    fn find(text: &str, needle: &str) -> Option<(u32, u32)> {
        let idx = text.find(needle)?;
        let line = text[..idx].matches('\n').count() + 1;
        let col = text[..idx].rsplit('\n').next().unwrap_or("").chars().count() + 1;
        Some((line as u32, col as u32))
    }
}
