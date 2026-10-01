use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

use crate::chat::{Message, Role};
use crate::mcp::McpClient;
use crate::provider::{ApiEvent, ChatRequest, Provider};
use crate::tools;

pub struct AgentCfg {
    pub context_limit: u64,
    pub max_rounds: usize,
    pub output_budget: usize,
    pub perm: crate::perm::PermCfg,
}

impl Default for AgentCfg {
    fn default() -> Self {
        Self {
            context_limit: 0,
            max_rounds: 15,
            output_budget: 32 * 1024,
            perm: Default::default(),
        }
    }
}

const COMPACT_KEEP: usize = 6;
const COMPACT_MIN_MSGS: usize = 8;
const EMPTY_RETRIES: usize = 2;
const OVERFLOW_RETRIES: usize = 2;

fn effective_limit(cfg: &AgentCfg, model: &str) -> u64 {
    if cfg.context_limit > 0 {
        return cfg.context_limit;
    }
    let mi = crate::models::lookup(model);
    if mi.window > 0 {
        mi.window / 10 * 9
    } else {
        0
    }
}

pub async fn run(
    provider: Arc<dyn Provider>,
    mut req: ChatRequest,
    tx: UnboundedSender<ApiEvent>,
    allow_all: Arc<AtomicBool>,
    mcp: Option<Arc<McpClient>>,
    queue: Arc<Mutex<Vec<String>>>,
    cfg: AgentCfg,
) -> Result<()> {
    let mut specs = tools::specs();
    if let Some(m) = &mcp {
        specs.extend(m.specs().await);
    }
    req.tools = specs;
    let ctx_limit = effective_limit(&cfg, &req.model);
    let mut msgs = req.messages.clone();
    let mut round = 0;
    let mut empty_retries = 0usize;
    let mut overflow_retries = 0usize;
    let mut used: u64 = 0;
    loop {
        round += 1;
        if round > cfg.max_rounds {
            return wrap_up(provider, &mut req, &mut msgs, &tx, cfg.max_rounds).await;
        }
        let est = est_tokens(&msgs) as u64;
        if ctx_limit > 0 && used.max(est) * 4 > ctx_limit * 3 {
            if compact(&provider, &req, &mut msgs, &tx).await {
                used = 0;
                req.messages = msgs.clone();
            }
        }
        let mut steered = false;
        for text in queue.lock().unwrap().drain(..) {
            let head: String = text.chars().take(60).collect();
            let _ = tx.send(ApiEvent::Note(format!("steer: {head}")));
            msgs.push(Message::new(Role::User, text));
            steered = true;
        }
        if steered {
            req.messages = msgs.clone();
        }
        let (itx, mut irx) = tokio::sync::mpsc::unbounded_channel();
        let counter = Arc::new(AtomicU64::new(0));
        let outer = tx.clone();
        let c2 = counter.clone();
        let fwd = tokio::spawn(async move {
            while let Some(ev) = irx.recv().await {
                if let ApiEvent::Usage { input, .. } = ev {
                    c2.fetch_max(input, Ordering::Relaxed);
                }
                let _ = outer.send(ev);
            }
        });
        let outcome = provider.chat(&req, &itx).await;
        drop(itx);
        let _ = fwd.await;
        let reply = match outcome {
            Ok(r) => r,
            Err(e) => {
                if overflow_retries < OVERFLOW_RETRIES && is_overflow(&e) {
                    overflow_retries += 1;
                    let _ = tx.send(ApiEvent::Note(format!(
                        "context overflow, compacting and retrying ({overflow_retries}/{OVERFLOW_RETRIES})"
                    )));
                    if compact(&provider, &req, &mut msgs, &tx).await {
                        req.messages = msgs.clone();
                        round -= 1;
                        continue;
                    }
                }
                return Err(e);
            }
        };
        overflow_retries = 0;
        let observed = counter.load(Ordering::Relaxed);
        used = if observed == 0 { est } else { observed };
        if reply.text.trim().is_empty() && reply.calls.is_empty() {
            empty_retries += 1;
            if empty_retries <= EMPTY_RETRIES {
                let _ = tx.send(ApiEvent::Note(format!(
                    "empty response, retrying ({empty_retries}/{EMPTY_RETRIES})"
                )));
                round -= 1;
                continue;
            }
        }
        if reply.calls.is_empty() {
            msgs.push(Message::new(Role::Assistant, reply.text.clone()));
            let _ = tx.send(ApiEvent::Done {
                text: reply.text,
                messages: msgs,
            });
            return Ok(());
        }
        empty_retries = 0;
        msgs.push(Message::new(Role::Assistant, reply.text.clone()).with_calls(reply.calls.clone()));
        req.messages = msgs.clone();
        for call in reply.calls {
            tx.send(ApiEvent::Tool {
                name: call.name.clone(),
                detail: tools::detail(&call.name, &call.args),
                diff: tools::preview(&call.name, &call.args),
            })
            .map_err(|_| anyhow!("closed"))?;
            match cfg.perm.check(&call.name, &call.args) {
                crate::perm::Perm::Deny => {
                    let _ = tx.send(ApiEvent::Note(format!(
                        "{} denied by permissions config",
                        call.name
                    )));
                    msgs.push(Message::tool(&call.id, "denied by permissions config"));
                    req.messages = msgs.clone();
                    continue;
                }
                crate::perm::Perm::Allow => {}
                crate::perm::Perm::Ask => {
                    if !allow_all.load(Ordering::Relaxed) {
                        let (otx, orx) = oneshot::channel();
                        tx.send(ApiEvent::Confirm {
                            name: call.name.clone(),
                            args: call.args.clone(),
                            rx: otx,
                        })
                        .map_err(|_| anyhow!("closed"))?;
                        if !orx.await.unwrap_or(false) {
                            msgs.push(Message::tool(&call.id, "user denied this action"));
                            req.messages = msgs.clone();
                            continue;
                        }
                    }
                }
            }
            let out = if call.name == "question" {
                match ask_user(&call.args, &tx).await {
                    Ok(a) => a,
                    Err(e) => format!("error: {e:#}"),
                }
            } else {
                match tools::execute(&call.name, &call.args, mcp.as_deref()).await {
                    Ok(o) => o,
                    Err(e) => format!("error: {e:#}"),
                }
            };
            let out = tools::budget(out, cfg.output_budget);
            let summary: String = out
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(70)
                .collect();
            let _ = tx.send(ApiEvent::Note(summary));
            msgs.push(Message::tool(&call.id, out));
            req.messages = msgs.clone();
        }
    }
}

