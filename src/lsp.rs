use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

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

#[cfg_attr(not(test), allow(dead_code))]
fn uri_to_path(uri: &str) -> String {
    let rest = uri.strip_prefix("file://").unwrap_or(uri);
    let mut out = String::new();
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                out.push(b as char);
                i += 3;
                continue;
            }
        }
        out.push(rest[i..].chars().next().unwrap_or('?'));
        i += rest[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
    }
    out
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
}
