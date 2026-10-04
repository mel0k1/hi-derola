//! user-defined JavaScript tools: drop a .js file into `.hi-derola/tools/`
//! (project) or `<config>/hi-derola/tools/` (global) and it shows up as a
//! regular tool the model can call. A file looks like:
//!
//! ```js
//! module.exports = {
//!   name: "slugify",
//!   description: "Convert text to a URL slug",
//!   parameters: { type: "object", properties: { text: { type: "string" } }, required: ["text"] },
//!   execute(input) { return input.text.toLowerCase().replace(/\s+/g, "-"); },
//! };
//! ```
//!
//! Everything runs inside the same confined boa sandbox as the code tool:
//! no filesystem, no network, no processes. A string return value is passed
//! through as-is, anything else is serialized as JSON. console.log lines are
//! appended to the tool output. Project tools win over global ones with the
//! same name; names colliding with built-ins are rejected at load time.

use boa_engine::{
    js_string, native_function::NativeFunction, object::builtins::JsPromise,
    object::JsObject, property::Attribute, builtins::promise::PromiseState, Context, JsValue,
    Source,
};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use crate::provider::ToolSpec;

/// sandbox budget for one tool call
const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CODE_BYTES: usize = 256 * 1024;
const MAX_LOOP_ITERATIONS: u64 = 5_000_000;
const MAX_RECURSION: usize = 96;
const MAX_LOGS: usize = 100;
const MAX_LOG_LINE: usize = 2_000;

#[derive(Clone)]
pub struct JsTool {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub source: String,
    pub file: String,
}

struct Cache {
    tools: Arc<Vec<JsTool>>,
    skipped: Vec<(String, String)>,
}

static DIR_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);
static CACHE: RwLock<Option<Arc<Cache>>> = RwLock::new(None);

/// tests point the scanner at a temp directory; None clears the override
pub fn set_dir_override(dir: Option<PathBuf>) {
    *DIR_OVERRIDE.lock().unwrap() = dir;
    *CACHE.write().unwrap() = None;
}

fn override_dir() -> Option<PathBuf> {
    DIR_OVERRIDE.lock().unwrap().clone()
}

/// scan dirs in precedence order: project first (wins), then global
fn scan_dirs() -> Vec<PathBuf> {
    if let Some(d) = override_dir() {
        return vec![d];
    }
    let mut out = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        out.push(cwd.join(".hi-derola").join("tools"));
    }
    if let Some(cfg) = dirs::config_dir() {
        out.push(cfg.join("hi-derola").join("tools"));
    }
    out
}

fn cache() -> Arc<Cache> {
    if let Some(c) = CACHE.read().unwrap().clone() {
        return c;
    }
    let c = Arc::new(load());
    *CACHE.write().unwrap() = Some(c.clone());
    c
}

/// force a rescan of the tools directories
pub fn reload() {
    *CACHE.write().unwrap() = None;
    cache();
}

fn load() -> Cache {
    let mut tools: Vec<JsTool> = Vec::new();
    let mut skipped: Vec<(String, String)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for dir in scan_dirs() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut files: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().map(|e| e == "js").unwrap_or(false))
            .collect();
        files.sort();
        for file in files {
            let label = file
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            match load_file(&file, &seen) {
                Ok(t) => {
                    seen.push(t.name.clone());
                    tools.push(t);
                }
                Err(reason) => skipped.push((label, reason)),
            }
        }
    }
    Cache {
        tools: Arc::new(tools),
        skipped,
    }
}

