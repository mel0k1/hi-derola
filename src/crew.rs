//! crew: one goal, several ai participants talking in a single session.
//!
//! a crew is a stored transcript (json per crew under the sessions dir
//! sibling `crew/`) plus a tiny round-based runner: every round each member
//! gets the transcript and answers with its role in mind; @Name steers who
//! speaks next, a "DONE:" line ends the crew, "RENAME:" lets a placeholder
//! pick its own callsign. members ride any configured provider profile, so
//! one crew can mix apis and models. replies stream live to the frontends
//! (CrewChunk), tool-enabled members ("tools" flag) may run bash/file tools
//! inside their rounds under the permission config, and per-member token
//! limits ("limit=N") auto-stop members (and the crew once all are spent).
//! usage rows are recorded per member — the `/crew usage` report splits
//! tokens and cost by participant.

use crate::chat::Message;
use crate::provider::{ApiEvent, ChatRequest};
use crate::usage::UsageRow;
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Member {
    pub name: String,
    pub role: String,
    /// [profiles.<name>] preset; None = the base [provider] section
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// token budget (input+output) for this member; auto-stops them when hit
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    /// member may use file/shell tools inside its rounds (under permissions)
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tools: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrewMsg {
    /// member name, "you" for the user, "crew" for system notes
    pub author: String,
    /// "user" | "agent" | "system"
    pub kind: String,
    pub content: String,
    pub ts: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Crew {
    pub id: String,
    pub goal: String,
    /// idle | running | done
    pub status: String,
    pub members: Vec<Member>,
    pub messages: Vec<CrewMsg>,
    #[serde(default)]
    pub usage: BTreeMap<String, Vec<UsageRow>>,
    pub created: u64,
    pub updated: u64,
}

static ACTIVE: Mutex<Option<String>> = Mutex::new(None);
static RUNNING: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);

pub fn active() -> Option<String> {
    ACTIVE.lock().unwrap().clone()
}

pub fn set_active(id: Option<String>) {
    *ACTIVE.lock().unwrap() = id;
}

pub fn is_running() -> bool {
    RUNNING.load(Ordering::Relaxed)
}

pub fn cancel() {
    CANCEL.store(true, Ordering::Relaxed);
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn store_dir() -> PathBuf {
    if let Ok(d) = std::env::var("HI_DEROLA_CREW_DIR") {
        if !d.trim().is_empty() {
            return PathBuf::from(d);
        }
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("hi-derola")
        .join("crew")
}

fn file_of(id: &str) -> PathBuf {
    store_dir().join(format!("{id}.json"))
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn save(crew: &Crew) -> Result<()> {
    if !safe_id(&crew.id) {
        bail!("bad crew id");
    }
    let dir = store_dir();
    std::fs::create_dir_all(&dir).context("create crew dir")?;
    let raw = serde_json::to_string_pretty(crew)?;
    let tmp = file_of(&crew.id).with_extension("tmp");
    std::fs::write(&tmp, raw)?;
    std::fs::rename(&tmp, file_of(&crew.id))?;
    Ok(())
}

pub fn load(id: &str) -> Result<Crew> {
    if !safe_id(id) {
        bail!("bad crew id");
    }
    let raw = std::fs::read_to_string(file_of(id)).context("read crew")?;
    serde_json::from_str(&raw).context("parse crew")
}

pub fn delete(id: &str) -> Result<()> {
    if !safe_id(id) {
        bail!("bad crew id");
    }
    match std::fs::remove_file(file_of(id)) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).context("delete crew"),
    }
}

pub struct CrewMeta {
    pub id: String,
    pub goal: String,
    pub status: String,
    pub members: usize,
    pub messages: usize,
    pub updated: u64,
    /// top mention edges author→target (the resume-list mini graph)
    pub mentions: Vec<(String, String, usize)>,
}

pub fn list() -> Vec<CrewMeta> {
    let dir = store_dir();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&p) else {
            continue;
        };
        let Ok(c) = serde_json::from_str::<Crew>(&raw) else {
            continue;
        };
        out.push(CrewMeta {
            mentions: mention_graph(&c).into_iter().take(6).collect(),
            id: c.id,
            goal: c.goal,
            status: c.status,
            members: c.members.len(),
            messages: c.messages.len(),
            updated: c.updated,
        });
    }
    out.sort_by_key(|c| std::cmp::Reverse(c.updated));
    out
}

