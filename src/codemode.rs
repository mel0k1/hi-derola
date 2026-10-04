//! code-mode: a confined JavaScript sandbox for orchestrating MCP tools.
//!
//! The model writes a small JS program; the sandbox (boa_engine, pure Rust,
//! no filesystem/network/process authority) exposes every connected MCP tool
//! as `mcp.<server>.<tool>(input)`. The program can sequence, transform,
//! branch and loop over results in one turn instead of one tool call per
//! round trip. Each child MCP call goes through the same permission checks
//! (and the same confirm UI) as a direct `mcp__server__tool` call.

use boa_engine::{
    js_string, native_function::NativeFunction, object::builtins::JsPromise,
    object::JsObject, property::Attribute, builtins::promise::PromiseState, Context,
    JsError, JsNativeError, JsResult, JsValue, Source,
};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::mcp::McpClient;
use crate::perm::{Perm, PermCfg, PermRule};
use crate::provider::{ConfirmReply, ToolSpec};

const MAX_CODE_BYTES: usize = 256 * 1024;
/// runaway protection for the interpreter itself
const MAX_LOOP_ITERATIONS: u64 = 5_000_000;
const MAX_RECURSION: usize = 96;
const MAX_TOOL_CALLS: usize = 200;
const MAX_LOGS: usize = 200;
const MAX_LOG_LINE: usize = 2_000;
const CATALOG_BUDGET: usize = 6_000;

/// wall-clock budget for one script; child MCP calls have their own timeouts
/// too, so a script always finishes close to this
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(180);

pub type CallFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, String>> + Send>>;
pub type Caller = Arc<dyn Fn(Value) -> CallFuture + Send + Sync>;

pub struct Target {
    pub server: String,
    pub tool: String,
    /// the permission subject, e.g. "mcp__github__search_repo"
    pub perm_key: String,
    pub description: String,
    pub call: Caller,
}

pub type ConfirmFn = Arc<dyn Fn(&str, &str) -> ConfirmReply + Send + Sync>;

pub struct RunCfg {
    pub targets: Vec<Target>,
    /// snapshot of the session permissions; child calls are checked against it
    pub perm: PermCfg,
    pub allow_all: Arc<AtomicBool>,
    pub timeout: Duration,
    pub confirm: ConfirmFn,
    pub note: Arc<dyn Fn(String) + Send + Sync>,
}

#[derive(Debug)]
pub struct Outcome {
    pub output: Result<String, String>,
    pub logs: Vec<String>,
    pub calls: usize,
    /// allow-rules created by "always allow" confirms during the run
    pub new_rules: Vec<PermRule>,
}

struct RunState {
    targets: Vec<Target>,
    perm: PermCfg,
    new_rules: Vec<PermRule>,
    allow_all: Arc<AtomicBool>,
    confirm: ConfirmFn,
    note: Arc<dyn Fn(String) + Send + Sync>,
    logs: Vec<String>,
    deadline: Instant,
    calls: usize,
    handle: tokio::runtime::Handle,
}

thread_local! {
    static RUN: RefCell<Option<RunState>> = const { RefCell::new(None) };
}

enum Decision {
    Allow,
    Deny(String),
}