async fn ask_user(args: &str, tx: &UnboundedSender<ApiEvent>) -> Result<String> {
    let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
    let qs = v["questions"].as_array().cloned().unwrap_or_default();
    if qs.is_empty() {
        bail!("question: no questions given");
    }
    let (otx, orx) = oneshot::channel();
    tx.send(ApiEvent::Ask {
        name: "question".into(),
        args: args.to_string(),
        rx: otx,
    })
    .map_err(|_| anyhow!("closed"))?;
    let answer = orx.await.unwrap_or_default();
    if answer.trim().is_empty() {
        return Ok("The user dismissed this question.".into());
    }
    Ok(format!(
        "User has answered your questions: {}. You can now continue with the user's answers in mind.",
        answer.trim()
    ))
}

async fn wrap_up(
    provider: Arc<dyn Provider>,
    req: &mut ChatRequest,
    msgs: &mut Vec<Message>,
    tx: &UnboundedSender<ApiEvent>,
    max_rounds: usize,
) -> Result<()> {
    let _ = tx.send(ApiEvent::Note(format!(
        "tool loop exceeded {max_rounds} rounds, asking the model to wrap up"
    )));
    msgs.push(Message::new(
        Role::User,
        "The tool call limit has been reached. Stop making changes and write a short final summary: what was done, what was changed, what remains.",
    ));
    req.tools = Vec::new();
    req.messages = msgs.clone();
    let text = match provider.chat(req, tx).await {
        Ok(r) if !r.text.trim().is_empty() => r.text,
        Ok(_) => "stopped: tool loop limit reached".into(),
        Err(e) => {
            let _ = tx.send(ApiEvent::Note(format!("wrap-up failed: {e:#}")));
            "stopped: tool loop limit reached".into()
        }
    };
    msgs.push(Message::new(Role::Assistant, text.clone()));
    let _ = tx.send(ApiEvent::Done {
        text,
        messages: msgs.clone(),
    });
    Ok(())
}

fn compact_split(msgs: &[Message], keep_last: usize) -> Option<usize> {
    if msgs.len() < COMPACT_MIN_MSGS || msgs.first()?.role != Role::User {
        return None;
    }
    let mut cut = msgs.len().saturating_sub(keep_last).max(1);
    while cut < msgs.len() && msgs[cut].role == Role::Tool {
        cut += 1;
    }
    if cut <= 1 || cut >= msgs.len() {
        return None;
    }
    Some(cut)
}

fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end < s.len() && !s.is_char_boundary(end) {
        end += 1;
    }
    let mut out = s[..end].to_string();
    out.push_str("...");
    out
}

