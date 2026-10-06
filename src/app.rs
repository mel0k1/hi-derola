use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    MouseEventKind,
};
use tokio::sync::mpsc;
use tokio::sync::oneshot;

use crate::agent;
use crate::chat::{Role, Session};
use crate::config::Config;
use crate::files;
use crate::mcp::{self, McpSlot};
use crate::provider::{ApiEvent, ChatRequest, ConfirmReply, Provider};
use crate::ui;

pub enum Kind {
    You,
    Bot,
    Info,
    Diff(Vec<crate::diff::Row>),
}

pub struct Entry {
    pub kind: Kind,
    pub text: String,
}

#[derive(PartialEq)]
pub enum Phase {
    Idle,
    Waiting,
    Confirm,
    Ask,
}

pub struct ConfirmCtx {
    pub name: String,
    pub args: String,
    pub rx: oneshot::Sender<ConfirmReply>,
}

pub struct AskCtx {
    pub rx: oneshot::Sender<String>,
    pub opts: Vec<String>,
}

pub struct App {
    pub cfg: Config,
    pub model: String,
    pub provider: Arc<dyn Provider>,
    pub session: Session,
    pub sid: String,
    pub title: Option<String>,
    pub entries: Vec<Entry>,
    pub input: String,
    pub scroll_up: usize,
    pub phase: Phase,
    pub confirm: Option<ConfirmCtx>,
    pub ask: Option<AskCtx>,
    pub confirm_feedback: bool,
    pub attachments: Vec<(String, String)>,
    pub streaming: Option<usize>,
    pub reasoning: Option<usize>,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub tokens_cached: u64,
    pub cost: f64,
    pub should_quit: bool,
    pub status: String,
    allow_all: Arc<AtomicBool>,
    plan: bool,
    queue: Arc<Mutex<Vec<String>>>,
    history: Vec<String>,
    hist_idx: usize,
    draft: String,
    mcp: McpSlot,
    /// live session id mirrored for mcp tools/call _meta passthrough
    mcp_session: Arc<std::sync::RwLock<String>>,
    tx: mpsc::UnboundedSender<ApiEvent>,
}

const HELP: &str = "commands:\n  /file <path>   attach file to next message\n  /model <name>  switch model, saved to config\n  /model         show current model\n  /models        list models available for the api key\n  /plan          toggle plan mode (read-only research)\n  /undo          revert file changes of the last turn\n  /redo          reapply undone changes\n  /init          create or improve AGENTS.md for this project\n  /compact       summarize and shrink the conversation context\n  /export [path] save the session as markdown\n  /sessions      list saved sessions\n  /resume [id]   switch to a saved session (latest by default)\n  /mcpauth [name] mcp OAuth status, or authorize a remote server in browser; /mcpauth <name> <code> finishes a flow with a pasted authorization code (resume after a restart)\n  /mcpres [server]  list mcp resources and uri templates\n  /mcpstatus     per-server status: connected/failed/needs auth/crashed\n  /mcpread <server> <uri> read an mcp resource into the chat\n  /mcpprompt [server] <name> [k=v] use an mcp prompt (no args lists prompts)\n  /mcpsub <server> <uri> subscribe to mcp resource updates (land in chat)\n  /mcpunsub <server> <uri> stop the subscription\n  /mcplog [server]  recent mcp log messages; /mcplog set <server|all> <level> sets the minimum level\n  /mcpadd <name> <url|command...> add a server at runtime (saved to config) and connect it\n  /mcpconnect <name> (re)connect a configured server\n  /mcpdisconnect <name> drop the live connection (config untouched)\n  /mcplogout <name> drop the stored oauth tokens; a fresh /mcpauth flow starts on next use\n  /jstools [reload] list user JS tools (.hi-derola/tools/), optional rescan\n  /sandbox       local VM sandboxes: bare = list; /sandbox start|stop|attach|detach <id|name>; /sandbox new <name> [debian|debian-std|ubuntu|ubuntu-std|custom=<path>] [ram=2048] [cpus=2] [disk=20] [login=x] [root=on|off] — attach routes bash into the VM over ssh\n  /clear         start new session\n  /quit          exit\n  custom: .hi-derola/commands/<name>.md or ~/.config/hi-derola/commands/<name>.md ($ARGUMENTS, $1..$9)\nkeys:\n  enter send  esc cancel/quit  up/down history  pgup/pgdn scroll  ctrl+c quit\ntools:\n  read/write/edit/apply_patch/list/glob/grep/bash (background: true)/webfetch/codesearch/mcp_resource + question, plan_write/plan_exit (plan mode), subagent (background, session_id), task_status, task_kill, todowrite/todoread, skill, lsp (hover/definition/references/symbols), code (JS sandbox over MCP tools), custom JS tools from .hi-derola/tools/, mcp servers\nconfirm:\n  y run  n skip  a allow all  w always allow (saved to config)  f reject with feedback\nqueue:\n  messages sent while busy are queued, they steer the current run";

pub fn help_text() -> &'static str {
    HELP
}

fn fmt_tokens(n: u64) -> String {
    if n < 1000 {
        n.to_string()
    } else {
        format!("{:.1}k", n as f64 / 1000.0)
    }
}

fn fmt_cost(c: f64) -> String {
    if c >= 1.0 {
        format!("${c:.2}")
    } else {
        format!("${c:.4}")
    }
}