fn tool_native(idx: usize, _this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    // phase 1: validate, count the call, clone what we need, release the
    // borrow before any long host IO
    let target = RUN.with(|r| {
        let mut g = r.borrow_mut();
        let st = g
            .as_mut()
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("code: no run state")))?;
        let t = st
            .targets
            .get(idx)
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("code: unknown tool binding")))?;
        st.calls += 1;
        if st.calls > MAX_TOOL_CALLS {
            return Err(JsError::from(JsNativeError::error().with_message(format!(
                "code: too many tool calls (limit {MAX_TOOL_CALLS})"
            )))
            .into());
        }
        if Instant::now() >= st.deadline {
            return Err(JsError::from(JsNativeError::error().with_message("code: time budget exceeded")));
        }
        let target = Target {
            server: t.server.clone(),
            tool: t.tool.clone(),
            perm_key: t.perm_key.clone(),
            description: t.description.clone(),
            call: t.call.clone(),
        };
        (st.note)(format!("code: {}.{} ...", target.server, target.tool));
        Ok(target)
    })?;

    let arg: Value = match args.len() {
        0 => Value::Null,
        1 => args[0].to_json(ctx)?,
        _ => Value::Array(
            args.iter()
                .map(|v| v.to_json(ctx))
                .collect::<JsResult<Vec<_>>>()?,
        ),
    };
    let arg_str = arg.to_string();

    // phase 2: permission — the same rules as a direct mcp__server__tool call
    let decision: Decision = RUN.with(|r| {
        let mut g = r.borrow_mut();
        let Some(st) = g.as_mut() else {
            return Decision::Deny("code: no run state".into());
        };
        let mut p = st.perm.check(&target.perm_key, &arg_str);
        if p == Perm::Ask && st.allow_all.load(Ordering::Relaxed) {
            p = Perm::Allow;
        }
        if p == Perm::Ask {
            let reply = (st.confirm)(&target.perm_key, &arg_str);
            if reply.approved {
                p = Perm::Allow;
                if reply.always {
                    if let Some(rule) = crate::perm::derive_rule(&target.perm_key, &arg_str) {
                        st.perm.rules.push(rule.clone());
                        st.new_rules.push(rule);
                    }
                }
            } else if reply.feedback.trim().is_empty() {
                p = Perm::Deny;
            } else {
                return Decision::Deny(format!(
                    "user denied the {} call with feedback: {}",
                    target.perm_key,
                    reply.feedback.trim()
                ));
            }
        }
        match p {
            Perm::Allow => Decision::Allow,
            _ => Decision::Deny(format!("user denied the {} call", target.perm_key)),
        }
    });

    let Decision::Deny(msg) = &decision else {
        // allowed: run the host call
        let fut = (target.call)(arg);
        let res = RUN.with(|r| {
            let mut g = r.borrow_mut();
            // no JS runs on this thread while we block, so holding the borrow
            // across block_on is safe (the interpreter is strictly sequential)
            let st = g
                .as_mut()
                .ok_or_else(|| JsError::from(JsNativeError::error().with_message("code: no run state")))?;
            let started = Instant::now();
            match st.handle.block_on(fut) {
                Ok(v) => {
                    let ms = started.elapsed().as_millis();
                    (st.note)(format!(
                        "code: {}.{} ok ({ms}ms)",
                        target.server, target.tool
                    ));
                    Ok(v)
                }
                Err(e) => {
                    (st.note)(format!(
                        "code: {}.{} failed: {e}",
                        target.server, target.tool
                    ));
                    Err(JsError::from(JsNativeError::error().with_message(e)))
                }
            }
        })?;
        return JsValue::from_json(&res, ctx);
    };
    Err(JsError::from(JsNativeError::error().with_message(msg.clone())))
}

fn console_native(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let mut line = String::new();
    for v in args {
        if !line.is_empty() {
            line.push(' ');
        }
        if v.is_object() {
            let text = v
                .to_json(ctx)
                .ok()
                .and_then(|j| serde_json::to_string(&j).ok())
                .unwrap_or_else(|| v.to_string(ctx).unwrap_or_default().to_std_string_escaped());
            line.push_str(&text);
        } else {
            line.push_str(&v.to_string(ctx)?.to_std_string_escaped());
        }
    }
    RUN.with(|r| {
        if let Some(st) = r.borrow_mut().as_mut() {
            if st.logs.len() < MAX_LOGS {
                st.logs.push(line.chars().take(MAX_LOG_LINE).collect());
            } else if st.logs.len() == MAX_LOGS {
                st.logs.push("... (log limit reached)".into());
            }
        }
    });
    Ok(JsValue::undefined())
}

