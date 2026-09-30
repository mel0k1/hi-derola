use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind, EnableMouseCapture,
    DisableMouseCapture,
};
use tokio::sync::mpsc;
use tokio::sync::oneshot;

use crate::agent;
use crate::chat::{Role, Session};
use crate::config::Config;
use crate::files;
use crate::mcp::{self, McpClient};
use crate::provider::{ApiEvent, ChatRequest, Provider};
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
}

pub struct ConfirmCtx {
    pub name: String,
    pub args: String,
    pub rx: oneshot::Sender<bool>,
}

pub struct App {
    pub cfg: Config,
    pub model: String,
    pub provider: Arc<dyn Provider>,
    pub session: Session,
    pub entries: Vec<Entry>,
    pub input: String,
    pub scroll_up: usize,
    pub phase: Phase,
    pub confirm: Option<ConfirmCtx>,
    pub attachments: Vec<(String, String)>,
    pub streaming: Option<usize>,
    pub reasoning: Option<usize>,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub should_quit: bool,
    pub status: String,
    allow_all: Arc<AtomicBool>,
    history: Vec<String>,
    hist_idx: usize,
    draft: String,
    mcp: Option<Arc<McpClient>>,
    tx: mpsc::UnboundedSender<ApiEvent>,
}

const HELP: &str = "commands:\n  /file <path>   attach file to next message\n  /model <name>  switch model, saved to config\n  /model         show current model\n  /clear         start new session\n  /quit          exit\nkeys:\n  enter send  esc cancel/quit  up/down history  pgup/pgdn scroll  ctrl+c quit\ntools:\n  read/write/edit/list/bash + mcp servers, mutations ask y/n/a";

fn fmt_tokens(n: u64) -> String {
    if n < 1000 {
        n.to_string()
    } else {
        format!("{:.1}k", n as f64 / 1000.0)
    }
}

impl App {
    pub fn new(cfg: Config, provider: Arc<dyn Provider>, tx: mpsc::UnboundedSender<ApiEvent>) -> Self {
        let model = cfg.provider.model.clone();
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let system = format!(
            "You are hi-derola, a coding assistant running in the user's terminal.\n\
             Working directory: {cwd}\n\
             Be concise and practical. Use markdown for formatting.\n\n\
             Use the provided tools to work with files and run commands instead of printing \
             code fences with file contents. Prefer read_file before modifying a file. \
             write_file writes the complete file content."
        );
        let status = format!("{} · {}", provider.name(), model);
        Self {
            cfg,
            model,
            provider,
            session: Session::new(system),
            entries: Vec::new(),
            input: String::new(),
            scroll_up: 0,
            phase: Phase::Idle,
            confirm: None,
            attachments: Vec::new(),
            streaming: None,
            reasoning: None,
            tokens_in: 0,
            tokens_out: 0,
            should_quit: false,
            status,
            allow_all: Arc::new(AtomicBool::new(false)),
            history: Vec::new(),
            hist_idx: 0,
            draft: String::new(),
            mcp: None,
            tx,
        }
    }

    pub async fn connect_mcp(&mut self) {
        let cfgs = self.cfg.mcp.clone();
        let (client, logs) = mcp::connect_all(&cfgs).await;
        for l in logs {
            self.info(l);
        }
        self.mcp = client;
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
            ApiEvent::Note(s) => self.info(s),
            ApiEvent::Tool { name, detail, diff } => {
                self.flush_stream();
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
            ApiEvent::Usage { input, output } => {
                self.tokens_in += input;
                self.tokens_out += output;
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
            }
            ApiEvent::Failed(e) => {
                self.info(format!("error: {e}"));
                self.phase = Phase::Idle;
                self.streaming = None;
                self.reasoning = None;
            }
        }
        self.status = self.status_line();
    }

    pub fn cancelled(&mut self) {
        self.phase = Phase::Idle;
        self.confirm = None;
        self.streaming = None;
        self.reasoning = None;
        self.info("cancelled");
        self.status = self.status_line();
    }

