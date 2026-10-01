use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde_json::Value;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use tokio::sync::mpsc;

slint::include_modules!();

use crate::agent;
use crate::app::help_text;
use crate::chat::Role;
use crate::config::Config;
use crate::files;
use crate::mcp::{self, McpClient};
use crate::provider::{ApiEvent, ChatRequest, Provider};
use crate::{md, snapshot, tools};

pub struct Shared {
    session: Mutex<crate::chat::Session>,
    streaming: Mutex<Option<usize>>,
    reasoning: Mutex<Option<usize>>,
    confirm: Mutex<Option<tokio::sync::oneshot::Sender<bool>>>,
    tokens: Mutex<(u64, u64)>,
    attachments: Mutex<Vec<(String, String)>>,
    model: Mutex<String>,
    inflight: Mutex<Option<tokio::task::JoinHandle<()>>>,
    allow_all: Arc<AtomicBool>,
    provider: Arc<dyn Provider>,
    cfg: Mutex<Config>,
    mcp: Option<Arc<McpClient>>,
    tx: mpsc::UnboundedSender<ApiEvent>,
    rt: tokio::runtime::Handle,
}

fn fmt_tokens(n: u64) -> String {
    if n < 1000 {
        n.to_string()
    } else {
        format!("{:.1}k", n as f64 / 1000.0)
    }
}

fn set_status(app: &MainWindow, sh: &Shared) {
    let (tin, tout) = *sh.tokens.lock().unwrap();
    let mut s = if app.get_waiting() {
        if app.get_confirming() {
            format!("confirm: {}", app.get_confirm_name())
        } else {
            "thinking...".to_string()
        }
    } else {
        format!("{} · {}", sh.provider.name(), sh.model.lock().unwrap().clone())
    };
    if tin > 0 || tout > 0 {
        s.push_str(&format!(
            " · {} in · {} out",
            fmt_tokens(tin),
            fmt_tokens(tout)
        ));
    }
    app.set_status(s.into());
}

fn confirm_detail(name: &str, args: &str) -> String {
    let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
    let head = if name == "bash" {
        v["command"].as_str().unwrap_or("").to_string()
    } else {
        tools::detail(name, args)
    };
    let mut s = head;
    for r in tools::preview(name, args) {
        s.push('\n');
        s.push_str(match r.tag {
            1 => "+ ",
            2 => "- ",
            _ => "  ",
        });
        s.push_str(&r.text);
    }
    s
}

fn flush(sh: &Shared, vm: &VecModel<MsgItem>) {
    let mut st = sh.streaming.lock().unwrap();
    if let Some(i) = *st {
        let empty = vm
            .row_data(i)
            .map(|r| r.text.trim().is_empty())
            .unwrap_or(true);
        if empty {
            vm.remove(i);
            if let Some(r) = sh.reasoning.lock().unwrap().as_mut() {
                if *r > i {
                    *r -= 1;
                }
            }
        }
    }
    *st = None;
}

fn info(vm: &VecModel<MsgItem>, text: &str) {
    vm.push(MsgItem {
        role: "info".into(),
        text: text.into(),
    });
}