fn build_objects(ctx: &mut Context) -> JsResult<()> {
    let count = RUN.with(|r| r.borrow().as_ref().map(|s| s.targets.len()).unwrap_or(0));
    let mut groups: BTreeMap<String, Vec<(String, usize)>> = BTreeMap::new();
    for idx in 0..count {
        let (server, tool) = RUN.with(|r| {
            let g = r.borrow();
            let st = g.as_ref().unwrap();
            (st.targets[idx].server.clone(), st.targets[idx].tool.clone())
        });
        groups.entry(sanitize(&server)).or_default().push((sanitize(&tool), idx));
    }
    let mcp = JsObject::with_object_proto(ctx.intrinsics());
    for (server, tools) in groups {
        let srv_obj = JsObject::with_object_proto(ctx.intrinsics());
        let mut used: HashSet<String> = HashSet::new();
        for (tool, idx) in tools {
            let mut name = tool.clone();
            let mut n = 2;
            while !used.insert(name.clone()) {
                name = format!("{tool}_{n}");
                n += 1;
            }
            let f = NativeFunction::from_copy_closure(move |t, args, ctx| {
                tool_native(idx, t, args, ctx)
            })
            .to_js_function(ctx.realm());
            srv_obj.set(js_string!(name.as_str()), f, false, ctx)?;
        }
        mcp.set(js_string!(server.as_str()), srv_obj, false, ctx)?;
    }
    ctx.register_global_property(js_string!("mcp"), mcp, Attribute::all())?;
    let console = JsObject::with_object_proto(ctx.intrinsics());
    let log = NativeFunction::from_fn_ptr(console_native).to_js_function(ctx.realm());
    for name in ["log", "info", "warn", "error"] {
        console.set(js_string!(name), log.clone(), false, ctx)?;
    }
    ctx.register_global_property(js_string!("console"), console, Attribute::all())?;
    Ok(())
}

fn run_in_thread(code: &str, cfg: RunCfg, handle: tokio::runtime::Handle) -> Outcome {
    let timeout = cfg.timeout;
    let state = RunState {
        targets: cfg.targets,
        perm: cfg.perm,
        new_rules: Vec::new(),
        allow_all: cfg.allow_all,
        confirm: cfg.confirm,
        note: cfg.note,
        logs: Vec::new(),
        deadline: Instant::now() + timeout,
        calls: 0,
        handle,
    };

    let mut outcome = Outcome {
        output: Ok(String::new()),
        logs: Vec::new(),
        calls: 0,
        new_rules: Vec::new(),
    };

    if code.len() > MAX_CODE_BYTES {
        outcome.output = Err(format!("code: script too large ({} bytes)", code.len()));
        return outcome;
    }

    let result: Result<String, String> = {
        let mut ctx = Context::default();
        {
            let lim = ctx.runtime_limits_mut();
            lim.set_loop_iteration_limit(MAX_LOOP_ITERATIONS);
            lim.set_recursion_limit(MAX_RECURSION);
        }
        RUN.with(|r| *r.borrow_mut() = Some(state));
        let setup = build_objects(&mut ctx);
        match setup {
            Err(e) => Err(e.to_string()),
            Ok(()) => {
                // wrap in an async IIFE so `await` works at the top level
                let wrapped = format!("(async () => {{\n{code}\n}})()");
                match ctx.eval(Source::from_bytes(wrapped.as_bytes())) {
                    Ok(promise) => {
                        let _ = ctx.run_jobs();
                        settle_text(promise, &mut ctx)
                    }
                    Err(e) => Err(e.to_string()),
                }
            }
        }
    };

    RUN.with(|r| {
        let mut g = r.borrow_mut();
        if let Some(st) = g.as_mut() {
            outcome.logs = std::mem::take(&mut st.logs);
            outcome.calls = st.calls;
            outcome.new_rules = std::mem::take(&mut st.new_rules);
        }
        *g = None;
    });

    outcome.output = match result {
        Ok(mut text) => {
            if !outcome.logs.is_empty() {
                text.push_str("\n\nLogs:\n");
                text.push_str(&outcome.logs.join("\n"));
            }
            Ok(text)
        }
        Err(mut msg) => {
            if !outcome.logs.is_empty() {
                msg.push_str("\n\nLogs:\n");
                msg.push_str(&outcome.logs.join("\n"));
            }
            Err(msg)
        }
    };
    outcome
}

