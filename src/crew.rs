//! crew: one goal, several ai participants talking in a single session.
//!
//! a crew is a stored transcript (json per crew under the sessions dir
//! sibling `crew/`) plus a tiny round-based runner: every round each member
//! gets the transcript and answers with its role in mind; @Name steers who
//! speaks next, a "DONE:" line ends the crew, "RENAME:" lets a placeholder
//! pick its own callsign. members ride any configured provider profile, so
//! one crew can mix apis and models. usage rows are recorded per member —
//! the `/crew usage` report splits tokens and cost by participant.

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

/// "Name|role[|profile|model]" — a leading "|role" makes a placeholder name
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
    let profile = rest
        .get(1)
        .copied()
        .filter(|s| !s.is_empty())
        .map(String::from);
    let model = rest
        .get(2)
        .copied()
        .filter(|s| !s.is_empty())
        .map(String::from);
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
    })
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
         - Coordinate, plan, review, split work — the crew chat has no file or shell tools.\n\
         - When the shared goal is fully achieved, end the reply with a line \"DONE: <short summary>\".\n",
    );
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
        while !queue.is_empty() {
            if CANCEL.load(Ordering::Relaxed) {
                break 'rounds;
            }
            let name = queue.remove(0);
            let Some(member) = crew.members.iter().find(|m| m.name == name).cloned() else {
                continue;
            };
            let reply = match ask_member(&crew, &member, cfg).await {
                Ok((text, rows)) => {
                    if !rows.is_empty() {
                        crew.usage
                            .entry(member.name.clone())
                            .or_default()
                            .extend(rows);
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

/// one provider call for one member; returns the reply plus the usage rows
/// captured from the provider's usage events (attributed by the caller)
async fn ask_member(
    crew: &Crew,
    member: &Member,
    cfg: &crate::config::Config,
) -> Result<(String, Vec<UsageRow>)> {
    let _ = crew;
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
    let req = ChatRequest {
        system: crew_system_prompt(crew, member),
        messages: transcript_messages(crew, 40),
        model: pc.model.clone(),
        max_tokens: pc.max_tokens,
        temperature: pc.temperature,
        top_p: pc.top_p,
        stream: false,
        tools: Vec::new(),
    };
    let (etx, mut erx) = tokio::sync::mpsc::unbounded_channel();
    let reply = provider.chat(&req, &etx).await?;
    drop(etx);
    let mut rows: Vec<UsageRow> = Vec::new();
    while let Ok(ev) = erx.try_recv() {
        if let ApiEvent::Usage {
            input,
            output,
            cached,
        } = ev
        {
            let cost = cost_of(&pc.kind, &pc.model, input, output, cached);
            rows.push(UsageRow {
                model: pc.model.clone(),
                input,
                output,
                cached,
                cost,
            });
        }
    }
    Ok((reply.text, rows))
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
    Ok(serde_json::json!({
        "id": crew.id,
        "goal": crew.goal,
        "status": crew.status,
        "members": crew.members,
        "messages": crew.messages,
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
        assert!(parse_member("", &taken).is_err());
        assert!(parse_member("||", &taken).is_err());
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