/// "Name|role[|profile|model[|flags]]" — a leading "|role" makes a placeholder
/// name; flags (any order): "tools" gives the member file/shell tools,
/// "limit=<n>" caps its tokens (20k / 1.5m suffixes work, "off"/0 clears)
pub fn parse_member(spec: &str, taken: &[Member]) -> Result<Member> {
    let parts: Vec<&str> = spec.split('|').map(str::trim).collect();
    if parts.is_empty() || parts.iter().all(|p| p.is_empty()) {
        bail!("member spec is empty — use Name|role or |role");
    }
    let (name_part, rest) = match parts[0].is_empty() {
        false => (Some(parts[0]), &parts[1..]),
        true => (None, &parts[1..]),
    };
    let role = rest.first().copied().unwrap_or("engineer").to_string();
    if role.is_empty() {
        bail!("member role is empty");
    }
    let mut positional: Vec<Option<String>> = Vec::new();
    let mut limit = None;
    let mut tools = false;
    for part in rest.iter().skip(1) {
        let lower = part.to_lowercase();
        if lower == "tools" {
            tools = true;
        } else if let Some(v) = lower.strip_prefix("limit=") {
            limit = parse_limit(v)?;
        } else if !lower.is_empty() {
            positional.push(Some(part.to_string()));
        }
    }
    let profile = positional.first().cloned().flatten();
    let model = positional.get(1).cloned().flatten();
    let name = match name_part {
        Some(n) => n.to_string(),
        None => {
            let mut i = taken.len() + 1;
            loop {
                let cand = format!("agent-{i}");
                if !taken.iter().any(|m| m.name == cand) {
                    break cand;
                }
                i += 1;
            }
        }
    };
    if name.len() > 32 {
        bail!("member name is too long (max 32 chars)");
    }
    Ok(Member {
        name,
        role,
        profile,
        model,
        limit,
        tools,
    })
}

/// "20k" / "1.5m" / "500000"; "off" or 0 clears the limit
pub fn parse_limit_opt(v: &str) -> Option<u64> {
    parse_limit(v).ok().flatten()
}

fn parse_limit(v: &str) -> Result<Option<u64>> {
    let v = v.trim();
    if v.is_empty() || v == "off" || v == "0" {
        return Ok(None);
    }
    let (num, mult) = match v.chars().last() {
        Some('k') | Some('K') => (&v[..v.len() - 1], 1_000u64),
        Some('m') | Some('M') => (&v[..v.len() - 1], 1_000_000),
        _ => (v, 1),
    };
    let n: f64 = num
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("bad limit \"{v}\" — use 200000, 200k or off"))?;
    if n < 0.0 {
        bail!("limit must be positive");
    }
    Ok(Some((n * mult as f64) as u64))
}

/// change a member's token budget; None clears it
pub fn set_limit(crew_id: &str, name: &str, limit: Option<u64>) -> Result<Crew> {
    let mut crew = load(crew_id)?;
    let m = crew
        .members
        .iter_mut()
        .find(|m| m.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| anyhow::anyhow!("no member named \"{name}\""))?;
    m.limit = limit;
    crew.updated = now();
    save(&crew)?;
    Ok(crew)
}

/// total tokens a member has burned so far (input + output, cached included)
pub fn member_used(crew: &Crew, name: &str) -> u64 {
    crew.usage
        .get(name)
        .map(|rows| rows.iter().map(|r| r.input + r.output).sum())
        .unwrap_or(0)
}

pub fn create(goal: &str, specs: &[String]) -> Result<Crew> {
    let goal = goal.trim();
    if goal.is_empty() {
        bail!("a crew needs a goal");
    }
    let mut members = Vec::new();
    for s in specs {
        let m = parse_member(s, &members)?;
        members.push(m);
    }
    if members.is_empty() {
        bail!("a crew needs at least one member — /crew add Name|role");
    }
    let crew = Crew {
        id: crate::sessions::new_id(),
        goal: goal.to_string(),
        status: "idle".into(),
        members,
        messages: vec![CrewMsg {
            author: "crew".into(),
            kind: "system".into(),
            content: format!("crew goal: {goal}"),
            ts: now(),
        }],
        usage: BTreeMap::new(),
        created: now(),
        updated: now(),
    };
    save(&crew)?;
    set_active(Some(crew.id.clone()));
    Ok(crew)
}

fn crew_system_prompt(crew: &Crew, member: &Member) -> String {
    let peers: Vec<String> = crew
        .members
        .iter()
        .filter(|m| m.name != member.name)
        .map(|m| format!("{} ({})", m.name, m.role))
        .collect();
    let placeholder = member.name.starts_with("agent-");
    let mut s = format!(
        "You are {name}, a member of a crew of AI agents working toward one shared goal. \
         Your role: {role}.\n\
         <goal>\n{goal}\n</goal>\n",
        name = member.name,
        role = member.role,
        goal = crew.goal,
    );
    if !peers.is_empty() {
        s.push_str(&format!("\nOther members: {}.\n", peers.join(", ")));
    }
    s.push_str(
        "\nProtocol:\n\
         - One reply per turn; keep it short and substantive, in the user's language.\n\
         - Address a specific member with @Name when the next step is theirs.\n\
         - Coordinate, plan, review, split work.\n\
         - When the shared goal is fully achieved, end the reply with a line \"DONE: <short summary>\".\n",
    );
    if member.tools {
        s.push_str(
            "- You have file and shell tools (bash, read_file, write_file, edit, glob, grep). \
                 Use them to actually do your part of the work; each tool run may ask the user \
                 for permission. Keep the final reply a short report of what you did.\n",
        );
    } else {
        s.push_str("- The crew chat has no file or shell tools for you.\n");
    }
    if placeholder {
        s.push_str(
            "- Your name is a placeholder. If you want your own callsign, start this first \
             reply with a line \"RENAME: <name>\" (max 24 chars, no spaces needed) and use it \
             afterwards.\n",
        );
    }
    let skills = crate::skills::spec_description();
    if !skills.is_empty() {
        s.push_str(&format!(
            "\nSkills the crew members can load in their own sessions (context only):\n{skills}\n"
        ));
    }
    s
}