/// drain microtasks and render the settled state of the script's promise
fn settle_text(promise: JsValue, ctx: &mut Context) -> Result<String, String> {
    let is_promise = promise
        .as_object()
        .map(|o| JsPromise::from_object(o.clone()).is_ok())
        .unwrap_or(false);
    if is_promise {
        let p = JsPromise::from_object(promise.as_object().unwrap().clone()).unwrap();
        return match p.state() {
            PromiseState::Fulfilled(v) => Ok(js_value_text(v, ctx)),
            PromiseState::Rejected(err) => Err(js_error_text(&err, ctx)),
            PromiseState::Pending => Err("code: script did not settle (pending promise)".into()),
        };
    }
    Ok(js_value_text(promise, ctx))
}

pub(crate) fn js_value_text(v: JsValue, ctx: &mut Context) -> String {
    if v.is_undefined() {
        return "undefined".into();
    }
    if let Some(s) = v.as_string() {
        return s.to_std_string_escaped();
    }
    match v.to_json(ctx) {
        Ok(j) => serde_json::to_string_pretty(&j).unwrap_or_else(|_| j.to_string()),
        Err(_) => v
            .to_string(ctx)
            .map(|s| s.to_std_string_escaped())
            .unwrap_or_else(|_| "unserializable result".into()),
    }
}

pub(crate) fn js_error_text(v: &JsValue, ctx: &mut Context) -> String {
    v.to_string(ctx)
        .map(|s| s.to_std_string_escaped())
        .unwrap_or_else(|_| "script error".into())
}

pub async fn run(code: String, cfg: RunCfg) -> Outcome {
    let handle = tokio::runtime::Handle::current();
    let timeout = cfg.timeout;
    let work = tokio::task::spawn_blocking(move || run_in_thread(&code, cfg, handle));
    // the in-interpreter deadline already bounds every call, so this outer
    // guard only matters if the interpreter itself wedges
    match tokio::time::timeout(timeout + Duration::from_secs(2), work).await {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(e)) => Outcome {
            output: Err(format!("code: worker crashed: {e}")),
            logs: Vec::new(),
            calls: 0,
            new_rules: Vec::new(),
        },
        Err(_) => Outcome {
            output: Err(format!("code: timed out ({}s)", timeout.as_secs())),
            logs: Vec::new(),
            calls: 0,
            new_rules: Vec::new(),
        },
    }
}

pub fn default_timeout() -> Duration {
    DEFAULT_TIMEOUT
}

pub(crate) fn sanitize(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    if s.is_empty() {
        s = "_".into();
    }
    if s.chars().next().unwrap().is_ascii_digit() {
        s.insert(0, '_');
    }
    s
}

fn js_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// exact call signature for the catalog: dot notation when both segments are
/// plain identifiers, bracket notation otherwise
fn signature(server_js: &str, tool_js: &str) -> String {
    if js_ident(server_js) && js_ident(tool_js) {
        format!("mcp.{server_js}.{tool_js}({{...}})")
    } else {
        format!("mcp[\"{server_js}\"][\"{tool_js}\"]({{...}})")
    }
}

/// build the MCP call targets from the connected client and its tool specs
pub fn mcp_targets(client: Arc<McpClient>, specs: &[ToolSpec]) -> Vec<Target> {
    specs
        .iter()
        .filter_map(|s| {
            let rest = s.name.strip_prefix("mcp__")?;
            let (server, tool) = rest.split_once("__")?;
            let c = client.clone();
            let key = rest.to_string();
            Some(Target {
                server: server.to_string(),
                tool: tool.to_string(),
                perm_key: s.name.clone(),
                description: s.description.clone(),
                call: Arc::new(move |args| {
                    let c = c.clone();
                    let key = key.clone();
                    Box::pin(async move {
                        let text = c.call(&key, &args.to_string()).await.map_err(|e| e.to_string())?;
                        Ok(parse_mcp_text(text))
                    })
                }),
            })
        })
        .collect()
}