fn transcript(msgs: &[Message]) -> String {
    let mut t = String::new();
    for m in msgs {
        match m.role {
            Role::User => t.push_str(&format!("[user]\n{}\n\n", clip(&m.content, 4000))),
            Role::Assistant => {
                if !m.content.trim().is_empty() {
                    t.push_str(&format!("[assistant]\n{}\n\n", clip(&m.content, 4000)));
                }
                for c in &m.tool_calls {
                    t.push_str(&format!("[assistant called tool] {} {}\n\n", c.name, clip(&c.args, 300)));
                }
            }
            Role::Tool => t.push_str(&format!("[tool result]\n{}\n\n", clip(&m.content, 2000))),
        }
    }
    t
}

async fn compact(
    provider: &Arc<dyn Provider>,
    req: &ChatRequest,
    msgs: &mut Vec<Message>,
    tx: &UnboundedSender<ApiEvent>,
) -> bool {
    let Some(cut) = compact_split(msgs, COMPACT_KEEP) else {
        return false;
    };
    let text = transcript(&msgs[1..cut]);
    let kept = msgs.len() - cut;
    let sum_req = ChatRequest {
        system: "You summarize coding agent conversations. Produce a dense factual summary: the user's goal, what was done (files changed, commands run, results), key decisions, current state, next steps. Plain text, no markdown headers, under 500 words.".into(),
        messages: vec![Message::new(
            Role::User,
            format!("Conversation transcript:\n\n{text}\n\nWrite the summary now."),
        )],
        model: req.model.clone(),
        max_tokens: req.max_tokens,
        temperature: req.temperature,
        top_p: req.top_p,
        stream: false,
        tools: Vec::new(),
    };
    let (btx, _brx) = tokio::sync::mpsc::unbounded_channel();
    let summary = match provider.chat(&sum_req, &btx).await {
        Ok(r) if !r.text.trim().is_empty() => r.text.trim().to_string(),
        Ok(_) => {
            let _ = tx.send(ApiEvent::Note("compaction failed: empty summary".into()));
            return false;
        }
        Err(e) => {
            let _ = tx.send(ApiEvent::Note(format!("compaction failed: {e:#}")));
            return false;
        }
    };
    let mut first = msgs[0].clone();
    first
        .content
        .push_str("\n\n---\nSummary of the earlier conversation (dropped to fit the context window):\n");
    first.content.push_str(&summary);
    msgs.drain(1..cut);
    msgs[0] = first;
    let _ = tx.send(ApiEvent::Note(format!(
        "context compacted: {} messages summarized, {} kept",
        cut - 1,
        kept
    )));
    true
}

fn est_tokens(msgs: &[Message]) -> usize {
    let chars: usize = msgs
        .iter()
        .map(|m| {
            m.content.len()
                + m.tool_calls
                    .iter()
                    .map(|c| c.args.len() + c.name.len())
                    .sum::<usize>()
        })
        .sum();
    chars / 4
}

fn is_overflow(e: &anyhow::Error) -> bool {
    let s = format!("{e:#}").to_lowercase();
    [
        "context length",
        "maximum context",
        "context_length_exceeded",
        "context window",
        "prompt is too long",
        "too many tokens",
        "input length exceeds",
        "reduce the length",
    ]
    .iter()
    .any(|m| s.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(role: Role, text: &str) -> Message {
        Message::new(role, text)
    }

    #[test]
    fn split_and_overflow() {
        let msgs = vec![
            m(Role::User, "task"),
            m(Role::Assistant, "a1"),
            m(Role::Tool, "t1"),
            m(Role::Assistant, "a2"),
            m(Role::User, "u2"),
            m(Role::Assistant, "a3"),
        ];
        assert_eq!(compact_split(&msgs, 6), None);
        let long: Vec<Message> = std::iter::once(m(Role::User, "task"))
            .chain((0..12).map(|i| m(Role::Assistant, &format!("a{i}"))))
            .collect();
        assert_eq!(compact_split(&long, 6), Some(7));
        let orphan: Vec<Message> = std::iter::once(m(Role::User, "task"))
            .chain((1..7).map(|i| m(Role::Assistant, &format!("a{i}"))))
            .chain(std::iter::once(m(Role::Tool, "t")))
            .chain((7..11).map(|i| m(Role::Assistant, &format!("a{i}"))))
            .collect();
        let cut = compact_split(&orphan, 6).unwrap();
        assert_eq!(orphan[cut].role, Role::Assistant);
        let e = anyhow::anyhow!("400 Bad Request: prompt is too long: 200000 tokens > 180000 maximum");
        assert!(is_overflow(&e));
        assert!(!is_overflow(&anyhow::anyhow!("401 unauthorized")));
    }

    #[test]
    fn clip_and_est() {
        assert_eq!(clip("hello", 10), "hello");
        assert!(clip("0123456789abcdef", 8).ends_with("..."));
        assert_eq!(est_tokens(&[m(Role::User, "x")]), 0);
        assert_eq!(est_tokens(&[m(Role::User, "abcd")]), 1);
    }
}