fn transcript_messages(crew: &Crew, max: usize) -> Vec<Message> {
    let skip = crew.messages.len().saturating_sub(max);
    crew.messages[skip..]
        .iter()
        .map(|m| {
            let who = match m.kind.as_str() {
                "user" => m.author.clone(),
                "system" => m.author.clone(),
                _ => m.author.clone(),
            };
            Message::new(crate::chat::Role::User, format!("{who}: {}", m.content))
        })
        .collect()
}

fn cost_of(kind: &str, model: &str, input: u64, output: u64, cached: u64) -> f64 {
    let disc = if kind == "anthropic" { 0.1 } else { 0.5 };
    crate::models::cost_cached(model, input, output, cached, disc)
}

/// run `rounds` rounds of the crew: each round every member answers once;
/// a reply containing a DONE line finishes the whole crew early. Usage is
/// attributed to the member whose provider call produced it.
pub async fn step(
    crew_id: &str,
    rounds: usize,
    cfg: &crate::config::Config,
    tx: &UnboundedSender<ApiEvent>,
) -> Result<String> {
    if RUNNING.swap(true, Ordering::Relaxed) {
        bail!("a crew is already running — /crew stop first");
    }
    CANCEL.store(false, Ordering::Relaxed);
    let out = step_inner(crew_id, rounds, cfg, tx).await;
    RUNNING.store(false, Ordering::Relaxed);
    out
}

async fn step_inner(
    crew_id: &str,
    rounds: usize,
    cfg: &crate::config::Config,
    tx: &UnboundedSender<ApiEvent>,
) -> Result<String> {
    let mut crew = load(crew_id)?;
    if crew.status == "done" {
        bail!("crew is already done — /crew new to start another");
    }
    crew.status = "running".into();
    crew.updated = now();
    save(&crew)?;

    let mut note = String::new();
    let mut done = false;
    'rounds: for _ in 0..rounds.max(1) {
        let order: Vec<String> = crew.members.iter().map(|m| m.name.clone()).collect();
        let mut queue = order.clone();
        let mut skipped_all = true;
        while !queue.is_empty() {
            if CANCEL.load(Ordering::Relaxed) {
                break 'rounds;
            }
            let name = queue.remove(0);
            let Some(member) = crew.members.iter().find(|m| m.name == name).cloned() else {
                continue;
            };
            // token budget auto-stop: over-limit members sit the round out
            let used = member_used(&crew, &name);
            if let Some(limit) = member.limit {
                if limit > 0 && used >= limit {
                    let msg = format!("{name}: token limit reached ({used} ≥ {limit}) — skipped");
                    crew.messages.push(CrewMsg {
                        author: "crew".into(),
                        kind: "system".into(),
                        content: msg.clone(),
                        ts: now(),
                    });
                    let _ = tx.send(ApiEvent::Note(msg));
                    continue;
                }
            }
            skipped_all = false;
            let reply = match ask_member(&crew, &member, cfg, tx).await {
                Ok((text, rows)) => {
                    if !rows.is_empty() {
                        crew.usage
                            .entry(member.name.clone())
                            .or_default()
                            .extend(rows);
                        if let Some(limit) = member.limit {
                            if limit > 0 && member_used(&crew, &name) >= limit {
                                let msg =
                                    format!("{name}: token limit reached ({limit}) — auto-stopped");
                                crew.messages.push(CrewMsg {
                                    author: "crew".into(),
                                    kind: "system".into(),
                                    content: msg.clone(),
                                    ts: now(),
                                });
                                let _ = tx.send(ApiEvent::Note(msg));
                            }
                        }
                    }
                    text
                }
                Err(e) => {
                    let msg = format!("{name}: error — {e:#}");
                    crew.messages.push(CrewMsg {
                        author: "crew".into(),
                        kind: "system".into(),
                        content: msg.clone(),
                        ts: now(),
                    });
                    let _ = tx.send(ApiEvent::Note(msg));
                    continue;
                }
            };
            // self-naming for placeholder members
            if let Some(new_name) = extract_line(&reply, "RENAME:") {
                let new_name = sanitize_name(&new_name);
                if !new_name.is_empty()
                    && new_name != member.name
                    && !crew.members.iter().any(|m| m.name == new_name)
                {
                    let rows = crew.usage.remove(&member.name).unwrap_or_default();
                    crew.usage.insert(new_name.clone(), rows);
                    for m in crew.members.iter_mut() {
                        if m.name == member.name {
                            m.name = new_name.clone();
                        }
                    }
                    crew.messages.push(CrewMsg {
                        author: "crew".into(),
                        kind: "system".into(),
                        content: format!("{} is now known as {new_name}", member.name),
                        ts: now(),
                    });
                    let _ = tx.send(ApiEvent::Note(format!(
                        "crew: {} renamed to {new_name}",
                        member.name
                    )));
                }
            }
            let author = crew
                .members
                .iter()
                .find(|m| {
                    m.role == member.role && m.model == member.model && m.profile == member.profile
                })
                .map(|m| m.name.clone())
                .unwrap_or_else(|| member.name.clone());
            crew.messages.push(CrewMsg {
                author: author.clone(),
                kind: "agent".into(),
                content: reply.clone(),
                ts: now(),
            });
            crew.updated = now();
            let _ = tx.send(ApiEvent::Crew {
                id: crew.id.clone(),
                author: author.clone(),
                role: member.role.clone(),
                content: reply.clone(),
            });
            if extract_line(&reply, "DONE:").is_some() {
                crew.status = "done".into();
                done = true;
                save(&crew)?;
                break 'rounds;
            }
            // @mention steering: the named member speaks next
            if let Some(target) = first_mention(&reply) {
                if let Some(pos) = queue.iter().position(|n| n.eq_ignore_ascii_case(&target)) {
                    let picked = queue.remove(pos);
                    queue.insert(0, picked);
                }
            }
        }
        // every member is over its budget — stop the crew instead of spinning
        if skipped_all {
            note.push_str(" (all members hit their token limits)");
            break;
        }
    }
    if !done {
        crew.status = "idle".into();
    }
    crew.updated = now();
    save(&crew)?;
    if CANCEL.load(Ordering::Relaxed) {
        note.push_str(" (cancelled)");
    }
    Ok(format!(
        "crew step finished — status: {}{note}",
        crew.status
    ))
}