/// MCP results arrive as plain text; hand the program real JSON when the
/// text parses, so `result.items` works instead of string surgery
fn parse_mcp_text(text: String) -> Value {
    let t = text.trim();
    if (t.starts_with('{') && t.ends_with('}')) || (t.starts_with('[') && t.ends_with(']')) {
        if let Ok(v) = serde_json::from_str::<Value>(t) {
            return v;
        }
    }
    Value::String(text)
}

/// the compact tool catalog embedded into the `code` tool description
pub fn catalog_from_mcp_specs(specs: &[ToolSpec]) -> String {
    let mut groups: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
    for s in specs {
        let Some(rest) = s.name.strip_prefix("mcp__") else {
            continue;
        };
        let Some((server, tool)) = rest.split_once("__") else {
            continue;
        };
        groups.entry(server).or_default().push((tool, s.description.as_str()));
    }
    if groups.is_empty() {
        return String::new();
    }
    let mut with_desc = String::new();
    let mut signatures = String::new();
    for (server, tools) in &groups {
        let head = |n: usize| {
            format!(
                "- {} ({} tool{})\n",
                sanitize(server),
                n,
                if n == 1 { "" } else { "s" }
            )
        };
        with_desc.push_str(&head(tools.len()));
        signatures.push_str(&head(tools.len()));
        for (tool, desc) in tools {
            let sj = sanitize(server);
            let tj = sanitize(tool);
            with_desc.push_str(&format!("  - {} // {}\n", signature(&sj, &tj), short_desc(desc)));
            signatures.push_str(&format!("  - {}\n", signature(&sj, &tj)));
        }
    }
    if with_desc.len() <= CATALOG_BUDGET {
        return with_desc;
    }
    if signatures.len() <= CATALOG_BUDGET {
        return signatures;
    }
    let mut cut = CATALOG_BUDGET;
    while cut > 0 && !signatures.is_char_boundary(cut) {
        cut -= 1;
    }
    format!(
        "{}... (catalog truncated; use Object.keys(mcp) inside the sandbox)",
        &signatures[..cut]
    )
}

fn short_desc(desc: &str) -> String {
    let line = desc.lines().next().unwrap_or("").trim();
    let mut s: String = line.chars().take(119).collect();
    if line.chars().count() > 119 {
        s.push('…');
    }
    s
}