fn apply(weak: &slint::Weak<MainWindow>, sh: &Arc<Shared>, ev: ApiEvent) {
    let Some(app) = weak.upgrade() else {
        return;
    };
    let m = app.get_messages();
    let vm = ModelRc::as_any(&m)
        .downcast_ref::<VecModel<MsgItem>>()
        .unwrap();
    match ev {
        ApiEvent::Chunk(s) => {
            let mut st = sh.streaming.lock().unwrap();
            match *st {
                Some(i) => {
                    if let Some(mut row) = vm.row_data(i) {
                        row.text.push_str(&s);
                        vm.set_row_data(i, row);
                    }
                }
                None => {
                    vm.push(MsgItem {
                        role: "bot".into(),
                        text: s.into(),
                    });
                    *st = Some(vm.row_count() - 1);
                }
            }
        }
        ApiEvent::Reasoning(s) => {
            let mut st = sh.reasoning.lock().unwrap();
            match *st {
                Some(i) => {
                    if let Some(mut row) = vm.row_data(i) {
                        row.text.push_str(&s);
                        vm.set_row_data(i, row);
                    }
                }
                None => {
                    vm.push(MsgItem {
                        role: "info".into(),
                        text: format!("reasoning: {s}").into(),
                    });
                    *st = Some(vm.row_count() - 1);
                }
            }
        }
        ApiEvent::Note(s) => info(&vm, &s),
        ApiEvent::Tool { name, detail, diff } => {
            flush(sh, &vm);
            info(&vm, &format!("tool {name} {detail}"));
            for r in diff {
                let (role, prefix) = match r.tag {
                    1 => ("diff-add", "+ "),
                    2 => ("diff-del", "- "),
                    _ => ("diff-ctx", "  "),
                };
                vm.push(MsgItem {
                    role: role.into(),
                    text: format!("{prefix}{}", r.text).into(),
                });
            }
        }
        ApiEvent::Confirm { name, args, rx } => {
            flush(sh, vm);
            *sh.confirm.lock().unwrap() = Some(rx);
            let detail = confirm_detail(&name, &args);
            app.set_confirm_name(name.into());
            app.set_confirm_detail(detail.into());
            app.set_confirming(true);
            set_status(&app, sh);
        }
        ApiEvent::Usage { input, output } => {
            let mut t = sh.tokens.lock().unwrap();
            t.0 += input;
            t.1 += output;
            drop(t);
            set_status(&app, sh);
        }
        ApiEvent::Done { text, messages } => {
            let plain = md::plain(&text);
            let st = *sh.streaming.lock().unwrap();
            match st {
                Some(i) => {
                    vm.set_row_data(
                        i,
                        MsgItem {
                            role: "bot".into(),
                            text: plain.into(),
                        },
                    );
                }
                None => {
                    if !text.is_empty() {
                        vm.push(MsgItem {
                            role: "bot".into(),
                            text: plain.into(),
                        });
                    }
                }
            }
            *sh.streaming.lock().unwrap() = None;
            *sh.reasoning.lock().unwrap() = None;
            sh.session.lock().unwrap().messages = messages;
            app.set_waiting(false);
            snapshot::end_turn();
            set_status(&app, sh);
        }
        ApiEvent::Failed(e) => {
            flush(sh, &vm);
            info(&vm, &format!("error: {e}"));
            *sh.streaming.lock().unwrap() = None;
            *sh.reasoning.lock().unwrap() = None;
            app.set_waiting(false);
            snapshot::end_turn();
            set_status(&app, sh);
        }
    }
}

fn command(app: &MainWindow, sh: &Arc<Shared>, vm: &VecModel<MsgItem>, line: &str) {
    let (cmd, arg) = line
        .split_once(' ')
        .map(|(c, a)| (c, a.trim()))
        .unwrap_or((line, ""));
    match cmd {
        "/help" | "/h" => info(vm, help_text()),
        "/quit" | "/q" | "/exit" => {
            let _ = slint::quit_event_loop();
        }
        "/clear" | "/new" => {
            sh.session.lock().unwrap().clear();
            vm.set_vec(Vec::new());
            sh.attachments.lock().unwrap().clear();
            sh.allow_all.store(false, Ordering::Relaxed);
            info(vm, "new session");
        }
        "/model" => {
            if arg.is_empty() {
                info(
                    vm,
                    &format!(
                        "model: {}\nconfig: {}",
                        sh.model.lock().unwrap(),
                        crate::config::config_path().display()
                    ),
                );
            } else {
                *sh.model.lock().unwrap() = arg.to_string();
                sh.cfg.lock().unwrap().provider.model = arg.to_string();
                match sh.cfg.lock().unwrap().save() {
                    Ok(_) => info(vm, &format!("model: {arg}")),
                    Err(e) => info(vm, &format!("model: {arg} (not saved: {e:#})")),
                }
            }
        }
        "/file" => {
            if arg.is_empty() {
                info(vm, "usage: /file <path>");
            } else {
                match files::read_attach(arg) {
                    Ok(content) => {
                        let size = content.len();
                        sh.attachments
                            .lock()
                            .unwrap()
                            .push((arg.to_string(), content));
                        info(vm, &format!("attached {arg} ({size} bytes)"));
                    }
                    Err(e) => info(vm, &format!("error: {e:#}")),
                }
            }
        }
        "/undo" | "/u" => match snapshot::undo() {
            Some(s) => info(vm, &s),
            None => info(vm, "nothing to undo"),
        },
        "/redo" => match snapshot::redo() {
            Some(s) => info(vm, &s),
            None => info(vm, "nothing to redo"),
        },
        _ => info(vm, &format!("unknown command: {cmd}, try /help")),
    }
    set_status(app, sh);
}