/// the tool subset a tool-enabled crew member may call
pub fn crew_tool_specs() -> Vec<crate::provider::ToolSpec> {
    crate::tools::specs()
        .into_iter()
        .filter(|s| {
            matches!(
                s.name.as_str(),
                "bash" | "read_file" | "write_file" | "edit" | "list_files" | "glob" | "grep"
            )
        })
        .collect()
}

fn member_request(
    crew: &Crew,
    member: &Member,
    cfg: &crate::config::Config,
) -> Result<(
    std::sync::Arc<dyn crate::provider::Provider>,
    ChatRequest,
    crate::config::ProviderConfig,
)> {
    let pc = cfg.provider_for(member.profile.as_deref(), member.model.as_deref());
    let api_key = pc.api_key.clone().or_else(|| cfg.api_key());
    let Some(api_key) = api_key.filter(|k| !k.trim().is_empty()) else {
        bail!(
            "no api key for member \"{}\" (profile {:?}) — settings > provider",
            member.name,
            member.profile
        );
    };
    let provider = crate::provider::build(&pc.kind, pc.base_url.clone(), api_key)?;
    let tools = if member.tools {
        crew_tool_specs()
    } else {
        Vec::new()
    };
    let req = ChatRequest {
        system: crew_system_prompt(crew, member),
        messages: transcript_messages(crew, 40),
        model: pc.model.clone(),
        max_tokens: pc.max_tokens,
        temperature: pc.temperature,
        top_p: pc.top_p,
        stream: true,
        tools,
    };
    Ok((provider, req, pc))
}

/// one provider turn for one member: streams deltas out as CrewChunk
/// events, returns the reply text, tool calls and usage rows
async fn member_call(
    provider: &dyn crate::provider::Provider,
    req: &ChatRequest,
    crew_id: &str,
    name: &str,
    model: &str,
    kind: &str,
    tx: &UnboundedSender<ApiEvent>,
) -> Result<(String, Vec<crate::chat::ToolCall>, Vec<UsageRow>)> {
    let (etx, mut erx) = tokio::sync::mpsc::unbounded_channel();
    let rows: std::sync::Arc<Mutex<Vec<UsageRow>>> = Default::default();
    let fwd = {
        let tx = tx.clone();
        let crew_id = crew_id.to_string();
        let name = name.to_string();
        let model = model.to_string();
        let kind = kind.to_string();
        let rows = rows.clone();
        tokio::spawn(async move {
            while let Some(ev) = erx.recv().await {
                match ev {
                    ApiEvent::Chunk(delta) => {
                        let _ = tx.send(ApiEvent::CrewChunk {
                            id: crew_id.clone(),
                            author: name.clone(),
                            delta,
                        });
                    }
                    ApiEvent::Usage {
                        input,
                        output,
                        cached,
                    } => {
                        let cost = cost_of(&kind, &model, input, output, cached);
                        rows.lock().unwrap().push(UsageRow {
                            model: model.clone(),
                            input,
                            output,
                            cached,
                            cost,
                        });
                    }
                    _ => {}
                }
            }
        })
    };
    let reply = provider.chat(req, &etx).await;
    drop(etx);
    let _ = fwd.await;
    let reply = reply?;
    let rows = std::mem::take(&mut *rows.lock().unwrap());
    Ok((reply.text, reply.calls, rows))
}