/// the `code` tool spec; `catalog` is the output of catalog_from_mcp_specs
pub fn code_spec(catalog: &str) -> ToolSpec {
    let description = format!(
        "Run a confined JavaScript program with access to the connected MCP servers. \
Inside the sandbox the global `mcp` object holds every MCP tool as an async function: \
`const r = await mcp.<server>.<tool>({{...input}})`. Use it to run many MCP calls in one step, \
transform, filter or aggregate their results in code, branch and loop. \
The program has NO filesystem, network or process access - only the MCP tools listed below. \
Wrap fallible calls in try/catch (a denied or failing tool throws). \
Return the final value with `return`; console.log lines come back as Logs. \
Return only the fields you need, not raw payloads.\n\n## Available tools\n{catalog}"
    );
    ToolSpec {
        name: "code".into(),
        description,
        parameters: json!({
            "type": "object",
            "properties": {
                "code": {"type": "string", "description": "JavaScript program body (may use await, loops, try/catch); call MCP tools via mcp.<server>.<tool>(input); end with a return statement"}
            },
            "required": ["code"]
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with(targets: Vec<Target>, perm: PermCfg) -> RunCfg {
        RunCfg {
            targets,
            perm,
            allow_all: Arc::new(AtomicBool::new(false)),
            timeout: Duration::from_secs(15),
            confirm: Arc::new(|_, _| ConfirmReply { approved: true, feedback: String::new(), always: false }),
            note: Arc::new(|_: String| {}),
        }
    }

    fn echo_target(server: &str, tool: &str, perm_key: &str) -> Target {
        Target {
            server: server.into(),
            tool: tool.into(),
            perm_key: perm_key.into(),
            description: "echo test tool".into(),
            call: Arc::new(|args| Box::pin(async move { Ok(json!({"echo": args, "n": 2})) })),
        }
    }

    /// mirrors the production path: the interpreter runs on a blocking
    /// thread, so Handle::block_on inside tool calls is legal
    async fn run_code(code: &str, cfg: RunCfg) -> Outcome {
        let code = code.to_string();
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || run_in_thread(&code, cfg, handle))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn orchestration_and_logs() {
        let targets = vec![
            echo_target("t1", "a", "mcp__t1__a"),
            Target {
                server: "t1".into(),
                tool: "b".into(),
                perm_key: "mcp__t1__b".into(),
                description: "failer".into(),
                call: Arc::new(|_| Box::pin(async { Err("boom".to_string()) })),
            },
        ];
        let out = run_code(
            r#"
            const a = await mcp.t1.a({x: 1});
            const rows = [10, 20].map(i => i * a.n);
            let caught = "";
            try { await mcp.t1.b({}); } catch (e) { caught = String(e); }
            console.log("rows", rows.length);
            return { sum: rows[0] + rows[1], caught, first: rows[0] };
        "#,
            cfg_with(targets, PermCfg::default()),
        )
        .await;
        let text = out.output.unwrap();
        assert!(text.contains("\"sum\": 60"), "{text}");
        assert!(text.contains("\"caught\": \"Error: boom\""), "{text}");
        assert!(text.contains("Logs:"), "{text}");
        assert!(text.contains("rows 2"), "{text}");
        assert_eq!(out.calls, 2);
        assert!(out.new_rules.is_empty());
    }

    #[tokio::test]
    async fn perm_ask_and_always_rule() {
        let asks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let asks2 = asks.clone();
        let mut cfg = cfg_with(
            vec![echo_target("srv", "tool", "mcp__srv__tool")],
            PermCfg::default(), // mcp defaults to ask
        );
        cfg.confirm = Arc::new(move |_, _| {
            asks2.fetch_add(1, Ordering::Relaxed);
            ConfirmReply { approved: true, feedback: String::new(), always: true }
        });
        let out = run_code(
            r#"
            const a = await mcp.srv.tool({i: 1});
            const b = await mcp.srv.tool({i: 2});
            return a.n + b.n;
        "#,
            cfg,
        )
        .await;
        assert_eq!(out.output.unwrap().trim(), "4");
        // the first call asks; the always-rule covers the second
        assert_eq!(asks.load(Ordering::Relaxed), 1);
        assert_eq!(out.new_rules.len(), 1);
        assert_eq!(out.new_rules[0].tool, "mcp");
        assert_eq!(out.new_rules[0].pattern.as_deref(), Some("srv__*"));
    }

    #[tokio::test]
    async fn perm_deny_blocks_call() {
        let cfg = cfg_with(
            vec![echo_target("srv", "tool", "mcp__srv__tool")],
            PermCfg {
                mcp: Some("deny".into()),
                ..Default::default()
            },
        );
        let out = run_code(
            r#"
            try { await mcp.srv.tool({}); return "no error"; }
            catch (e) { return String(e); }
        "#,
            cfg,
        )
        .await;
        let text = out.output.unwrap();
        assert!(text.contains("denied"), "{text}");
        assert_eq!(out.calls, 1);
    }

    #[tokio::test]
    async fn syntax_and_runtime_errors() {
        let out = run_code("return (", cfg_with(vec![], PermCfg::default())).await;
        let err = out.output.unwrap_err();
        assert!(err.contains("SyntaxError"), "{err}");
        let out = run_code("return nope()", cfg_with(vec![], PermCfg::default())).await;
        assert!(out.output.unwrap_err().contains("nope"));
    }

    #[tokio::test]
    async fn infinite_loop_aborts_via_limit() {
        let out = run_code("while (true) {}", cfg_with(vec![], PermCfg::default())).await;
        let err = out.output.unwrap_err();
        assert!(err.contains("loop iteration"), "{err}");
    }

    #[tokio::test]
    async fn text_result_passthrough() {
        let mut t = echo_target("srv", "text", "mcp__srv__text");
        t.call = Arc::new(|_| Box::pin(async { Ok(Value::String("plain text answer".into())) }));
        let out = run_code(
            "return await mcp.srv.text({})",
            cfg_with(vec![t], PermCfg { mcp: Some("allow".into()), ..Default::default() }),
        )
        .await;
        assert_eq!(out.output.unwrap(), "plain text answer");
    }

    #[test]
    fn sanitization_and_signatures() {
        assert_eq!(sanitize("my-server"), "my_server");
        assert_eq!(sanitize("9lives"), "_9lives");
        assert_eq!(sanitize(""), "_");
        assert!(js_ident("my_server"));
        assert!(!js_ident("my-server"));
        assert!(!js_ident("9lives"));
        assert_eq!(signature("github", "search_repo"), "mcp.github.search_repo({...})");
        // sanitized names are always valid identifiers
        assert_eq!(signature("my_server", "my_tool"), "mcp.my_server.my_tool({...})");
        assert_eq!(signature("_9lives", "t2"), "mcp._9lives.t2({...})");
        // raw names with dashes/leading digits would need bracket notation
        assert_eq!(
            signature("my-server", "get-data"),
            "mcp[\"my-server\"][\"get-data\"]({...})"
        );
    }

    #[test]
    fn catalog_lines() {
        let specs = vec![
            ToolSpec {
                name: "mcp__github__search".into(),
                description: "Search repositories.\nLong tail".into(),
                parameters: json!({}),
            },
            ToolSpec {
                name: "mcp__my-server__get".into(),
                description: String::new(),
                parameters: json!({}),
            },
        ];
        let c = catalog_from_mcp_specs(&specs);
        assert!(c.contains("- github (1 tool)"), "{c}");
        assert!(c.contains("  - mcp.github.search({...}) // Search repositories."), "{c}");
        assert!(c.contains("- my_server (1 tool)"), "{c}");
        assert!(c.contains("  - mcp.my_server.get({...}) //"), "{c}");
    }

    #[test]
    fn mcp_text_results_parse_as_json() {
        assert_eq!(parse_mcp_text("{\"a\":1}".into()), json!({"a":1}));
        assert_eq!(parse_mcp_text("[1,2]".into()), json!([1, 2]));
        assert_eq!(parse_mcp_text("just text".into()), Value::String("just text".into()));
        assert_eq!(parse_mcp_text("{not json".into()), Value::String("{not json".into()));
    }

    #[tokio::test]
    async fn deadline_trips_before_the_second_call() {
        let mut cfg = cfg_with(
            vec![Target {
                server: "slow".into(),
                tool: "tool".into(),
                perm_key: "mcp__slow__tool".into(),
                description: String::new(),
                call: Arc::new(|_| {
                    Box::pin(async {
                        tokio::time::sleep(Duration::from_millis(150)).await;
                        Ok(json!({"ok": true}))
                    })
                }),
            }],
            PermCfg { mcp: Some("allow".into()), ..Default::default() },
        );
        cfg.timeout = Duration::from_millis(100);
        let out = run_code(
            r#"
            const a = await mcp.slow.tool({i: 1});
            return await mcp.slow.tool({i: 2});
        "#,
            cfg,
        )
        .await;
        let err = out.output.unwrap_err();
        assert!(err.contains("time budget exceeded"), "{err}");
        assert_eq!(out.calls, 2);
    }

    #[tokio::test]
    async fn run_timeout_returns_error() {
        // a child call that outlives the outer guard; the guard must return
        // "timed out" to the caller regardless (the thread itself is bounded
        // by the sleep so the test runtime can still shut down cleanly)
        let cfg = cfg_with(
            vec![Target {
                server: "slow".into(),
                tool: "tool".into(),
                perm_key: "mcp__slow__tool".into(),
                description: String::new(),
                call: Arc::new(|_| {
                    Box::pin(async {
                        tokio::time::sleep(Duration::from_secs(4)).await;
                        Ok(json!({"ok": true}))
                    })
                }),
            }],
            PermCfg { mcp: Some("allow".into()), ..Default::default() },
        );
        let out = run(
            "return await mcp.slow.tool({})".into(),
            RunCfg { timeout: Duration::from_millis(100), ..cfg },
        )
        .await;
        let err = out.output.unwrap_err();
        assert!(err.contains("timed out"), "{err}");
    }
}
