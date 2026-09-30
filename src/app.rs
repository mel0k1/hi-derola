use std::sync::Arc;

use anyhow::Result;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind, EnableMouseCapture,
    DisableMouseCapture,
};
use tokio::sync::mpsc;

use crate::chat::{Role, Session};
use crate::config::Config;
use crate::files::{self, WriteBlock};
use crate::provider::{ApiEvent, ChatRequest, Provider};
use crate::ui;

pub enum Kind {
    You,
    Bot,
    Info,
}

pub struct Entry {
    pub kind: Kind,
    pub text: String,
}

pub enum Phase {
    Idle,
    Waiting,
    Confirm,
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
    pub pending: Vec<WriteBlock>,
    pub pending_idx: usize,
    pub attachments: Vec<(String, String)>,
    pub streaming: Option<usize>,
    pub reasoning: Option<usize>,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub should_quit: bool,
    pub status: String,
    tx: mpsc::UnboundedSender<ApiEvent>,
}

const HELP: &str = "commands:\n  /file <path>   attach file to next message\n  /model <name>  switch model, saved to config\n  /model         show current model\n  /clear         start new session\n  /quit          exit\nkeys:\n  enter send  esc cancel/quit  pgup/pgdn scroll  ctrl+c quit\nwrites:\n  model outputs ```path blocks, confirm each with y/n";

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
             Be concise and practical.\n\n\
             When asked to create or modify files, answer with fenced code blocks where the \
             info string is the target file path, one block per file:\n\n\
             ```src/main.rs\n\
             // complete file content\n\
             ```\n\n\
             Always output the complete file content, never diffs, never ellipses."
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
            pending: Vec::new(),
            pending_idx: 0,
            attachments: Vec::new(),
            streaming: None,
            reasoning: None,
            tokens_in: 0,
            tokens_out: 0,
            should_quit: false,
            status,
            tx,
        }
    }

    fn info(&mut self, text: impl Into<String>) {
        self.entries.push(Entry {
            kind: Kind::Info,
            text: text.into(),
        });
        self.scroll_up = 0;
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
            ApiEvent::Usage { input, output } => {
                self.tokens_in += input;
                self.tokens_out += output;
                self.status = self.status_line();
            }
            ApiEvent::Done(full) => {
                match self.streaming {
                    Some(i) => self.entries[i].text = full.clone(),
                    None => {
                        if !full.is_empty() {
                            self.entries.push(Entry {
                                kind: Kind::Bot,
                                text: full.clone(),
                            });
                        }
                    }
                }
                self.session.push(Role::Assistant, full.clone());
                let blocks = files::parse_write_blocks(&full);
                if blocks.is_empty() {
                    self.phase = Phase::Idle;
                } else {
                    self.pending = blocks;
                    self.pending_idx = 0;
                    self.phase = Phase::Confirm;
                }
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
        self.streaming = None;
        self.reasoning = None;
        self.info("cancelled");
        self.status = self.status_line();
    }

    fn status_line(&self) -> String {
        match self.phase {
            Phase::Waiting => "thinking...".into(),
            Phase::Confirm => {
                let b = &self.pending[self.pending_idx];
                format!("apply {}?  y/n", b.path)
            }
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
        match code {
            KeyCode::Char('y') => {
                let block = self.pending[self.pending_idx].clone();
                match files::apply(&block) {
                    Ok(n) => self.info(format!("wrote {} ({n} lines)", block.path)),
                    Err(e) => self.info(format!("error: {e:#}")),
                }
                self.pending_idx += 1;
            }
            KeyCode::Char('n') => {
                let path = self.pending[self.pending_idx].path.clone();
                self.info(format!("skipped {path}"));
                self.pending_idx += 1;
            }
            KeyCode::Char('a') => {
                for b in self.pending[self.pending_idx..].to_vec() {
                    match files::apply(&b) {
                        Ok(n) => self.info(format!("wrote {} ({n} lines)", b.path)),
                        Err(e) => self.info(format!("error: {e:#}")),
                    }
                }
                self.pending_idx = self.pending.len();
            }
            KeyCode::Char('s') | KeyCode::Esc => {
                self.pending_idx = self.pending.len();
            }
            _ => return,
        }
        if self.pending_idx >= self.pending.len() {
            self.pending.clear();
            self.pending_idx = 0;
            self.phase = Phase::Idle;
        }
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
            KeyCode::Char(c)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                self.input.push(c);
            }
            _ => {}
        }
    }

    fn submit(&mut self, inflight: &mut Option<tokio::task::JoinHandle<()>>) {
        let text = self.input.trim().to_string();
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
        let req = ChatRequest {
            system: self.session.system.clone(),
            messages: self.session.messages.clone(),
            model: self.model.clone(),
            max_tokens: self.cfg.provider.max_tokens,
            stream: self.cfg.provider.stream,
        };
        let handle = tokio::spawn(async move {
            if let Err(e) = provider.chat(req, tx.clone()).await {
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
                        && matches!(app.phase, Phase::Waiting)
                    {
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