fn fmt_age(secs: u64) -> String {
    if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

/// human bytes for download progress in /sandbox listings (MiB until 1 GiB)
fn fmt_mib(bytes: u64) -> String {
    let mib = bytes as f64 / (1024.0 * 1024.0);
    if mib >= 1024.0 {
        format!("{:.1} GiB", mib / 1024.0)
    } else {
        format!("{mib:.0} MiB")
    }
}

fn key_seq(c: char) -> usize {
    c.to_digit(10)
        .map(|d| d.saturating_sub(1) as usize)
        .unwrap_or(usize::MAX)
}

impl App {
    pub fn new(
        cfg: Config,
        provider: Arc<dyn Provider>,
        tx: mpsc::UnboundedSender<ApiEvent>,
    ) -> Self {
        let model = cfg.provider.model.clone();
        let system = crate::base_prompt("in the user's terminal", &model);
        let status = format!("{} · {}", provider.name(), model);
        let sid = crate::sessions::new_id();
        let mut app = Self {
            cfg,
            model,
            provider,
            session: Session::new(system),
            sid: sid.clone(),
            mcp_session: Arc::new(std::sync::RwLock::new(sid)),
            title: None,
            entries: Vec::new(),
            input: String::new(),
            scroll_up: 0,
            phase: Phase::Idle,
            confirm: None,
            ask: None,
            confirm_feedback: false,
            attachments: Vec::new(),
            streaming: None,
            reasoning: None,
            tokens_in: 0,
            tokens_out: 0,
            tokens_cached: 0,
            cost: 0.0,
            should_quit: false,
            status,
            allow_all: Arc::new(AtomicBool::new(false)),
            plan: false,
            queue: Arc::new(Mutex::new(Vec::new())),
            history: Vec::new(),
            hist_idx: 0,
            draft: String::new(),
            mcp: Arc::new(Mutex::new(None)),
            tx,
        };
        // a shell route left over from an earlier session of this process
        // re-applies its sandbox addendum
        if crate::sandbox::shell_route().is_some() {
            app.refresh_system_prompt();
        }
        app
    }

    /// rebuild the system prompt: base (venue + model + AGENTS.md) plus the
    /// sandbox addendum while the bash tool is routed into a VM
    fn refresh_system_prompt(&mut self) {
        self.session.system = crate::base_prompt("in the user's terminal", &self.model);
        if let Some(add) = crate::sandbox::shell_route_addendum() {
            self.session.system.push_str("\n\n");
            self.session.system.push_str(&add);
        }
    }

    /// /sandbox — the sandbox tab for the terminal: list, lifecycle, quick
    /// create and the bash route attach/detach (GUI parity lives in the
    /// sandbox tab of the window)
    fn sandbox_command(&mut self, arg: &str) {
        const USAGE: &str = "usage: /sandbox [list] · /sandbox start|stop|attach|detach <id|name> · /sandbox fetch <id|name> <vm-path> [host-path] · /sandbox new <name> [debian|debian-std|ubuntu|ubuntu-std|custom=<path>] [ram=2048] [cpus=2] [disk=20] [login=x] [root=on|off]";
        let m = crate::sandbox::SandboxManager::global();
        let mut parts = arg.split_whitespace();
        let sub = parts.next().unwrap_or("");
        match sub {
            "" | "list" => {
                let list = m.list();
                if list.is_empty() {
                    self.info(format!(
                        "no sandboxes yet — the GUI sandbox tab walks you through it, or:\n{USAGE}"
                    ));
                    return;
                }
                let route = crate::sandbox::shell_route();
                let mut out = String::from("sandboxes:");
                for s in list {
                    out.push_str(&format!(
                        "\n  {} [{}] · {} · {:?}",
                        s.spec.name,
                        s.spec.id,
                        s.spec.kind.label(),
                        s.state
                    ));
                    if let Some(d) = &s.download {
                        if !d.done && d.total > 0 {
                            out.push_str(&format!(
                                " ({} / {})",
                                fmt_mib(d.downloaded),
                                fmt_mib(d.total)
                            ));
                        }
                    }
                    if s.state == crate::sandbox::VmState::Running {
                        if let Some(ssh) = &s.ssh {
                            out.push_str(&format!(
                                " · ssh {:?}{} · agent {:?}",
                                ssh.state,
                                if ssh.state == crate::sandbox::SshState::Ready {
                                    format!(" ({}s)", ssh.elapsed_secs)
                                } else {
                                    String::new()
                                },
                                ssh.agent.state
                            ));
                        }
                    }
                    if let Some(e) = &s.error {
                        let one = e
                            .lines()
                            .next()
                            .unwrap_or("")
                            .chars()
                            .take(90)
                            .collect::<String>();
                        out.push_str(&format!(" · {one}"));
                    }
                    if route.as_deref() == Some(s.spec.id.as_str()) {
                        out.push_str("  ← bash here");
                    }
                }
                out.push_str(&format!("\n{USAGE}"));
                self.info(out);
            }
            "attach" | "detach" => {
                if sub == "detach" {
                    if crate::sandbox::shell_route().is_none() {
                        self.info("no sandbox attached — bash already runs on the host");
                        return;
                    }
                    crate::sandbox::set_shell_route(None);
                    self.refresh_system_prompt();
                    self.info("detached — bash is back on the host");
                    return;
                }
                if !matches!(self.phase, Phase::Idle) {
                    self.info("wait for the current run to finish, then attach");
                    return;
                }
                let Some(key) = parts.next() else {
                    self.info(format!("/sandbox attach <id|name> — {USAGE}"));
                    return;
                };
                let key = key.to_lowercase();
                let hit = m.list().into_iter().find(|s| {
                    s.spec.id == key
                        || s.spec.name == key
                        || s.spec.id.starts_with(&key)
                        || s.spec.name.to_lowercase().contains(&key)
                });
                let Some(s) = hit else {
                    self.info(format!(
                        "no sandbox matches \"{key}\" — /sandbox lists them"
                    ));
                    return;
                };
                if s.state != crate::sandbox::VmState::Running {
                    self.info(format!(
                        "sandbox \"{}\" is {:?} — /sandbox start {} first",
                        s.spec.name, s.state, s.spec.id
                    ));
                    return;
                }
                let ready = s
                    .ssh
                    .as_ref()
                    .map(|x| x.state == crate::sandbox::SshState::Ready)
                    .unwrap_or(false);
                if !ready {
                    self.info(
                        "ssh is not ready yet — this works only on cloud images (seed kinds) after the ready badge (/sandbox to check)",
                    );
                    return;
                }
                crate::sandbox::set_shell_route(Some(s.spec.id.clone()));
                self.refresh_system_prompt();
                self.info(format!(
                    "bash now runs inside \"{}\" ({}, {}) — your host files stay out of reach; /sandbox detach returns to the host shell",
                    s.spec.name,
                    s.spec.kind.label(),
                    s.spec.id
                ));
            }
            "fetch" => {
                let mut it = parts;
                let Some(key) = it.next() else {
                    self.info(format!(
                        "/sandbox fetch <id|name> <vm-path> [host-path] — {USAGE}"
                    ));
                    return;
                };
                let Some(vm_path) = it.next() else {
                    self.info("usage: /sandbox fetch <id|name> <vm-path> [host-path]");
                    return;
                };
                let host_path = it.next().map(str::to_string).unwrap_or_else(|| {
                    let base = vm_path.rsplit('/').next().unwrap_or(vm_path);
                    std::env::current_dir()
                        .unwrap_or_default()
                        .join(base)
                        .display()
                        .to_string()
                });
                let key = key.to_lowercase();
                let Some(s) = m.list().into_iter().find(|s| {
                    s.spec.id == key
                        || s.spec.name == key
                        || s.spec.id.starts_with(&key)
                        || s.spec.name.to_lowercase().contains(&key)
                }) else {
                    self.info(format!(
                        "no sandbox matches \"{key}\" — /sandbox lists them"
                    ));
                    return;
                };
                match crate::sandbox::fetch_from_vm(&s.spec.id, vm_path, &host_path) {
                    Ok(msg) => self.info(msg),
                    Err(e) => self.info(format!("error: {e:#}")),
                }
            }
            "start" | "stop" => {
                let Some(key) = parts.next() else {
                    self.info(format!("/sandbox {sub} <id|name> — {USAGE}"));
                    return;
                };
                let key = key.to_lowercase();
                let hit = m.list().into_iter().find(|s| {
                    s.spec.id == key
                        || s.spec.name == key
                        || s.spec.id.starts_with(&key)
                        || s.spec.name.to_lowercase().contains(&key)
                });
                let Some(s) = hit else {
                    self.info(format!(
                        "no sandbox matches \"{key}\" — /sandbox lists them"
                    ));
                    return;
                };
                let res = if sub == "start" {
                    m.start(&s.spec.id)
                } else {
                    m.stop(&s.spec.id)
                };
                match res {
                    Ok(st) => self.info(format!(
                        "sandbox \"{}\" is now {:?}{}",
                        st.spec.name,
                        st.state,
                        if st.state == crate::sandbox::VmState::Running {
                            " — ssh polling shows on /sandbox; attach once it is ready"
                        } else {
                            ""
                        }
                    )),
                    Err(e) => self.info(format!("error: {e:#}")),
                }
            }
            "new" => {
                let rest: Vec<&str> = parts.collect();
                if rest.is_empty() {
                    self.info(format!(
                        "/sandbox new <name> [kind] [key=value...] — {USAGE}"
                    ));
                    return;
                }
                let name = rest[0].to_string();
                let mut kind = crate::sandbox::ImageKind::DebianTrixie;
                let mut iso_path = None;
                let mut login = crate::seed::DEFAULT_LOGIN.to_string();
                let mut disk_gib = None;
                let mut ram_mib = None;
                let mut cpus = None;
                // root defaults on for the TUI: the in-VM agent needs
                // passwordless sudo to install itself
                let mut root = true;
                for a in &rest[1..] {
                    if let Some(v) = a.strip_prefix("ram=") {
                        ram_mib = v.parse().ok();
                    } else if let Some(v) = a.strip_prefix("cpus=") {
                        cpus = v.parse().ok();
                    } else if let Some(v) = a.strip_prefix("disk=") {
                        disk_gib = v.parse().ok();
                    } else if let Some(v) = a.strip_prefix("login=") {
                        login = v.to_string();
                    } else if *a == "root=on" {
                        root = true;
                    } else if *a == "root=off" {
                        root = false;
                    } else if let Some(p) = a.strip_prefix("custom=") {
                        kind = crate::sandbox::ImageKind::Custom;
                        iso_path = Some(p.to_string());
                    } else {
                        match *a {
                            "debian" => kind = crate::sandbox::ImageKind::DebianTrixie,
                            "debian-std" => kind = crate::sandbox::ImageKind::DebianTrixieStd,
                            "ubuntu" => kind = crate::sandbox::ImageKind::Ubuntu2404,
                            "ubuntu-std" => kind = crate::sandbox::ImageKind::Ubuntu2404Std,
                            other => {
                                self.info(format!(
                                    "unknown kind or option \"{other}\" — kinds: debian, debian-std, ubuntu, ubuntu-std, custom=<path>; options: ram= cpus= disk= login= root=on|off"
                                ));
                                return;
                            }
                        }
                    }
                }
                let used: std::collections::HashSet<u16> =
                    m.list().iter().map(|s| s.spec.ssh_port).collect();
                let mut port = crate::sandbox::DEFAULT_PORT;
                while used.contains(&port) {
                    port += 1;
                }
                let req = crate::sandbox::NewSandbox {
                    name,
                    kind,
                    login: Some(login),
                    iso_path,
                    disk_gib,
                    ram_mib,
                    cpus,
                    root,
                    ssh_port: Some(port),
                };
                match m.create(&req) {
                    Ok(st) => {
                        let mut msg = format!(
                            "created \"{}\" [{}] id {} — ssh port {}",
                            st.spec.name,
                            st.spec.kind.label(),
                            st.spec.id,
                            st.spec.ssh_port
                        );
                        if st.spec.kind.needs_download() {
                            msg.push_str(
                                "\nthe image is downloading in the background (/sandbox shows progress); /sandbox start <id|name> boots it",
                            );
                        } else {
                            msg.push_str(" — /sandbox start <id|name> boots it");
                        }
                        self.info(msg);
                    }
                    Err(e) => self.info(format!("error: {e:#}")),
                }
            }
            _ => self.info(USAGE),
        }
    }

    /// client hooks for mcp: workspace root (cwd) + sampling via our provider
    fn mcp_hooks(&self) -> mcp::McpHooks {
        mcp::McpHooks::workspace(std::env::current_dir().ok())
            .with_notes(self.tx.clone())
            .with_session(self.mcp_session.clone())
            .with_eliciter(mcp::default_eliciter(self.tx.clone()))
            .with_mcp_timeout(self.cfg.agent.mcp_timeout)
            .with_sampler(mcp::default_sampler(
                self.provider.clone(),
                self.model.clone(),
                self.cfg.provider.temperature,
                self.tx.clone(),
            ))
    }

    pub async fn connect_mcp(&mut self) {
        let cfgs = self.cfg.mcp.clone();
        let hooks = self.mcp_hooks();
        let (client, logs) = mcp::connect_all(&cfgs, &hooks).await;
        for l in logs {
            self.info(l);
        }
        *self.mcp.lock().unwrap() = client;
    }

    fn info(&mut self, text: impl Into<String>) {
        self.entries.push(Entry {
            kind: Kind::Info,
            text: text.into(),
        });
        self.scroll_up = 0;
    }

    fn flush_stream(&mut self) {
        if let Some(i) = self.streaming.take() {
            if self.entries[i].text.trim().is_empty() {
                self.entries.remove(i);
                if let Some(r) = self.reasoning {
                    if r > i {
                        self.reasoning = Some(r - 1);
                    }
                }
            }
        }
    }

    pub fn on_api(&mut self, ev: ApiEvent) {
        match ev {
            ApiEvent::Chunk(s) => {
                match self.streaming {
                    Some(i) => self.entries[i].text.push_str(&s),
                    None => {
                        self.entries.push(Entry {
                            kind: Kind::Bot,
                            text: s,
                        });
                        self.streaming = Some(self.entries.len() - 1);
                    }
                }
                self.scroll_up = 0;
            }
            ApiEvent::Reasoning(s) => {
                match self.reasoning {
                    Some(i) => self.entries[i].text.push_str(&s),
                    None => {
                        self.entries.push(Entry {
                            kind: Kind::Info,
                            text: format!("reasoning: {s}"),
                        });
                        self.reasoning = Some(self.entries.len() - 1);
                    }
                }
                self.scroll_up = 0;
            }
            ApiEvent::Note(s) => {
                self.flush_stream();
                self.info(s);
            }
            ApiEvent::Tool {
                name, detail, diff, ..
            } => {
                self.flush_stream();
                self.reasoning = None;
                self.info(format!("tool {name} {detail}"));
                if !diff.is_empty() {
                    self.entries.push(Entry {
                        kind: Kind::Diff(diff),
                        text: String::new(),
                    });
                    self.scroll_up = 0;
                }
            }
            ApiEvent::Confirm { name, args, rx } => {
                self.flush_stream();
                self.confirm = Some(ConfirmCtx { name, args, rx });
                self.phase = Phase::Confirm;
                self.scroll_up = 0;
            }
            ApiEvent::Ask { args, rx, .. } => {
                self.flush_stream();
                self.reasoning = None;
                let v: serde_json::Value =
                    serde_json::from_str(&args).unwrap_or(serde_json::Value::Null);
                let mut opts = Vec::new();
                if let Some(qs) = v["questions"].as_array() {
                    for q in qs {
                        let text = q["question"].as_str().unwrap_or("");
                        let header = q["header"].as_str().unwrap_or("");
                        if header.is_empty() {
                            self.info(format!("question: {text}"));
                        } else {
                            self.info(format!("question [{header}]: {text}"));
                        }
                        if let Some(os) = q["options"].as_array() {
                            for (i, o) in os.iter().enumerate() {
                                let label = o["label"].as_str().unwrap_or("");
                                let desc = o["description"].as_str().unwrap_or("");
                                if desc.is_empty() {
                                    self.info(format!("  {}: {label}", i + 1));
                                } else {
                                    self.info(format!("  {}: {label} - {desc}", i + 1));
                                }
                                opts.push(label.to_string());
                            }
                        }
                    }
                }
                self.info("type an answer, press a number for an option, esc skips");
                self.ask = Some(AskCtx { rx, opts });
                self.phase = Phase::Ask;
                self.scroll_up = 0;
            }
            ApiEvent::Plan(on) => {
                self.flush_stream();
                self.plan = on;
                self.info(if on {
                    "plan mode on"
                } else {
                    "plan mode off (approved via plan_exit)"
                });
            }
            ApiEvent::Todo(s) => {
                self.flush_stream();
                self.info(format!("todo list updated:\n{s}"));
            }
            ApiEvent::BgOut { .. } => {}
            ApiEvent::Usage {
                input,
                output,
                cached,
            } => {
                self.tokens_in += input;
                self.tokens_out += output;
                self.tokens_cached += cached;
                let disc = if self.cfg.provider.kind == "anthropic" {
                    0.1
                } else {
                    0.5
                };
                self.cost += crate::models::cost_cached(&self.model, input, output, cached, disc);
            }
            ApiEvent::Done { text, messages } => {
                if let Some(i) = self.streaming {
                    self.entries[i].text = text.clone();
                } else if !text.is_empty() {
                    self.entries.push(Entry {
                        kind: Kind::Bot,
                        text: text.clone(),
                    });
                }
                if !messages.is_empty() {
                    self.session.messages = messages;
                }
                self.phase = Phase::Idle;
                self.streaming = None;
                self.reasoning = None;
                crate::snapshot::end_turn();
                self.save_session();
            }
            ApiEvent::Failed(e) => {
                self.info(format!("error: {e}"));
                self.phase = Phase::Idle;
                self.streaming = None;
                self.reasoning = None;
                crate::snapshot::end_turn();
            }
            ApiEvent::Wake => {}
            ApiEvent::Submit(text) => {
                // normally intercepted by the main loop; from inside a run it steers
                self.queue.lock().unwrap().push(text);
            }
        }
        self.status = self.status_line();
    }

    fn save_session(&mut self) {
        if self.session.messages.is_empty() {
            return;
        }
        let st = crate::sessions::StoredSession {
            id: self.sid.clone(),
            title: self.title.clone().unwrap_or_else(|| "new chat".into()),
            created: 0,
            updated: 0,
            system: self.session.system.clone(),
            messages: self.session.messages.clone(),
            tokens_in: self.tokens_in,
            tokens_out: self.tokens_out,
            cost: self.cost,
            todos: crate::todo::get(),
            parent: None,
            changes: Vec::new(),
            queue: self.queue.lock().unwrap().clone(),
        };
        if let Err(e) = crate::sessions::save(&st) {
            self.info(format!("session not saved: {e:#}"));
        }
    }

    fn load_session(&mut self, st: crate::sessions::StoredSession) {
        self.entries.clear();
        self.streaming = None;
        self.reasoning = None;
        self.attachments.clear();
        self.sid = st.id.clone();
        if let Ok(mut g) = self.mcp_session.write() {
            *g = self.sid.clone();
        }
        self.title = Some(st.title.clone());
        self.session.system = st.system;
        self.session.messages = st.messages.clone();
        self.tokens_in = st.tokens_in;
        self.tokens_out = st.tokens_out;
        self.tokens_cached = 0;
        self.cost = st.cost;
        crate::todo::set_list(st.todos);
        *self.queue.lock().unwrap() = st.queue.clone();
        if !st.queue.is_empty() {
            self.info(format!(
                "{} queued message(s) restored, they will steer the next run",
                st.queue.len()
            ));
        }
        for m in &st.messages {
            match m.role {
                Role::User if !m.content.trim().is_empty() => self.entries.push(Entry {
                    kind: Kind::You,
                    text: m.content.clone(),
                }),
                Role::Assistant if !m.content.trim().is_empty() => self.entries.push(Entry {
                    kind: Kind::Bot,
                    text: m.content.clone(),
                }),
                _ => {}
            }
        }
        self.info(format!(
            "resumed {} ({} messages)",
            st.title,
            st.messages.len()
        ));
        self.scroll_up = 0;
    }

    pub fn cancelled(&mut self) {
        self.phase = Phase::Idle;
        self.confirm = None;
        self.ask = None;
        self.confirm_feedback = false;
        self.streaming = None;
        self.reasoning = None;
        self.info("cancelled");
        self.status = self.status_line();
        crate::snapshot::end_turn();
    }

    fn status_line(&self) -> String {
        match self.phase {
            Phase::Waiting => "thinking...".into(),
            Phase::Confirm => match &self.confirm {
                Some(c) if self.confirm_feedback => {
                    "reject feedback: type, enter sends, esc denies".into()
                }
                Some(c) => format!("run {}?  y/n/a/f", c.name),
                None => "confirm...".into(),
            },
            Phase::Ask => "answer the question".into(),
            Phase::Idle => {
                let mut s = format!("{} · {}", self.provider.name(), self.model);
                if self.tokens_in > 0 || self.tokens_out > 0 {
                    s.push_str(&format!(
                        " · {} in · {} out",
                        fmt_tokens(self.tokens_in),
                        fmt_tokens(self.tokens_out)
                    ));
                    if self.tokens_cached > 0 {
                        s.push_str(&format!(" ({} cached)", fmt_tokens(self.tokens_cached)));
                    }
                }
                if self.cost > 0.0 {
                    s.push_str(&format!(" · {}", fmt_cost(self.cost)));
                }
                s
            }
        }
    }

    fn confirm_key(&mut self, key: KeyEvent) {
        let code = key.code;
        let Some(c) = self.confirm.take() else {
            self.phase = Phase::Idle;
            return;
        };
        if self.confirm_feedback {
            match code {
                KeyCode::Enter => {
                    let feedback = self.input.trim().to_string();
                    self.input.clear();
                    self.confirm_feedback = false;
                    let _ = c.rx.send(ConfirmReply {
                        approved: false,
                        feedback,
                        always: false,
                    });
                    self.info("rejected with feedback");
                }
                KeyCode::Esc => {
                    self.input.clear();
                    self.confirm_feedback = false;
                    let _ = c.rx.send(ConfirmReply::default());
                    self.info("denied");
                }
                KeyCode::Backspace => {
                    self.input.pop();
                    self.confirm = Some(c);
                    return;
                }
                KeyCode::Char(ch)
                    if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
                {
                    self.input.push(ch);
                    self.confirm = Some(c);
                    return;
                }
                _ => {
                    self.confirm = Some(c);
                    return;
                }
            }
            self.phase = Phase::Waiting;
            self.scroll_up = 0;
            self.status = self.status_line();
            return;
        }
        match code {
            KeyCode::Char('y') => {
                let _ = c.rx.send(ConfirmReply {
                    approved: true,
                    feedback: String::new(),
                    always: false,
                });
            }
            KeyCode::Char('a') => {
                self.allow_all.store(true, Ordering::Relaxed);
                let _ = c.rx.send(ConfirmReply {
                    approved: true,
                    feedback: String::new(),
                    always: false,
                });
            }
            KeyCode::Char('w') => {
                if let Some(rule) = crate::perm::derive_rule(&c.name, &c.args) {
                    if let Err(e) = crate::config::Config::append_perm_rule(rule.clone()) {
                        self.info(format!("rule not saved: {e:#}"));
                    }
                    self.cfg.permissions.rules.push(rule);
                    self.info("always allowed: rule saved to config");
                }
                let _ = c.rx.send(ConfirmReply {
                    approved: true,
                    feedback: String::new(),
                    always: true,
                });
            }
            KeyCode::Char('f') => {
                self.input.clear();
                self.confirm_feedback = true;
                self.confirm = Some(c);
                self.status = self.status_line();
                return;
            }
            KeyCode::Char('n') | KeyCode::Char('s') => {
                let _ = c.rx.send(ConfirmReply::default());
                self.info("denied");
            }
            _ => {
                self.confirm = Some(c);
                return;
            }
        }
        self.phase = Phase::Waiting;
        self.scroll_up = 0;
        self.status = self.status_line();
    }

    fn ask_key(&mut self, key: KeyEvent) {
        let code = key.code;
        let Some(a) = self.ask.take() else {
            self.phase = Phase::Idle;
            return;
        };
        let restore = |app: &mut Self, a: AskCtx| {
            app.ask = Some(a);
        };
        match code {
            KeyCode::Enter => {
                let answer = self.input.trim().to_string();
                self.input.clear();
                let _ = a.rx.send(answer);
            }
            KeyCode::Esc => {
                self.input.clear();
                let _ = a.rx.send(String::new());
                self.info("skipped");
            }
            KeyCode::Backspace => {
                self.input.pop();
                restore(self, a);
                return;
            }
            KeyCode::Char(c @ '1'..='9')
                if self.input.is_empty()
                    && key.modifiers.is_empty()
                    && key_seq(c) < a.opts.len() =>
            {
                let label = a.opts[key_seq(c)].clone();
                self.info(format!("answered: {label}"));
                let _ = a.rx.send(label);
            }
            KeyCode::Char(c)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                self.input.push(c);
                restore(self, a);
                return;
            }
            _ => {
                restore(self, a);
                return;
            }
        }
        self.phase = Phase::Waiting;
        self.scroll_up = 0;
        self.status = self.status_line();
    }

    pub fn on_key(&mut self, key: KeyEvent, inflight: &mut Option<tokio::task::JoinHandle<()>>) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        match self.phase {
            Phase::Confirm => {
                self.confirm_key(key);
                return;
            }
            Phase::Ask => {
                self.ask_key(key);
                return;
            }
            Phase::Waiting => {
                match key.code {
                    KeyCode::Enter => self.submit(inflight),
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.input.clear();
                    }
                    KeyCode::Backspace => {
                        self.input.pop();
                    }
                    KeyCode::Char(c)
                        if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
                    {
                        self.input.push(c);
                    }
                    _ => {}
                }
                return;
            }
            Phase::Idle => {}
        }
        match key.code {
            KeyCode::Esc => self.should_quit = true,
            KeyCode::Enter => self.submit(inflight),
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.clear();
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                while self.input.ends_with(char::is_whitespace) {
                    self.input.pop();
                }
                while !self.input.is_empty() && !self.input.ends_with(char::is_whitespace) {
                    self.input.pop();
                }
            }
            KeyCode::PageUp => self.scroll_up = self.scroll_up.saturating_add(10),
            KeyCode::PageDown => self.scroll_up = self.scroll_up.saturating_sub(10),
            KeyCode::Up => self.hist_prev(),
            KeyCode::Down => self.hist_next(),
            KeyCode::Char(c)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                self.input.push(c);
            }
            _ => {}
        }
    }

    fn hist_prev(&mut self) {
        if self.hist_idx == 0 || self.history.is_empty() {
            return;
        }
        if self.hist_idx == self.history.len() {
            self.draft = self.input.clone();
        }
        self.hist_idx -= 1;
        self.input = self.history[self.hist_idx].clone();
    }

    fn hist_next(&mut self) {
        if self.hist_idx >= self.history.len() {
            return;
        }
        self.hist_idx += 1;
        self.input = if self.hist_idx == self.history.len() {
            self.draft.clone()
        } else {
            self.history[self.hist_idx].clone()
        };
    }

    fn submit(&mut self, inflight: &mut Option<tokio::task::JoinHandle<()>>) {
        let text = self.input.trim().to_string();
        if !text.is_empty() && self.history.last().map(|h| h != &text).unwrap_or(true) {
            self.history.push(text.clone());
        }
        self.hist_idx = self.history.len();
        self.draft.clear();
        if text.starts_with('/') {
            self.input.clear();
            self.command(&text, inflight);
            self.status = self.status_line();
            return;
        }
        // manual subagent invocation: "@explore find the parser"
        if let Some((agent_name, rest)) = crate::agents::split_mention(&text) {
            self.input.clear();
            if !matches!(self.phase, Phase::Idle) {
                self.info("wait for the current run to finish");
                return;
            }
            let (blocks, _ok, _miss) = files::mentions(&rest);
            let prompt = format!("{blocks}{rest}");
            let (sub_req, sid, read_only) = match agent::resolve_sub_req(
                Some(&agent_name),
                &prompt,
                None,
                Some(&self.sid),
                &self.model,
                self.cfg.provider.max_tokens,
                self.cfg.provider.temperature,
                self.cfg.provider.top_p,
            ) {
                Ok(r) => r,
                Err(e) => {
                    self.info(e);
                    return;
                }
            };
            let id = agent::spawn_standalone_subagent(
                self.provider.clone(),
                sub_req,
                sid,
                String::new(),
                read_only,
                agent::AgentCfg {
                    context_limit: self.cfg.agent.context_limit,
                    max_rounds: self.cfg.agent.max_rounds,
                    output_budget: self.cfg.agent.output_budget,
                    perm: self.cfg.permissions.clone(),
                    nested: false,
                    plan: false,
                    read_only: false,
                    parent_sid: Some(self.sid.clone()),
                    depth: 0,
                    max_depth: self.cfg.agent.subagent_depth,
                    compaction: self.cfg.agent.compaction.clone(),
                },
                self.allow_all.clone(),
                self.mcp.clone(),
                self.queue.clone(),
                self.tx.clone(),
            );
            self.phase = Phase::Waiting;
            self.status = self.status_line();
            self.info(format!(
                "@{agent_name} running as {id} (progress: task_status, stop: task_kill)"
            ));
            return;
        }
        if text.is_empty() && self.attachments.is_empty() {
            return;
        }
        let mut composed = String::new();
        let mut images: Vec<crate::chat::Image> = Vec::new();
        for (path, content) in &self.attachments {
            if let Some((mime, data)) = files::split_data_url(content) {
                images.push(crate::chat::Image { mime, data });
                composed.push_str(&format!("[image: {path}]\n\n"));
            } else {
                composed.push_str(&format!("[file: {path}]\n{content}\n\n"));
            }
        }
        let (mention_blocks, mention_ok, mention_miss) = files::mentions(&text);
        composed.push_str(&mention_blocks);
        composed.push_str(&text);
        self.attachments.clear();
        self.input.clear();
        self.entries.push(Entry {
            kind: Kind::You,
            text,
        });
        if !mention_ok.is_empty() || !mention_miss.is_empty() {
            let mut line = String::new();
            if !mention_ok.is_empty() {
                line.push_str(&format!("@mentions attached: {}", mention_ok.join(", ")));
            }
            if !mention_miss.is_empty() {
                if !line.is_empty() {
                    line.push_str(" · ");
                }
                line.push_str(&format!("not found: {}", mention_miss.join(", ")));
            }
            self.info(line);
        }
        if !matches!(self.phase, Phase::Idle) {
            self.queue.lock().unwrap().push(composed);
            self.info("queued: will steer the current run");
            self.save_session();
            return;
        }
        self.session
            .messages
            .push(crate::chat::Message::new(Role::User, composed).with_images(images));
        self.scroll_up = 0;
        self.start_run(inflight);
    }

    /// submit text as a user message and start a run (mcp prompts, Submit events)
    pub fn submit_text(
        &mut self,
        text: String,
        inflight: &mut Option<tokio::task::JoinHandle<()>>,
    ) {
        if text.trim().is_empty() {
            return;
        }
        if !matches!(self.phase, Phase::Idle) {
            self.queue.lock().unwrap().push(text);
            self.info("queued: will steer the current run");
            return;
        }
        self.entries.push(Entry {
            kind: Kind::You,
            text: text.clone(),
        });
        self.session.push(Role::User, text);
        self.scroll_up = 0;
        self.start_run(inflight);
    }

    fn start_run(&mut self, inflight: &mut Option<tokio::task::JoinHandle<()>>) {
        self.phase = Phase::Waiting;
        self.status = self.status_line();
        self.scroll_up = 0;
        crate::snapshot::begin_turn();
        if self.title.is_none() {
            if let Some(m) = self
                .session
                .messages
                .iter()
                .rev()
                .find(|m| m.role == Role::User)
            {
                self.title = Some(crate::sessions::title_from(&m.content));
            }
        }

        let provider = self.provider.clone();
        let tx = self.tx.clone();
        let allow_all = self.allow_all.clone();
        let mcp = self.mcp.clone();
        let queue = self.queue.clone();
        let agent_cfg = crate::agent::AgentCfg {
            context_limit: self.cfg.agent.context_limit,
            max_rounds: self.cfg.agent.max_rounds,
            output_budget: self.cfg.agent.output_budget,
            perm: self.cfg.permissions.clone(),
            nested: false,
            plan: self.plan,
            read_only: false,
            parent_sid: Some(self.sid.clone()),
            depth: 0,
            max_depth: self.cfg.agent.subagent_depth,
            compaction: self.cfg.agent.compaction.clone(),
        };
        let mut req = ChatRequest {
            system: self.session.system.clone(),
            messages: self.session.messages.clone(),
            model: self.model.clone(),
            max_tokens: self.cfg.provider.max_tokens,
            temperature: self.cfg.provider.temperature,
            top_p: self.cfg.provider.top_p,
            stream: self.cfg.provider.stream,
            tools: Vec::new(),
        };
        if self.plan {
            req.system.push_str("\n\n");
            req.system.push_str(
                "PLAN MODE is active: research the codebase (read_file, glob, grep, read-only bash commands) and design an approach. File modifications are disabled. Save the full plan to .hi-derola/plan.md with plan_write (rewrite the whole file on every update), then call plan_exit to ask the user to approve leaving plan mode.",
            );
        }
        let handle = tokio::spawn(async move {
            if let Err(e) =
                agent::run(provider, req, tx.clone(), allow_all, mcp, queue, agent_cfg).await
            {
                let _ = tx.send(ApiEvent::Failed(format!("{e:#}")));
            }
        });
        *inflight = Some(handle);
    }

    pub fn resume_queued(&mut self, inflight: &mut Option<tokio::task::JoinHandle<()>>) {
        if !matches!(self.phase, Phase::Idle) || self.confirm.is_some() || self.ask.is_some() {
            return;
        }
        let next = {
            let mut q = self.queue.lock().unwrap();
            if q.is_empty() {
                None
            } else {
                Some(q.remove(0))
            }
        };
        if let Some(composed) = next {
            self.session.push(Role::User, composed);
            self.info("running queued message");
            self.save_session();
            self.start_run(inflight);
        }
    }

    fn command(&mut self, line: &str, inflight: &mut Option<tokio::task::JoinHandle<()>>) {
        let (cmd, arg) = line
            .split_once(' ')
            .map(|(c, a)| (c, a.trim()))
            .unwrap_or((line, ""));
        match cmd {
            "/help" | "/h" => self.info(HELP),
            "/quit" | "/q" | "/exit" => self.should_quit = true,
            "/clear" | "/new" => {
                self.session.clear();
                self.entries.clear();
                self.attachments.clear();
                self.allow_all.store(false, Ordering::Relaxed);
                self.queue.lock().unwrap().clear();
                crate::todo::clear();
                self.sid = crate::sessions::new_id();
                if let Ok(mut g) = self.mcp_session.write() {
                    *g = self.sid.clone();
                }
                self.title = None;
                self.tokens_in = 0;
                self.tokens_out = 0;
                self.tokens_cached = 0;
                self.cost = 0.0;
                self.info("new session");
            }
            "/sessions" => {
                let list = crate::sessions::list();
                if list.is_empty() {
                    self.info("no saved sessions");
                } else {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let mut out = String::from("saved sessions (/resume <id prefix>):");
                    for (i, s) in list.iter().take(12).enumerate() {
                        let mark = if s.parent.is_some() { "↳ " } else { "" };
                        out.push_str(&format!(
                            "\n{}. {}{} · {} · {} msgs · {}",
                            i + 1,
                            mark,
                            &s.id[..s.id.len().min(12)],
                            s.title,
                            s.count,
                            fmt_age(now.saturating_sub(s.updated))
                        ));
                    }
                    self.info(out);
                }
            }
            "/resume" => {
                if !matches!(self.phase, Phase::Idle)
                    || self.confirm.is_some()
                    || self.ask.is_some()
                {
                    self.info("wait for the current run to finish");
                    return;
                }
                let target = if arg.is_empty() {
                    // default to the latest main session, not a subagent child
                    crate::sessions::list()
                        .into_iter()
                        .find(|s| s.parent.is_none())
                        .map(|s| s.id)
                } else {
                    crate::sessions::list()
                        .into_iter()
                        .find(|s| s.id.starts_with(arg))
                        .map(|s| s.id)
                };
                let Some(t) = target else {
                    self.info("no matching session");
                    return;
                };
                match crate::sessions::load(&t) {
                    Ok(st) => self.load_session(st),
                    Err(e) => self.info(format!("error: {e:#}")),
                }
            }
            "/model" => {
                if arg.is_empty() {
                    self.info(format!(
                        "model: {}\nconfig: {}",
                        self.model,
                        crate::config::config_path().display()
                    ));
                } else {
                    self.model = arg.to_string();
                    self.cfg.provider.model = arg.to_string();
                    self.refresh_system_prompt();
                    match self.cfg.save() {
                        Ok(_) => self.info(format!("model: {}", self.model)),
                        Err(e) => self.info(format!("model: {} (not saved: {e:#})", self.model)),
                    }
                }
            }
            "/models" => {
                let kind = self.cfg.provider.kind.clone();
                let base = self.cfg.provider.base_url.clone();
                let key = self.cfg.api_key().unwrap_or_default();
                let tx = self.tx.clone();
                self.info("fetching models...");
                tokio::spawn(async move {
                    match crate::provider::list_models(&kind, base.as_deref(), &key).await {
                        Ok(list) if list.is_empty() => {
                            let _ = tx.send(ApiEvent::Note("no models found".into()));
                        }
                        Ok(list) => {
                            let _ = tx.send(ApiEvent::Note(format!(
                                "models ({}):\n{}",
                                list.len(),
                                list.join("\n")
                            )));
                        }
                        Err(e) => {
                            let _ = tx.send(ApiEvent::Note(format!("error: {e:#}")));
                        }
                    }
                });
            }
            "/mcpauth" => {
                if arg.is_empty() {
                    let mut out = String::from("mcp auth:");
                    for c in &self.cfg.mcp {
                        out.push_str(&format!("\n  {}", crate::mcpauth::status_line(c)));
                    }
                    self.info(out);
                } else {
                    let mut parts = arg.split_whitespace();
                    let name = parts.next().unwrap_or("").to_string();
                    // a pasted authorization code resumes a pending flow
                    // (works after a restart — the verifier is persisted)
                    let code = parts.next().map(|s| s.to_string());
                    let cfgs = self.cfg.mcp.clone();
                    let hooks = self.mcp_hooks();
                    let tx = self.tx.clone();
                    let mcp_slot = self.mcp.clone();
                    match code {
                        Some(code) => {
                            self.info(format!(
                                "finishing OAuth for {name} with the pasted code..."
                            ));
                            tokio::spawn(async move {
                                match crate::mcpauth::finish_auth(&name, &cfgs, &code).await {
                                    Ok(msg) => {
                                        let _ = tx.send(ApiEvent::Note(msg));
                                        for l in mcp::reconnect_one(&mcp_slot, &cfgs, &hooks, &name)
                                            .await
                                        {
                                            let _ = tx.send(ApiEvent::Note(l));
                                        }
                                    }
                                    Err(e) => {
                                        let _ = tx.send(ApiEvent::Note(format!("error: {e:#}")));
                                    }
                                }
                            });
                        }
                        None => {
                            self.info(format!("starting OAuth for {name} — check your browser"));
                            tokio::spawn(async move {
                                match crate::mcpauth::authorize_flow(&name, &cfgs).await {
                                    Ok(msg) => {
                                        let _ = tx.send(ApiEvent::Note(msg));
                                        for l in mcp::reconnect_one(&mcp_slot, &cfgs, &hooks, &name)
                                            .await
                                        {
                                            let _ = tx.send(ApiEvent::Note(l));
                                        }
                                    }
                                    Err(e) => {
                                        let _ = tx.send(ApiEvent::Note(format!("error: {e:#}")));
                                    }
                                }
                            });
                        }
                    }
                }
            }
            "/mcpadd" => {
                let cfg = match mcp::parse_add(arg) {
                    Ok(c) => c,
                    Err(e) => {
                        self.info(format!("error: {e:#}"));
                        return;
                    }
                };
                let name = cfg.name.clone();
                self.cfg.mcp.retain(|c| c.name != name);
                self.cfg.mcp.push(cfg.clone());
                let saved = match self.cfg.save() {
                    Ok(_) => "saved to config".to_string(),
                    Err(e) => format!("not saved: {e:#}"),
                };
                let hooks = self.mcp_hooks();
                let slot = self.mcp.clone();
                let tx = self.tx.clone();
                self.info(format!("adding mcp {name} ({saved})..."));
                tokio::spawn(async move {
                    let existing = slot.lock().unwrap().clone();
                    let msg = match existing {
                        Some(m) => match m.add(&cfg, &hooks).await {
                            Ok(sum) => format!("mcp {name}: connected ({sum})"),
                            Err(e) => format!("error: {e:#}"),
                        },
                        // no live client yet: start one with this server
                        None => {
                            let (client, mut logs) =
                                mcp::connect_all(std::slice::from_ref(&cfg), &hooks).await;
                            *slot.lock().unwrap() = client;
                            logs.pop()
                                .unwrap_or_else(|| format!("mcp {name}: connected"))
                        }
                    };
                    let _ = tx.send(ApiEvent::Note(msg));
                });
            }
            "/mcpconnect" => {
                if arg.is_empty() {
                    self.info("usage: /mcpconnect <name> — (re)connect a configured server");
                    return;
                }
                let name = arg.trim().to_string();
                let cfgs = self.cfg.mcp.clone();
                let hooks = self.mcp_hooks();
                let slot = self.mcp.clone();
                let tx = self.tx.clone();
                self.info(format!("connecting mcp {name}..."));
                tokio::spawn(async move {
                    for l in mcp::reconnect_one(&slot, &cfgs, &hooks, &name).await {
                        let _ = tx.send(ApiEvent::Note(l));
                    }
                });
            }
            "/mcpdisconnect" => {
                if arg.is_empty() {
                    self.info("usage: /mcpdisconnect <name> — drop the live connection (config untouched) /mcpconnect brings it back");
                    return;
                }
                let name = arg.trim().to_string();
                let mcp = self.mcp.lock().unwrap().clone();
                let tx = self.tx.clone();
                self.info(format!("disconnecting mcp {name}..."));
                tokio::spawn(async move {
                    let Some(m) = mcp else {
                        let _ = tx.send(ApiEvent::Note("mcp is not configured".into()));
                        return;
                    };
                    let msg = m
                        .disconnect(&name)
                        .await
                        .unwrap_or_else(|e| format!("error: {e:#}"));
                    let _ = tx.send(ApiEvent::Note(msg));
                });
            }
            "/mcplogout" => {
                if arg.is_empty() {
                    self.info("usage: /mcplogout <name> — drop the stored oauth tokens; the next use starts a fresh /mcpauth flow");
                    return;
                }
                let name = arg.trim().to_string();
                let msg = if crate::mcpauth::logout(&name) {
                    format!("mcp {name}: signed out (tokens cleared, /mcpauth starts fresh)")
                } else {
                    format!("mcp {name}: no stored credentials")
                };
                self.info(msg);
            }
            "/mcpres" => {
                let mcp = self.mcp.lock().unwrap().clone();
                let filter = arg.trim().to_string();
                let tx = self.tx.clone();
                self.info("listing mcp resources...");
                tokio::spawn(async move {
                    let Some(m) = mcp else {
                        let _ = tx.send(ApiEvent::Note("mcp is not configured".into()));
                        return;
                    };
                    let list: Vec<_> = m
                        .resources()
                        .await
                        .into_iter()
                        .filter(|r| filter.is_empty() || r.server == filter)
                        .collect();
                    let tpls: Vec<_> = m
                        .templates()
                        .await
                        .into_iter()
                        .filter(|t| filter.is_empty() || t.server == filter)
                        .collect();
                    let subs = m.subscriptions().await;
                    if list.is_empty() && tpls.is_empty() {
                        let _ = tx.send(ApiEvent::Note(format!(
                            "no mcp resources{}",
                            if filter.is_empty() {
                                String::new()
                            } else {
                                format!(" on {filter}")
                            }
                        )));
                        return;
                    }
                    let mut out = String::new();
                    if !list.is_empty() {
                        out.push_str(&format!("mcp resources ({}):", list.len()));
                        for r in list {
                            out.push_str(&format!("\n  {}  {}", r.server, r.uri));
                            if subs.iter().any(|(s, u)| s == &r.server && u == &r.uri) {
                                out.push_str(" [subscribed]");
                            }
                            if !r.name.is_empty() && r.name != r.uri {
                                out.push_str(&format!(" ({})", r.name));
                            }
                            if !r.description.is_empty() {
                                out.push_str(&format!(" — {}", r.description));
                            }
                        }
                    }
                    if !tpls.is_empty() {
                        if !out.is_empty() {
                            out.push_str("\n\n");
                        }
                        out.push_str(&format!("mcp templates ({}):", tpls.len()));
                        for t in tpls {
                            out.push_str(&format!("\n  {}  {}", t.server, t.uri_template));
                            if !t.name.is_empty() {
                                out.push_str(&format!(" ({})", t.name));
                            }
                            if !t.description.is_empty() {
                                out.push_str(&format!(" — {}", t.description));
                            }
                        }
                        out.push_str("\n\nfill the braces with real values and read via /mcpread <server> <uri>");
                    }
                    let _ = tx.send(ApiEvent::Note(out));
                });
            }
            "/mcpread" => {
                let Some((server, uri)) = arg.trim().split_once(char::is_whitespace) else {
                    self.info("usage: /mcpread <server> <uri> — run /mcpres to list resources");
                    return;
                };
                let server = server.trim().to_string();
                let uri = uri.trim().to_string();
                let mcp = self.mcp.lock().unwrap().clone();
                let tx = self.tx.clone();
                self.info(format!("reading {server} {uri}..."));
                tokio::spawn(async move {
                    let Some(m) = mcp else {
                        let _ = tx.send(ApiEvent::Note("mcp is not configured".into()));
                        return;
                    };
                    match m.read_resource(&server, &uri).await {
                        Ok(text) => {
                            let _ = tx.send(ApiEvent::Note(
                                crate::provider::truncate(&text).trim().to_string(),
                            ));
                        }
                        Err(e) => {
                            let _ = tx.send(ApiEvent::Note(format!("error: {e:#}")));
                        }
                    }
                });
            }
            "/mcpprompt" => {
                let parts: Vec<String> = arg.split_whitespace().map(|s| s.to_string()).collect();
                let mcp = self.mcp.lock().unwrap().clone();
                let tx = self.tx.clone();
                if parts.is_empty() {
                    self.info("listing mcp prompts...");
                    tokio::spawn(async move {
                        let Some(m) = mcp else {
                            let _ = tx.send(ApiEvent::Note("mcp is not configured".into()));
                            return;
                        };
                        let list = m.prompts().await;
                        if list.is_empty() {
                            let _ = tx.send(ApiEvent::Note("no mcp prompts".into()));
                            return;
                        }
                        let mut out = format!("mcp prompts ({}):", list.len());
                        for p in list {
                            out.push_str(&format!("\n  {}  {}", p.server, p.name));
                            if !p.description.is_empty() {
                                out.push_str(&format!(" — {}", p.description));
                            }
                            if !p.arguments.is_empty() {
                                let names: Vec<String> = p
                                    .arguments
                                    .iter()
                                    .map(|a| {
                                        if a.required {
                                            format!("{}*", a.name)
                                        } else {
                                            a.name.clone()
                                        }
                                    })
                                    .collect();
                                out.push_str(&format!(" (args: {})", names.join(", ")));
                            }
                        }
                        out.push_str("\n\nusage: /mcpprompt <server> <name> [key=value ...]");
                        let _ = tx.send(ApiEvent::Note(out));
                    });
                } else if parts.len() < 2 {
                    self.info("usage: /mcpprompt <server> <name> [key=value ...]");
                } else {
                    let server = parts[0].clone();
                    let name = parts[1].clone();
                    let mut a = serde_json::Map::new();
                    for p in &parts[2..] {
                        if let Some((k, v)) = p.split_once('=') {
                            a.insert(k.to_string(), serde_json::Value::String(v.to_string()));
                        } else {
                            self.info(format!("ignoring bad arg {p} (expected key=value)"));
                        }
                    }
                    self.info(format!("fetching prompt {server}/{name}..."));
                    tokio::spawn(async move {
                        let Some(m) = mcp else {
                            let _ = tx.send(ApiEvent::Note("mcp is not configured".into()));
                            return;
                        };
                        match m
                            .get_prompt(&server, &name, &serde_json::Value::Object(a))
                            .await
                        {
                            Ok(msgs) if msgs.is_empty() => {
                                let _ =
                                    tx.send(ApiEvent::Note("prompt returned no messages".into()));
                            }
                            Ok(msgs) => {
                                let mut text = String::new();
                                for (role, t) in &msgs {
                                    if role != "user" {
                                        text.push_str(&format!("[{role}]\n"));
                                    }
                                    text.push_str(t);
                                    text.push_str("\n\n");
                                }
                                let _ = tx.send(ApiEvent::Submit(text.trim().to_string()));
                            }
                            Err(e) => {
                                let _ = tx.send(ApiEvent::Note(format!("error: {e:#}")));
                            }
                        }
                    });
                }
            }
            "/mcpstatus" => {
                let cfgs = self.cfg.mcp.clone();
                let mcp = self.mcp.clone();
                let tx = self.tx.clone();
                self.info("checking mcp servers...");
                tokio::spawn(async move {
                    let list = mcp::status(&mcp, &cfgs).await;
                    if list.is_empty() {
                        let _ = tx.send(ApiEvent::Note(
                            "no mcp servers configured ([[mcp]] in config.toml)".into(),
                        ));
                        return;
                    }
                    let mut out = format!("mcp servers ({}):", list.len());
                    for e in list {
                        out.push_str(&format!("\n  [{}] {}: {}", e.state, e.name, e.detail));
                    }
                    let _ = tx.send(ApiEvent::Note(out));
                });
            }
            "/mcpsub" | "/mcpunsub" => {
                let Some((server, uri)) = arg.trim().split_once(char::is_whitespace) else {
                    self.info("usage: /mcpsub <server> <uri> — run /mcpres to list resources");
                    return;
                };
                let server = server.trim().to_string();
                let uri = uri.trim().to_string();
                let unsub = cmd == "/mcpunsub";
                let mcp = self.mcp.lock().unwrap().clone();
                let tx = self.tx.clone();
                self.info(format!(
                    "{} {server} {uri}...",
                    if unsub {
                        "unsubscribing"
                    } else {
                        "subscribing"
                    }
                ));
                tokio::spawn(async move {
                    let Some(m) = mcp else {
                        let _ = tx.send(ApiEvent::Note("mcp is not configured".into()));
                        return;
                    };
                    let res = if unsub {
                        m.unsubscribe(&server, &uri).await
                    } else {
                        m.subscribe(&server, &uri).await
                    };
                    match res {
                        Ok(()) => {
                            let _ = tx.send(ApiEvent::Note(format!(
                                "mcp {server}: {uri} {}",
                                if unsub {
                                    "unsubscribed"
                                } else {
                                    "subscribed — updates land in this chat"
                                }
                            )));
                        }
                        Err(e) => {
                            let _ = tx.send(ApiEvent::Note(format!("error: {e:#}")));
                        }
                    }
                });
            }
            "/mcplog" => {
                let parts: Vec<String> = arg.split_whitespace().map(|s| s.to_string()).collect();
                if parts.first().map(|p| p == "set").unwrap_or(false) {
                    if parts.len() < 3 {
                        self.info("usage: /mcplog set <server|all> <level> — levels: debug info notice warning error critical alert emergency");
                        return;
                    }
                    let server = if parts[1] == "all" {
                        String::new()
                    } else {
                        parts[1].clone()
                    };
                    let level = parts[2].clone();
                    let mcp = self.mcp.lock().unwrap().clone();
                    let tx = self.tx.clone();
                    self.info(format!("setting mcp log level {level}..."));
                    tokio::spawn(async move {
                        let Some(m) = mcp else {
                            let _ = tx.send(ApiEvent::Note("mcp is not configured".into()));
                            return;
                        };
                        for l in m.set_log_level(&server, &level).await {
                            let _ = tx.send(ApiEvent::Note(l));
                        }
                    });
                } else {
                    let filter = parts.first().cloned().unwrap_or_default();
                    let list: Vec<mcp::McpLogEntry> = self
                        .mcp
                        .lock()
                        .unwrap()
                        .clone()
                        .map(|m| m.logs())
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|e| filter.is_empty() || e.server == filter)
                        .collect();
                    if list.is_empty() {
                        self.info("no mcp log messages yet — warning and above pop into the chat");
                        return;
                    }
                    let tail = &list[list.len().saturating_sub(20)..];
                    let mut out = format!("mcp logs (last {}):", tail.len());
                    for e in tail {
                        let who = if e.logger.is_empty() {
                            e.server.clone()
                        } else {
                            format!("{} {}", e.server, e.logger)
                        };
                        out.push_str(&format!("\n  [{}] {}: {}", e.level, who, e.data));
                    }
                    self.info(out);
                }
            }
            "/jstools" => {
                if arg.trim() == "reload" {
                    crate::jstools::reload();
                    self.info(format!(
                        "JS tools rescanned:\n{}",
                        crate::jstools::summary()
                    ));
                } else {
                    self.info(format!("JS tools:\n{}", crate::jstools::summary()));
                }
            }
            "/sandbox" => self.sandbox_command(arg),
            "/file" => {
                if arg.is_empty() {
                    self.info("usage: /file <path>");
                } else if files::is_image(arg) {
                    match files::read_image(arg) {
                        Ok((mime, data)) => {
                            self.attachments
                                .push((arg.to_string(), format!("data:{mime};base64,{data}")));
                            self.info(format!("attached image {arg} ({mime})"));
                        }
                        Err(e) => self.info(format!("error: {e:#}")),
                    }
                } else {
                    match files::read_attach(arg) {
                        Ok(content) => {
                            let size = content.len();
                            self.attachments.push((arg.to_string(), content));
                            self.info(format!("attached {arg} ({size} bytes)"));
                        }
                        Err(e) => self.info(format!("error: {e:#}")),
                    }
                }
            }
            "/undo" | "/u" => match crate::snapshot::undo() {
                Some(s) => self.info(s),
                None => self.info("nothing to undo"),
            },
            "/redo" => match crate::snapshot::redo() {
                Some(s) => self.info(s),
                None => self.info("nothing to redo"),
            },
            "/plan" => {
                self.plan = !self.plan;
                self.info(if self.plan {
                    "plan mode on: read-only research, the agent will propose a plan instead of making changes"
                } else {
                    "plan mode off"
                });
            }
            "/init" => {
                if !matches!(self.phase, Phase::Idle) {
                    self.info("wait for the current run to finish");
                    return;
                }
                let cwd = std::env::current_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                self.session
                    .push(Role::User, crate::commands::init_prompt(&cwd));
                self.info("initializing AGENTS.md...");
                self.start_run(inflight);
            }
            "/compact" => {
                if !matches!(self.phase, Phase::Idle)
                    || self.confirm.is_some()
                    || self.ask.is_some()
                {
                    self.info("wait for the current run to finish");
                    return;
                }
                if self.session.messages.len() < 6 {
                    self.info("nothing to compact yet");
                    return;
                }
                let req = ChatRequest {
                    system: self.session.system.clone(),
                    messages: self.session.messages.clone(),
                    model: self.model.clone(),
                    max_tokens: self.cfg.provider.max_tokens,
                    temperature: self.cfg.provider.temperature,
                    top_p: self.cfg.provider.top_p,
                    stream: false,
                    tools: Vec::new(),
                };
                let provider = self.provider.clone();
                let tx = self.tx.clone();
                let keep = self.cfg.agent.compaction.keep;
                self.info("compacting context...");
                tokio::spawn(async move {
                    let mut msgs = req.messages.clone();
                    if agent::compact_session(provider, &req, &mut msgs, &tx, keep).await {
                        let _ = tx.send(ApiEvent::Done {
                            text: "context compacted".into(),
                            messages: msgs,
                        });
                    } else {
                        let _ = tx.send(ApiEvent::Note("compaction failed".into()));
                    }
                });
            }
            "/export" => {
                let path = if arg.is_empty() {
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    format!("hiderola-session-{ts}.md")
                } else {
                    arg.to_string()
                };
                let md = crate::commands::export_markdown("", &self.session.messages);
                match std::fs::write(&path, md) {
                    Ok(_) => self.info(format!("exported to {path}")),
                    Err(e) => self.info(format!("error: {e:#}")),
                }
            }
            _ => match crate::commands::get(cmd) {
                Some(c) => {
                    if !matches!(self.phase, Phase::Idle) {
                        self.info("wait for the current run to finish");
                        return;
                    }
                    let text = crate::commands::render(&c.template, arg);
                    self.session.push(Role::User, text);
                    self.start_run(inflight);
                }
                None => {
                    let cmds = crate::commands::discover();
                    let hint = if cmds.is_empty() {
                        "no custom commands found (.hi-derola/commands/<name>.md or ~/.config/hi-derola/commands/<name>.md)".to_string()
                    } else {
                        let mut s = String::from("available custom commands:");
                        for c in cmds {
                            let d = c.description.trim();
                            let tail = if d.is_empty() {
                                String::new()
                            } else {
                                format!(" — {d}")
                            };
                            s.push_str(&format!("\n  /{}{tail}", c.name));
                        }
                        s
                    };
                    self.info(format!(
                        "unknown command: {cmd}\n{hint}\ntype /help for the built-in commands"
                    ));
                }
            },
        }
    }
}