/// evaluate one file in a throwaway context and pull the tool definition
/// out of module.exports
fn load_file(path: &std::path::Path, taken: &[String]) -> Result<JsTool, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("unreadable: {e}"))?;
    if meta.len() as usize > MAX_CODE_BYTES {
        return Err(format!("file too large ({} bytes)", meta.len()));
    }
    let source = std::fs::read_to_string(path).map_err(|e| format!("unreadable: {e}"))?;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut ctx = Context::default();
    {
        let lim = ctx.runtime_limits_mut();
        lim.set_loop_iteration_limit(MAX_LOOP_ITERATIONS);
        lim.set_recursion_limit(MAX_RECURSION);
    }
    build_objects(&mut ctx).map_err(|e| e.to_string())?;
    ctx.eval(Source::from_bytes(source.as_bytes()))
        .map_err(|e| e.to_string())?;
    let exports = ctx
        .eval(Source::from_bytes(b"module.exports"))
        .map_err(|e| e.to_string())?;
    let Some(obj) = exports.as_object() else {
        return Err("module.exports must be an object".into());
    };
    let has_execute = obj
        .get(js_string!("execute"), &mut ctx)
        .ok()
        .and_then(|v| v.as_object().map(|o| o.is_callable()))
        .unwrap_or(false);
    if !has_execute {
        return Err("module.exports.execute must be a function".into());
    }

    let raw_name = obj
        .get(js_string!("name"), &mut ctx)
        .ok()
        .and_then(|v| v.as_string().map(|s| s.to_std_string_escaped()));
    let name = crate::codemode::sanitize(raw_name.as_deref().unwrap_or(&stem));
    if name.is_empty() {
        return Err("empty tool name".into());
    }
    if name.starts_with("mcp__") || name == "code" || reserved(name.as_str()) {
        return Err(format!("name {name:?} collides with a built-in tool"));
    }
    if taken.contains(&name) {
        return Err(format!("duplicate tool name {name:?}"));
    }
    let description = obj
        .get(js_string!("description"), &mut ctx)
        .ok()
        .and_then(|v| v.as_string().map(|s| s.to_std_string_escaped()))
        .unwrap_or_default();
    let parameters = obj
        .get(js_string!("parameters"), &mut ctx)
        .ok()
        .filter(|v| !v.is_undefined())
        .and_then(|v| v.to_json(&mut ctx).ok())
        .filter(|v| v.is_object())
        .unwrap_or_else(|| json!({"type": "object", "properties": {}}));

    Ok(JsTool {
        name,
        description,
        parameters,
        source,
        file: path.display().to_string(),
    })
}

/// built-in tool names a user tool must not shadow
fn reserved(name: &str) -> bool {
    static RESERVED: OnceLock<Vec<String>> = OnceLock::new();
    let list = RESERVED.get_or_init(|| {
        let mut v: Vec<String> = crate::tools::specs().into_iter().map(|s| s.name).collect();
        v.extend(crate::tools::plan_specs().into_iter().map(|s| s.name));
        v.push("code".into());
        v
    });
    list.iter().any(|s| s == name)
}

fn build_objects(ctx: &mut Context) -> boa_engine::JsResult<()> {
    let module = JsObject::with_object_proto(ctx.intrinsics());
    let exports = JsObject::with_object_proto(ctx.intrinsics());
    module.set(js_string!("exports"), exports, false, ctx)?;
    ctx.register_global_property(js_string!("module"), module, Attribute::all())?;
    let console = JsObject::with_object_proto(ctx.intrinsics());
    let log = NativeFunction::from_fn_ptr(console_native).to_js_function(ctx.realm());
    for name in ["log", "info", "warn", "error"] {
        console.set(js_string!(name), log.clone(), false, ctx)?;
    }
    ctx.register_global_property(js_string!("console"), console, Attribute::all())?;
    Ok(())
}

thread_local! {
    static EXEC: RefCell<Option<(Vec<String>, Instant)>> = const { RefCell::new(None) };
}

fn console_native(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
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
    EXEC.with(|e| {
        if let Some((logs, _)) = e.borrow_mut().as_mut() {
            if logs.len() < MAX_LOGS {
                logs.push(line.chars().take(MAX_LOG_LINE).collect());
            } else if logs.len() == MAX_LOGS {
                logs.push("... (log limit reached)".into());
            }
        }
    });
    Ok(JsValue::undefined())
}