pub fn run(cfg: Config, provider: Arc<dyn Provider>, rt: &tokio::runtime::Runtime) -> Result<()> {
    let app = MainWindow::new()?;
    let (tx, rx) = mpsc::unbounded_channel::<ApiEvent>();

    let (client, logs) = rt.block_on(mcp::connect_all(&cfg.mcp));

    let model_name = cfg.provider.model.clone();
    let model = Rc::new(VecModel::from(Vec::new()));
    app.set_messages(ModelRc::from(model.clone()));
    for l in &logs {
        info(&model, l);
    }

    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let system = format!(
        "You are hi-derola, a coding assistant running on the user's machine.\n\
         Working directory: {cwd}\n\
         Be concise and practical. Use markdown for formatting.\n\n\
         Use the provided tools (read_file, write_file, edit, glob, grep, list_files, bash) \
         to work with files and run commands instead of printing code fences with file \
         contents. Use glob and grep to locate code before reading. \
         Prefer read_file before modifying a file. \
         write_file writes the complete file content."
    );

    let sh = Arc::new(Shared {
        session: Mutex::new(crate::chat::Session::new(system)),
        streaming: Mutex::new(None),
        reasoning: Mutex::new(None),
        confirm: Mutex::new(None),
        tokens: Mutex::new((0, 0)),
        attachments: Mutex::new(Vec::new()),
        model: Mutex::new(model_name),
        inflight: Mutex::new(None),
        allow_all: Arc::new(AtomicBool::new(false)),
        provider,
        cfg: Mutex::new(cfg),
        mcp: client,
        tx: tx.clone(),
        rt: rt.handle().clone(),
    });

    app.set_waiting(false);
    app.set_confirming(false);
    set_status(&app, &sh);

    let weak = app.as_weak();
    let shf = sh.clone();
    rt.spawn(async move {
        let mut rx = rx;
        while let Some(ev) = rx.recv().await {
            let weak = weak.clone();
            let sh = shf.clone();
            let _ = slint::invoke_from_event_loop(move || apply(&weak, &sh, ev));
        }
    });

    let weak = app.as_weak();
    let sh_s = sh.clone();
    let model_s = model.clone();
    app.on_submit(move |text| {
        let Some(app) = weak.upgrade() else {
            return;
        };
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        if text.starts_with('/') {
            command(&app, &sh_s, &model_s, &text);
            return;
        }
        let mut composed = String::new();
        {
            let mut at = sh_s.attachments.lock().unwrap();
            for (p, c) in at.iter() {
                composed.push_str(&format!("[file: {p}]\n{c}\n\n"));
            }
            at.clear();
        }
        composed.push_str(&text);
        let (system, messages) = {
            let mut ses = sh_s.session.lock().unwrap();
            ses.push(Role::User, composed);
            (ses.system.clone(), ses.messages.clone())
        };
        let model_str = sh_s.model.lock().unwrap().clone();
        model_s.push(MsgItem {
            role: "you".into(),
            text: text.into(),
        });
        app.set_waiting(true);
        set_status(&app, &sh_s);
        snapshot::begin_turn();

        let p = sh_s.cfg.lock().unwrap().provider.clone();
        let req = ChatRequest {
            system,
            messages,
            model: model_str,
            max_tokens: p.max_tokens,
            temperature: p.temperature,
            top_p: p.top_p,
            stream: p.stream,
            tools: Vec::new(),
        };
        let sh_t = sh_s.clone();
        let handle = sh_s.rt.spawn(async move {
            if let Err(e) =
                agent::run(sh_t.provider.clone(), req, sh_t.tx.clone(), sh_t.allow_all.clone(), sh_t.mcp.clone())
                    .await
            {
                let _ = sh_t.tx.send(ApiEvent::Failed(format!("{e:#}")));
            }
        });
        *sh_s.inflight.lock().unwrap() = Some(handle);
    });

    let weak = app.as_weak();
    let sh_c = sh.clone();
    let model_c = model.clone();
    app.on_confirm(move |v| {
        let Some(app) = weak.upgrade() else {
            return;
        };
        if let Some(c) = sh_c.confirm.lock().unwrap().take() {
            let _ = c.send(v);
        }
        app.set_confirming(false);
        app.set_waiting(true);
        if !v {
            info(&model_c, "denied");
        }
        set_status(&app, &sh_c);
    });

    let weak = app.as_weak();
    let sh_a = sh.clone();
    app.on_allow_all(move || {
        let Some(app) = weak.upgrade() else {
            return;
        };
        sh_a.allow_all.store(true, Ordering::Relaxed);
        if let Some(c) = sh_a.confirm.lock().unwrap().take() {
            let _ = c.send(true);
        }
        app.set_confirming(false);
        app.set_waiting(true);
        set_status(&app, &sh_a);
    });

    let weak = app.as_weak();
    let sh_st = sh.clone();
    let model_st = model.clone();
    app.on_stop(move || {
        let Some(app) = weak.upgrade() else {
            return;
        };
        if let Some(h) = sh_st.inflight.lock().unwrap().take() {
            h.abort();
        }
        if let Some(c) = sh_st.confirm.lock().unwrap().take() {
            let _ = c.send(false);
        }
        *sh_st.streaming.lock().unwrap() = None;
        *sh_st.reasoning.lock().unwrap() = None;
        app.set_waiting(false);
        app.set_confirming(false);
        info(&model_st, "cancelled");
        snapshot::end_turn();
        set_status(&app, &sh_st);
    });

    app.run()?;
    Ok(())
}