    fn status_line(&self) -> String {
        match self.phase {
            Phase::Waiting => "thinking...".into(),
            Phase::Confirm => match &self.confirm {
                Some(c) => format!("run {}?  y/n/a", c.name),
                None => "confirm...".into(),
            },
            Phase::Idle => {
                let mut s = format!("{} · {}", self.provider.name(), self.model);
                if self.tokens_in > 0 || self.tokens_out > 0 {
                    s.push_str(&format!(
                        " · {} in · {} out",
                        fmt_tokens(self.tokens_in),
                        fmt_tokens(self.tokens_out)
                    ));
                }
                s
            }
        }
    }

    fn confirm_key(&mut self, code: KeyCode) {
        let Some(c) = self.confirm.take() else {
            self.phase = Phase::Idle;
            return;
        };
        match code {
            KeyCode::Char('y') => {
                let _ = c.rx.send(true);
            }
            KeyCode::Char('a') => {
                self.allow_all.store(true, Ordering::Relaxed);
                let _ = c.rx.send(true);
            }
            KeyCode::Char('n') | KeyCode::Char('s') => {
                let _ = c.rx.send(false);
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
                self.confirm_key(key.code);
                return;
            }
            Phase::Waiting => {
                match key.code {
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
            self.command(&text);
            self.status = self.status_line();
            return;
        }
        if text.is_empty() && self.attachments.is_empty() {
            return;
        }
        let mut composed = String::new();
        for (path, content) in &self.attachments {
            composed.push_str(&format!("[file: {path}]\n{content}\n\n"));
        }
        composed.push_str(&text);
        self.attachments.clear();
        self.input.clear();
        self.session.push(Role::User, composed);
        self.entries.push(Entry {
            kind: Kind::You,
            text,
        });
        self.scroll_up = 0;
        self.phase = Phase::Waiting;
        self.status = self.status_line();

        let provider = self.provider.clone();
        let tx = self.tx.clone();
        let allow_all = self.allow_all.clone();
        let mcp = self.mcp.clone();
        let req = ChatRequest {
            system: self.session.system.clone(),
            messages: self.session.messages.clone(),
            model: self.model.clone(),
            max_tokens: self.cfg.provider.max_tokens,
            temperature: self.cfg.provider.temperature,
            top_p: self.cfg.provider.top_p,
            stream: self.cfg.provider.stream,
            tools: Vec::new(),
        };
        let handle = tokio::spawn(async move {
            if let Err(e) = agent::run(provider, req, tx.clone(), allow_all, mcp).await {
                let _ = tx.send(ApiEvent::Failed(format!("{e:#}")));
            }
        });
        *inflight = Some(handle);
    }

    fn command(&mut self, line: &str) {
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
                self.info("new session");
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
                    match self.cfg.save() {
                        Ok(_) => self.info(format!("model: {}", self.model)),
                        Err(e) => self.info(format!("model: {} (not saved: {e:#})", self.model)),
                    }
                }
            }
            "/file" => {
                if arg.is_empty() {
                    self.info("usage: /file <path>");
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
            _ => self.info(format!("unknown command: {cmd}, try /help")),
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
            app.on_api(ev);
        }
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
                                let _ = c.rx.send(false);
                            }
                        }
                        if let Some(h) = inflight.take() {
                            h.abort();
                        }
                        while rx.try_recv().is_ok() {}
                        app.cancelled();
                    } else {
                        app.on_key(k, &mut inflight);
                    }
                }
                Event::Paste(s) => app.input.push_str(&s),
                Event::Mouse(m) => match m.kind {
                    MouseEventKind::ScrollUp => app.scroll_up = app.scroll_up.saturating_add(3),
                    MouseEventKind::ScrollDown => {
                        app.scroll_up = app.scroll_up.saturating_sub(3)
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    };
    crossterm::execute!(std::io::stdout(), DisableMouseCapture)?;
    res
}