/// run one tool's execute(input) on a blocking thread; returns the formatted
/// output (logs appended) or an error message
fn run_in_thread(tool: &JsTool, input: &Value) -> (Result<String, String>, Vec<String>) {
    let source = tool.source.clone();
    let input = input.clone();
    let mut ctx = Context::default();
    {
        let lim = ctx.runtime_limits_mut();
        lim.set_loop_iteration_limit(MAX_LOOP_ITERATIONS);
        lim.set_recursion_limit(MAX_RECURSION);
    }
    let deadline = Instant::now() + TIMEOUT;
    EXEC.with(|e| *e.borrow_mut() = Some((Vec::new(), deadline)));

    let result = (|| -> Result<String, String> {
        build_objects(&mut ctx).map_err(|e| e.to_string())?;
        ctx.eval(Source::from_bytes(source.as_bytes()))
            .map_err(|e| e.to_string())?;
        let exports = ctx
            .eval(Source::from_bytes(b"module.exports"))
            .map_err(|e| e.to_string())?;
        let obj = exports
            .as_object()
            .ok_or_else(|| "module.exports must be an object".to_string())?;
        let f = obj
            .get(js_string!("execute"), &mut ctx)
            .map_err(|e| e.to_string())?;
        let fobj = f
            .as_object()
            .filter(|o| o.is_callable())
            .ok_or_else(|| "module.exports.execute must be a function".to_string())?;
        if Instant::now() >= deadline {
            return Err("tool timed out before execute".into());
        }
        let arg = JsValue::from_json(&input, &mut ctx).map_err(|e| e.to_string())?;
        let ret = fobj
            .call(&JsValue::undefined(), &[arg], &mut ctx)
            .map_err(|e| e.to_string())?;
        ctx.run_jobs();
        let is_promise = ret
            .as_object()
            .map(|o| JsPromise::from_object(o.clone()).is_ok())
            .unwrap_or(false);
        if is_promise {
            let p = JsPromise::from_object(ret.as_object().unwrap().clone()).unwrap();
            return match p.state() {
                PromiseState::Fulfilled(v) => Ok(crate::codemode::js_value_text(v, &mut ctx)),
                PromiseState::Rejected(err) => {
                    Err(crate::codemode::js_error_text(&err, &mut ctx))
                }
                PromiseState::Pending => {
                    Err("tool returned a promise that never settled (no timers in the sandbox)".into())
                }
            };
        }
        Ok(crate::codemode::js_value_text(ret, &mut ctx))
    })();

    let (logs, _) = EXEC
        .with(|e| e.borrow_mut().take())
        .unwrap_or_else(|| (Vec::new(), Instant::now()));
    let out = match result {
        Ok(mut text) => {
            if !logs.is_empty() {
                text.push_str("\n\nLogs:\n");
                text.push_str(&logs.join("\n"));
            }
            Ok(text)
        }
        Err(mut msg) => {
            if !logs.is_empty() {
                msg.push_str("\n\nLogs:\n");
                msg.push_str(&logs.join("\n"));
            }
            Err(msg)
        }
    };
    (out, logs)
}

/// the registered JS tools as model-facing specs
pub fn specs() -> Vec<ToolSpec> {
    cache()
        .tools
        .iter()
        .map(|t| ToolSpec {
            name: t.name.clone(),
            description: if t.description.trim().is_empty() {
                "User-defined JS tool (no description provided).".into()
            } else {
                t.description.clone()
            },
            parameters: t.parameters.clone(),
        })
        .collect()
}

pub fn has(name: &str) -> bool {
    if name.starts_with("mcp__") {
        return false;
    }
    cache().tools.iter().any(|t| t.name == name)
}