/// one member's turn: tool-less members stream a single call, tool-enabled
/// ones run a short agentic loop where every call goes through the
/// permission config (Confirm events ride the same channel)
async fn ask_member(
    crew: &Crew,
    member: &Member,
    cfg: &crate::config::Config,
    tx: &UnboundedSender<ApiEvent>,
) -> Result<(String, Vec<UsageRow>)> {
    let (provider, mut req, pc) = member_request(crew, member, cfg)?;
    if !member.tools {
        let (text, _calls, rows) = member_call(
            provider.as_ref(),
            &req,
            &crew.id,
            &member.name,
            &pc.model,
            &pc.kind,
            tx,
        )
        .await?;
        return Ok((text, rows));
    }

    const MAX_TOOL_ROUNDS: usize = 8;
    let mut perm = cfg.permissions.clone();
    let mut msgs = req.messages.clone();
    let mut all_rows: Vec<UsageRow> = Vec::new();
    let mut text = String::new();
    for round in 0..=MAX_TOOL_ROUNDS {
        req.messages = msgs.clone();
        let (t, calls, rows) = member_call(
            provider.as_ref(),
            &req,
            &crew.id,
            &member.name,
            &pc.model,
            &pc.kind,
            tx,
        )
        .await?;
        all_rows.extend(rows);
        if calls.is_empty() || round == MAX_TOOL_ROUNDS {
            text = t;
            break;
        }
        msgs.push(Message::new(crate::chat::Role::Assistant, t).with_calls(calls.clone()));
        for call in calls {
            let _ = tx.send(ApiEvent::Tool {
                name: format!("{} (crew {})", call.name, member.name),
                detail: crate::tools::detail(&call.name, &call.args),
                diff: crate::tools::preview(&call.name, &call.args),
                paths: crate::tools::paths(&call.name, &call.args),
            });
            // bash routed into an attached VM is gated as the "sandbox" perm
            let perm_tool = if call.name == "bash" && crate::sandbox::shell_route().is_some() {
                "sandbox"
            } else {
                call.name.as_str()
            };
            let tool_msg = |content: String| Message::tool(&call.id, content);
            match perm.check(perm_tool, &call.args) {
                crate::perm::Perm::Deny => {
                    msgs.push(tool_msg("denied by permissions config".into()));
                }
                crate::perm::Perm::Ask => {
                    let (otx, orx) = tokio::sync::oneshot::channel();
                    let _ = tx.send(ApiEvent::Confirm {
                        name: call.name.clone(),
                        args: call.args.clone(),
                        rx: otx,
                    });
                    let r = orx.await.unwrap_or_default();
                    if !r.approved {
                        let why = if r.feedback.is_empty() {
                            "denied by the user".to_string()
                        } else {
                            format!("denied by the user: {}", r.feedback)
                        };
                        msgs.push(tool_msg(why));
                        continue;
                    }
                    if r.always {
                        if let Some(rule) = crate::perm::derive_rule(perm_tool, &call.args) {
                            perm.rules.push(rule);
                        }
                    }
                    let out = crate::tools::execute(&call.name, &call.args, None).await;
                    msgs.push(tool_msg(match out {
                        Ok(o) => o,
                        Err(e) => format!("error: {e:#}"),
                    }));
                }
                crate::perm::Perm::Allow => {
                    let out = crate::tools::execute(&call.name, &call.args, None).await;
                    msgs.push(tool_msg(match out {
                        Ok(o) => o,
                        Err(e) => format!("error: {e:#}"),
                    }));
                }
            }
        }
    }
    Ok((text, all_rows))
}

fn extract_line(text: &str, prefix: &str) -> Option<String> {
    text.lines()
        .find_map(|l| l.trim().strip_prefix(prefix))
        .map(|s| s.trim().to_string())
}

fn sanitize_name(s: &str) -> String {
    let t: String = s
        .trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == '.' || c == ',')
        .chars()
        .take(24)
        .collect();
    t.trim().to_string()
}