pub async fn run(
    terminal: &mut ratatui::DefaultTerminal,
    cfg: Config,
    provider: Arc<dyn Provider>,
) -> Result<()> {
    crossterm::execute!(std::io::stdout(), EnableMouseCapture)?;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut app = App::new(cfg, provider, tx);
    app.connect_mcp().await;
    let mut inflight: Option<tokio::task::JoinHandle<()>> = None;
    let res = loop {
        terminal.draw(|f| ui::draw(f, &app))?;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                ApiEvent::Submit(text) => app.submit_text(text, &mut inflight),
                other => app.on_api(other),
            }
        }
        app.resume_queued(&mut inflight);
        if app.should_quit {
            break Ok(());
        }
        if crossterm::event::poll(std::time::Duration::from_millis(30))? {
            match crossterm::event::read()? {
                Event::Key(k) => {
                    if k.kind == KeyEventKind::Press
                        && k.code == KeyCode::Esc
                        && matches!(app.phase, Phase::Waiting | Phase::Confirm)
                    {
                        if app.phase == Phase::Confirm {
                            if let Some(c) = app.confirm.take() {
                                let _ = c.rx.send(ConfirmReply::default());
                            }
                        }
                        if let Some(h) = inflight.take() {
                            h.abort();
                        }
                        while rx.try_recv().is_ok() {}
                        app.cancelled();
                        app.resume_queued(&mut inflight);
                    } else {
                        app.on_key(k, &mut inflight);
                    }
                }
                Event::Paste(s) => app.input.push_str(&s),
                Event::Mouse(m) => match m.kind {
                    MouseEventKind::ScrollUp => app.scroll_up = app.scroll_up.saturating_add(3),
                    MouseEventKind::ScrollDown => app.scroll_up = app.scroll_up.saturating_sub(3),
                    _ => {}
                },
                _ => {}
            }
        }
    };
    crossterm::execute!(std::io::stdout(), DisableMouseCapture)?;
    res
}