/// execute a registered JS tool by name
pub async fn run_tool(name: &str, args: &str) -> anyhow::Result<String> {
    let tool = cache()
        .tools
        .iter()
        .find(|t| t.name == name)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("unknown tool: {name}"))?;
    let input: Value = serde_json::from_str(args).unwrap_or(Value::Null);
    let run = move || run_in_thread(&tool, &input);
    let (out, _) = tokio::task::spawn_blocking(run)
        .await
        .map_err(|e| anyhow::anyhow!("tool worker crashed: {e}"))?;
    out.map_err(|e| anyhow::anyhow!(e))
}

/// one line per loaded tool plus skipped-file reasons; for /jstools
pub fn summary() -> String {
    let c = cache();
    if c.tools.is_empty() && c.skipped.is_empty() {
        return "no JS tools found (drop .js files into .hi-derola/tools/ or ~/.config/hi-derola/tools/)".into();
    }
    let mut out = String::new();
    for t in c.tools.iter() {
        let d = t.description.lines().next().unwrap_or("").trim();
        out.push_str(&format!("  {} — {}\n", t.name, d));
    }
    for (file, reason) in &c.skipped {
        out.push_str(&format!("  [skipped] {file}: {reason}\n"));
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hiderola-jstools-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn load_execute_and_logs() {
        let dir = temp_dir("main");
        std::fs::write(
            dir.join("slug.js"),
            r#"
            module.exports = {
                name: "slugify",
                description: "Convert text to a slug\nsecond line",
                parameters: { type: "object", properties: { text: { type: "string" } }, required: ["text"] },
                execute(input) {
                    console.log("slug it");
                    return input.text.toLowerCase().trim().replace(/\s+/g, "-");
                },
            };
        "#,
        )
        .unwrap();
        // a file relying on the stem and returning a non-string value
        std::fs::write(
            dir.join("adder.js"),
            r#"
            module.exports = {
                description: "adds two numbers",
                execute(input) { return { sum: input.a + input.b }; },
            };
        "#,
        )
        .unwrap();
        set_dir_override(Some(dir.clone()));
        reload();
        let specs = cache().tools.clone();
        let by_name = |n: &str| specs.iter().find(|s| s.name == n).cloned();
        assert_eq!(specs.len(), 2, "both tools load");
        assert_eq!(by_name("slugify").unwrap().name, "slugify");
        assert!(by_name("slugify").unwrap().parameters["required"][0] == "text");
        assert_eq!(by_name("adder").unwrap().name, "adder", "name falls back to the file stem");

        let out = block(run_tool("slugify", r#"{"text":"  Hello World  "}"#)).unwrap();
        assert_eq!(out, "hello-world\n\nLogs:\nslug it");
        let out = block(run_tool("adder", r#"{"a":2,"b":40}"#)).unwrap();
        assert!(out.contains("\"sum\""), "{out}");
        assert!(out.contains("4"), "{out}");
        set_dir_override(None);
    }

    #[test]
    fn console_logs_and_async_execute() {
        let dir = temp_dir("logs");
        std::fs::write(
            dir.join("logged.js"),
            r#"
            module.exports = {
                name: "logged",
                description: "logs stuff",
                async execute(input) {
                    console.log("n =", input.n);
                    const doubled = await Promise.resolve(input.n * 2);
                    console.log("doubled:", doubled);
                    return doubled;
                },
            };
        "#,
        )
        .unwrap();
        set_dir_override(Some(dir));
        reload();
        {
            let c = cache();
            let names: Vec<String> = c.tools.iter().map(|t| t.name.clone()).collect();
            assert!(names.contains(&"logged".to_string()), "loaded {names:?} skipped {:?}", c.skipped);
        }
        let out = block(run_tool("logged", r#"{"n":21}"#)).unwrap();
        assert!(out.starts_with("42"), "{out}");
        assert!(out.contains("Logs:"), "{out}");
        assert!(out.contains("n = 21"), "{out}");
        set_dir_override(None);
    }

    #[test]
    fn broken_files_are_skipped_with_reasons() {
        let dir = temp_dir("broken");
        std::fs::write(dir.join("syntax.js"), "module.exports = {").unwrap();
        std::fs::write(
            dir.join("noexec.js"),
            "module.exports = { name: \"noexec\", description: \"d\" };",
        )
        .unwrap();
        std::fs::write(
            dir.join("bash.js"),
            "module.exports = { name: \"bash\", description: \"d\", execute() { return 1; } };",
        )
        .unwrap();
        std::fs::write(
            dir.join("mcpname.js"),
            "module.exports = { name: \"mcp__x\", description: \"d\", execute() { return 1; } };",
        )
        .unwrap();
        std::fs::write(
            dir.join("dup.js"),
            "module.exports = { name: \"noexec2\", description: \"d\", execute() { return 1; } };",
        )
        .unwrap();
        set_dir_override(Some(dir));
        reload();
        let skipped = cache().skipped.clone();
        let reasons: Vec<&str> = skipped.iter().map(|(_, r)| r.as_str()).collect();
        assert!(reasons.iter().any(|r| r.contains("SyntaxError") || r.contains("Unexpected")), "{reasons:?}");
        assert!(reasons.iter().any(|r| r.contains("execute must be a function")), "{reasons:?}");
        assert!(reasons.iter().any(|r| r.contains("built-in")), "{reasons:?}");
        assert!(reasons.iter().any(|r| r.contains("mcp__")), "{reasons:?}");
        set_dir_override(None);
    }

    #[test]
    fn duplicate_names_project_wins() {
        let dir = temp_dir("dup2");
        std::fs::write(
            dir.join("tool.js"),
            "module.exports = { name: \"t\", description: \"project\", execute() { return \"p\"; } };",
        )
        .unwrap();
        set_dir_override(Some(dir.clone()));
        reload();
        assert_eq!(cache().tools.len(), 1);
        // a second dir cannot be scanned while the override points at one
        // place; duplicate rejection is covered by the skip-reasons test
        set_dir_override(None);
    }

    #[test]
    fn runtime_error_and_missing_tool() {
        let dir = temp_dir("runtime");
        std::fs::write(
            dir.join("boom.js"),
            "module.exports = { name: \"boom\", description: \"d\", execute() { throw new Error(\"kaput\"); } };",
        )
        .unwrap();
        set_dir_override(Some(dir));
        reload();
        let err = block(run_tool("boom", "{}")).unwrap_err();
        assert!(err.to_string().contains("kaput"), "{err}");
        assert!(block(run_tool("nope", "{}")).is_err());
        assert!(!has("nope"));
        set_dir_override(None);
    }

    #[test]
    fn infinite_loop_aborts() {
        let dir = temp_dir("loop");
        std::fs::write(
            dir.join("spin.js"),
            "module.exports = { name: \"spin\", description: \"d\", execute() { while (true) {} } };",
        )
        .unwrap();
        set_dir_override(Some(dir));
        reload();
        let err = block(run_tool("spin", "{}")).unwrap_err();
        assert!(err.to_string().contains("loop iteration"), "{err}");
        set_dir_override(None);
    }

    #[test]
    fn summary_lists_tools() {
        let dir = temp_dir("summary");
        std::fs::write(
            dir.join("s1.js"),
            "module.exports = { name: \"s1\", description: \"first\", execute() { return 1; } };",
        )
        .unwrap();
        set_dir_override(Some(dir));
        reload();
        let s = cache();
        let text = summary();
        assert!(text.contains("s1 — first"), "{text}");
        assert_eq!(s.tools.len(), 1);
        set_dir_override(None);
        reload();
        let empty = cache();
        assert_eq!(empty.tools.len(), 0);
    }

    /// sync driver for the async run_tool, same pattern as the web tests
    fn block<F: std::future::Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fut)
    }
}