fn trunc(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max - 1).collect();
        format!("{cut}…")
    }
}

fn first_mention(text: &str) -> Option<String> {
    let at = text.find('@')?;
    let rest = &text[at + 1..];
    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// every @mention in one message, resolved against member names (+ "you")
fn mentions_in(text: &str, known: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find('@') {
        rest = &rest[at + 1..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
            .collect();
        if name.is_empty() {
            continue;
        }
        if let Some(k) = known.iter().find(|k| k.eq_ignore_ascii_case(&name)) {
            out.push(k.clone());
        }
    }
    out
}

/// author→target mention edge counts for the graph view; "you" is the user
pub fn mention_graph(crew: &Crew) -> Vec<(String, String, usize)> {
    let mut known: Vec<String> = crew.members.iter().map(|m| m.name.clone()).collect();
    known.push("you".into());
    let mut edges: BTreeMap<(String, String), usize> = BTreeMap::new();
    for m in &crew.messages {
        if m.kind == "system" {
            continue;
        }
        for target in mentions_in(&m.content, &known) {
            if target == m.author {
                continue;
            }
            *edges.entry((m.author.clone(), target)).or_default() += 1;
        }
    }
    let mut out: Vec<(String, String, usize)> =
        edges.into_iter().map(|((a, b), n)| (a, b, n)).collect();
    out.sort_by_key(|(_, _, n)| std::cmp::Reverse(*n));
    out
}

pub fn send(crew_id: &str, text: &str) -> Result<()> {
    let mut crew = load(crew_id)?;
    let text = text.trim();
    if text.is_empty() {
        bail!("empty message");
    }
    crew.messages.push(CrewMsg {
        author: "you".into(),
        kind: "user".into(),
        content: text.to_string(),
        ts: now(),
    });
    crew.updated = now();
    save(&crew)?;
    Ok(())
}

pub fn add_member(crew_id: &str, spec: &str) -> Result<Crew> {
    let mut crew = load(crew_id)?;
    let m = parse_member(spec, &crew.members)?;
    crew.members.push(m);
    crew.updated = now();
    save(&crew)?;
    Ok(crew)
}

pub fn remove_member(crew_id: &str, name: &str) -> Result<Crew> {
    let mut crew = load(crew_id)?;
    let before = crew.members.len();
    crew.members.retain(|m| m.name != name);
    if crew.members.len() == before {
        bail!("no member named \"{name}\"");
    }
    crew.updated = now();
    save(&crew)?;
    Ok(crew)
}

/// per-member token/cost summary — the "who burned what" report
pub fn usage_report(crew_id: &str) -> Result<String> {
    let crew = load(crew_id)?;
    let mut out = String::new();
    if crew.usage.is_empty() {
        return Ok("crew usage: no api requests yet".into());
    }
    let mut treq = 0usize;
    let mut tin = 0u64;
    let mut tout = 0u64;
    let mut tcached = 0u64;
    let mut tcost = 0.0f64;
    out.push_str("crew usage — per member:");
    for (name, rows) in &crew.usage {
        let role = crew
            .members
            .iter()
            .find(|m| &m.name == name)
            .map(|m| m.role.clone())
            .unwrap_or_default();
        let (reqs, tin_m, tout_m, tc_m, cost_m) = crate::usage::totals(rows);
        treq += reqs;
        tin += tin_m;
        tout += tout_m;
        tcached += tc_m;
        tcost += cost_m;
        out.push_str(&format!(
            "\n  {:<18} {:>4} req {:>9} in {:>9} out {:>9} cached  {}",
            trunc(name, 18),
            reqs,
            crate::usage::fmt_tokens(tin_m),
            crate::usage::fmt_tokens(tout_m),
            crate::usage::fmt_tokens(tc_m),
            crate::usage::fmt_cost(cost_m)
        ));
        out.push_str(&format!("  [{role}]"));
        for t in crate::usage::aggregate(rows) {
            out.push_str(&format!(
                "\n      {:<26} {} in / {} out",
                t.model,
                crate::usage::fmt_tokens(t.input),
                crate::usage::fmt_tokens(t.output)
            ));
        }
    }
    out.push_str(&format!(
        "\n\ntotal: {treq} requests · {} in · {} out ({} cached) · {}",
        crate::usage::fmt_tokens(tin),
        crate::usage::fmt_tokens(tout),
        crate::usage::fmt_tokens(tcached),
        crate::usage::fmt_cost(tcost)
    ));
    Ok(out)
}

/// struct shape the gui crew panel renders for the usage split
pub fn usage_json(crew_id: &str) -> Result<serde_json::Value> {
    let crew = load(crew_id)?;
    let mut rows = Vec::new();
    let mut tin = 0u64;
    let mut tout = 0u64;
    let mut tcost = 0.0f64;
    for (name, usage) in &crew.usage {
        let (reqs, in_t, out_t, _c, cost) = crate::usage::totals(usage);
        tin += in_t;
        tout += out_t;
        tcost += cost;
        rows.push(serde_json::json!({
            "name": name,
            "role": crew.members.iter().find(|m| &m.name == name).map(|m| m.role.clone()).unwrap_or_default(),
            "requests": reqs,
            "input": in_t,
            "output": out_t,
            "cost": cost,
        }));
    }
    Ok(serde_json::json!({
        "id": crew.id,
        "status": crew.status,
        "rows": rows,
        "total": {"input": tin, "output": tout, "cost": tcost},
    }))
}

/// full state for the gui crew panel
pub fn state_json(crew_id: &str) -> Result<serde_json::Value> {
    let crew = load(crew_id)?;
    let graph: Vec<serde_json::Value> = mention_graph(&crew)
        .into_iter()
        .map(|(from, to, n)| serde_json::json!({"from": from, "to": to, "n": n}))
        .collect();
    let members: Vec<serde_json::Value> = crew
        .members
        .iter()
        .map(|m| {
            serde_json::json!({
                "name": m.name,
                "role": m.role,
                "profile": m.profile,
                "model": m.model,
                "limit": m.limit,
                "tools": m.tools,
                "used": member_used(&crew, &m.name),
            })
        })
        .collect();
    Ok(serde_json::json!({
        "id": crew.id,
        "goal": crew.goal,
        "status": crew.status,
        "members": members,
        "messages": crew.messages,
        "graph": graph,
        "running": is_running(),
    }))
}

/// auto mode: rounds until DONE, cancel or the cap; the returned string is
/// a short final summary for the chat log
pub fn run_auto(
    crew_id: &str,
    max_rounds: usize,
    cfg: &crate::config::Config,
    tx: &UnboundedSender<ApiEvent>,
) -> tokio::task::JoinHandle<String> {
    let crew_id = crew_id.to_string();
    let cfg = cfg.clone();
    let tx = tx.clone();
    let total = max_rounds.max(1);
    tokio::spawn(async move {
        let mut last = String::new();
        for i in 0..total {
            if CANCEL.load(Ordering::Relaxed) {
                last = "crew auto: cancelled".into();
                break;
            }
            let _ = tx.send(ApiEvent::Note(format!("crew: round {}/{}", i + 1, total)));
            match step(&crew_id, 1, &cfg, &tx).await {
                Ok(msg) => last = msg,
                Err(e) => {
                    last = format!("crew auto stopped: {e:#}");
                    break;
                }
            }
            let done = load(&crew_id).map(|c| c.status == "done").unwrap_or(true);
            if done {
                break;
            }
        }
        last
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn member(name: &str, role: &str) -> Member {
        Member {
            name: name.into(),
            role: role.into(),
            profile: None,
            model: None,
            limit: None,
            tools: false,
        }
    }

    #[test]
    fn parse_member_specs() {
        let taken: Vec<Member> = vec![member("rex", "planner")];
        let m = parse_member("Nora|critic", &taken).unwrap();
        assert_eq!(m.name, "Nora");
        assert_eq!(m.role, "critic");
        let m = parse_member("|reviewer", &taken).unwrap();
        assert_eq!(m.name, "agent-2");
        assert_eq!(m.role, "reviewer");
        let m = parse_member("Mix|coder|fast|gpt-5-mini", &taken).unwrap();
        assert_eq!(m.profile.as_deref(), Some("fast"));
        assert_eq!(m.model.as_deref(), Some("gpt-5-mini"));
        assert!(!m.tools);
        assert!(m.limit.is_none());
        let m = parse_member("Max|coder|tools|limit=20k", &taken).unwrap();
        assert!(m.tools);
        assert_eq!(m.limit, Some(20_000));
        let m = parse_member("Ada|dev|limit=1.5m|fast|tools", &taken).unwrap();
        assert_eq!(m.profile.as_deref(), Some("fast"));
        assert_eq!(m.limit, Some(1_500_000));
        assert!(m.tools);
        let m = parse_member("Ox|ops|limit=off", &taken).unwrap();
        assert!(m.limit.is_none());
        assert!(parse_member("B|ops|limit=wat", &taken).is_err());
        assert!(parse_member("", &taken).is_err());
        assert!(parse_member("||", &taken).is_err());
    }

    #[test]
    fn mention_edges() {
        let mut crew = Crew {
            id: "s-1-g".into(),
            goal: "g".into(),
            status: "idle".into(),
            members: vec![member("rex", "planner"), member("nora", "critic")],
            messages: vec![],
            usage: BTreeMap::new(),
            created: 0,
            updated: 0,
        };
        crew.messages.push(CrewMsg {
            author: "you".into(),
            kind: "user".into(),
            content: "@Rex start, cc @Nora".into(),
            ts: 0,
        });
        crew.messages.push(CrewMsg {
            author: "rex".into(),
            kind: "agent".into(),
            content: "done, @Nora over to you (email a@b.c is not a mention)".into(),
            ts: 0,
        });
        crew.messages.push(CrewMsg {
            author: "rex".into(),
            kind: "agent".into(),
            content: "@rex self-ping ignored".into(),
            ts: 0,
        });
        let g = mention_graph(&crew);
        assert!(g.contains(&("you".into(), "rex".into(), 1)));
        assert!(g.contains(&("you".into(), "nora".into(), 1)));
        assert!(g.contains(&("rex".into(), "nora".into(), 1)));
        assert!(!g.iter().any(|(a, b, _)| a == "rex" && b == "rex"));
    }

    #[test]
    fn rename_done_mention_extraction() {
        assert_eq!(
            extract_line("hello\nDONE: plan ready", "DONE:").as_deref(),
            Some("plan ready")
        );
        assert_eq!(extract_line("no marker", "DONE:"), None);
        assert_eq!(
            extract_line("RENAME: \"fast fox\"", "RENAME:").as_deref(),
            Some("\"fast fox\"")
        );
        assert_eq!(sanitize_name("  \"Big B\". "), "Big B");
        assert_eq!(
            sanitize_name(&extract_line("RENAME: \"fast fox\"", "RENAME:").unwrap()),
            "fast fox"
        );
        assert_eq!(first_mention("ask @Nora to review"), Some("Nora".into()));
        assert_eq!(first_mention("no mentions"), None);
        assert_eq!(first_mention("email me a@b.c"), Some("b".into()));
    }

    #[test]
    fn transcript_and_prompt() {
        let mut crew = Crew {
            id: "s-1-test".into(),
            goal: "ship it".into(),
            status: "idle".into(),
            members: vec![member("rex", "planner"), member("nora", "critic")],
            messages: vec![
                CrewMsg {
                    author: "crew".into(),
                    kind: "system".into(),
                    content: "crew goal: ship it".into(),
                    ts: 0,
                },
                CrewMsg {
                    author: "you".into(),
                    kind: "user".into(),
                    content: "begin".into(),
                    ts: 0,
                },
                CrewMsg {
                    author: "rex".into(),
                    kind: "agent".into(),
                    content: "@Nora check the plan".into(),
                    ts: 0,
                },
            ],
            usage: BTreeMap::new(),
            created: 0,
            updated: 0,
        };
        let msgs = transcript_messages(&crew, 40);
        assert_eq!(msgs.len(), 3);
        assert!(msgs[2].content.contains("rex: @Nora check the plan"));
        // transcript cap keeps only the tail
        for i in 0..50 {
            crew.messages.push(CrewMsg {
                author: "rex".into(),
                kind: "agent".into(),
                content: format!("msg {i}"),
                ts: 0,
            });
        }
        assert_eq!(transcript_messages(&crew, 40).len(), 40);
        let p = crew_system_prompt(&crew, &crew.members[0]);
        assert!(p.contains("You are rex"));
        assert!(p.contains("crew goal: ship it") || p.contains("ship it"));
        assert!(p.contains("nora (critic)") || p.contains("Nora (critic)"));
        assert!(!p.contains("RENAME:"), "real names get no rename hint");
        let p2 = crew_system_prompt(&crew, &member("agent-9", "coder"));
        assert!(p2.contains("RENAME:"));
    }

    #[test]
    fn usage_report_splits_by_member() {
        let _g = TEST_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("hiderola-crew-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("HI_DEROLA_CREW_DIR", &dir);
        let crew = Crew {
            id: "s-1-usage".into(),
            goal: "g".into(),
            status: "idle".into(),
            members: vec![member("rex", "planner"), member("nora", "critic")],
            messages: vec![],
            usage: BTreeMap::from([
                (
                    "rex".into(),
                    vec![UsageRow {
                        model: "m".into(),
                        input: 1000,
                        output: 100,
                        cached: 0,
                        cost: 0.01,
                    }],
                ),
                (
                    "nora".into(),
                    vec![UsageRow {
                        model: "m".into(),
                        input: 2000,
                        output: 200,
                        cached: 500,
                        cost: 0.02,
                    }],
                ),
            ]),
            created: 0,
            updated: 0,
        };
        std::fs::create_dir_all(store_dir()).unwrap();
        save(&crew).unwrap();
        let report = usage_report(&crew.id).unwrap();
        assert!(report.contains("rex"), "{report}");
        assert!(report.contains("nora"), "{report}");
        assert!(report.contains("total: 2 requests"));
        let j = usage_json(&crew.id).unwrap();
        assert_eq!(j["rows"].as_array().unwrap().len(), 2);
        assert_eq!(j["total"]["input"], 3000);
        delete(&crew.id).unwrap();
        std::env::remove_var("HI_DEROLA_CREW_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
