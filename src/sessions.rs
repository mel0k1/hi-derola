use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::chat::Message;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionMeta {
    pub id: String,
    pub title: String,
    pub created: u64,
    pub updated: u64,
    pub count: usize,
    #[serde(default)]
    pub cost: f64,
    #[serde(default)]
    pub parent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeRec {
    pub path: String,
    pub adds: u64,
    pub dels: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSession {
    pub id: String,
    pub title: String,
    pub created: u64,
    pub updated: u64,
    pub system: String,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub tokens_in: u64,
    #[serde(default)]
    pub tokens_out: u64,
    #[serde(default)]
    pub cost: f64,
    #[serde(default)]
    pub todos: Vec<crate::todo::Todo>,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub changes: Vec<ChangeRec>,
}

pub fn store_dir() -> PathBuf {
    if let Ok(d) = std::env::var("HI_DEROLA_SESSIONS_DIR") {
        if !d.trim().is_empty() {
            return PathBuf::from(d);
        }
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("hi-derola")
        .join("sessions")
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn new_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64 + d.as_secs())
        .unwrap_or(0);
    let pid = std::process::id() as u64;
    let mix = (nanos.wrapping_mul(0x9e3779b97f4a7c15) ^ pid.wrapping_mul(0xbf58476d1ce4e5b9)) as u32;
    format!("s-{}-{:08x}", now(), mix)
}

pub fn title_from(text: &str) -> String {
    let line = text.lines().next().unwrap_or("");
    let mut t: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut = t.char_indices().nth(48).map(|(i, _)| i);
    if let Some(i) = cut {
        t.truncate(i);
        t.push('…');
    }
    if t.trim().is_empty() {
        "new chat".into()
    } else {
        t
    }
}

fn file(dir: &std::path::Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn save(st: &StoredSession) -> Result<()> {
    if !safe_id(&st.id) {
        anyhow::bail!("bad session id");
    }
    let dir = store_dir();
    std::fs::create_dir_all(&dir).context("create sessions dir")?;
    let mut st = st.clone();
    if st.created == 0 {
        st.created = now();
    }
    if st.updated < st.created {
        st.updated = now().max(st.created);
    }
    let raw = serde_json::to_string_pretty(&st).context("serialize session")?;
    let tmp = file(&dir, &st.id).with_extension("tmp");
    std::fs::write(&tmp, raw).context("write session")?;
    std::fs::rename(&tmp, file(&dir, &st.id)).context("commit session")?;
    Ok(())
}

pub fn load(id: &str) -> Result<StoredSession> {
    if !safe_id(id) {
        anyhow::bail!("bad session id");
    }
    let raw = std::fs::read_to_string(file(&store_dir(), id)).context("read session")?;
    serde_json::from_str(&raw).context("parse session")
}

pub fn delete(id: &str) -> Result<()> {
    if !safe_id(id) {
        anyhow::bail!("bad session id");
    }
    match std::fs::remove_file(file(&store_dir(), id)) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).context("delete session"),
    }
}

pub fn list() -> Vec<SessionMeta> {
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
        let Ok(v) = serde_json::from_str::<StoredSession>(&raw) else {
            continue;
        };
        out.push(SessionMeta {
            id: v.id,
            title: v.title,
            created: v.created,
            updated: v.updated,
            count: v.messages.len(),
            cost: v.cost,
            parent: v.parent,
        });
    }
    out.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| b.created.cmp(&a.created)));
    out.dedup_by(|a, b| a.id == b.id);
    out
}

pub fn latest() -> Option<StoredSession> {
    list().first().and_then(|m| load(&m.id).ok())
}

pub fn touch(st: &mut StoredSession) {
    let t = now();
    if st.created == 0 {
        st.created = t;
    }
    st.updated = t;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::{Message, Role};

    fn env_guard(name: &str) -> guard::Guard {
        guard::take(name)
    }

    mod guard {
        use std::sync::{Mutex, MutexGuard};
        pub static ENV: Mutex<()> = Mutex::new(());
        pub struct Guard {
            _lock: MutexGuard<'static, ()>,
            dir: std::path::PathBuf,
        }
        pub fn take(name: &str) -> Guard {
            let lock = ENV.lock().unwrap();
            let d = std::env::temp_dir().join(format!("hiderola-test-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::env::set_var("HI_DEROLA_SESSIONS_DIR", &d);
            Guard { _lock: lock, dir: d }
        }
        impl Drop for Guard {
            fn drop(&mut self) {
                std::env::remove_var("HI_DEROLA_SESSIONS_DIR");
                let _ = std::fs::remove_dir_all(&self.dir);
            }
        }
    }

    #[test]
    fn save_load_list_delete() {
        let _g = env_guard("a");
        let st = StoredSession {
            id: new_id(),
            title: title_from("fix the parser\nsecond line"),
            created: 0,
            updated: 0,
            system: "sys".into(),
            messages: vec![
                Message::new(Role::User, "fix the parser"),
                Message::new(Role::Assistant, "done").with_calls(vec![crate::chat::ToolCall {
                    id: "t1".into(),
                    name: "edit".into(),
                    args: "{}".into(),
                }]),
                Message::tool("t1", "ok"),
            ],
            tokens_in: 0,
            tokens_out: 0,
            cost: 0.0,
            todos: vec![],
            parent: None,
            changes: vec![],
        };
        save(&st).unwrap();
        let got = load(&st.id).unwrap();
        assert_eq!(got.title, "fix the parser");
        assert_eq!(got.messages.len(), 3);
        assert_eq!(got.messages[1].tool_calls[0].name, "edit");
        assert_eq!(got.messages[2].role, Role::Tool);

        let l = list();
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].id, st.id);
        assert_eq!(l[0].count, 3);
        assert!(l[0].updated > 0);

        delete(&st.id).unwrap();
        assert!(list().is_empty());
        assert!(delete("nope-123").is_ok());
    }

    #[test]
    fn safe_ids_and_title() {
        assert!(!safe_id("../etc/passwd"));
        assert!(!safe_id(""));
        assert!(safe_id("s-123-abcd"));
        assert_eq!(title_from(""), "new chat");
        let long = "x".repeat(120);
        assert_eq!(title_from(&long).chars().count(), 49);
        assert_eq!(title_from("a  b\t c"), "a b c");
    }

    #[test]
    fn latest_picks_recent() {
        let _g = env_guard("c");
        let mut a = StoredSession {
            id: "s-1-test".into(),
            title: "old".into(),
            created: 1,
            updated: 1,
            system: String::new(),
            messages: vec![],
            tokens_in: 0,
            tokens_out: 0,
            cost: 0.0,
            todos: vec![],
            parent: None,
            changes: vec![],
        };
        save(&a).unwrap();
        a.id = "s-2-test".into();
        a.updated = 2;
        a.todos = vec![crate::todo::Todo {
            content: "probe".into(),
            status: "pending".into(),
            priority: "low".into(),
        }];
        save(&a).unwrap();
        assert_eq!(latest().unwrap().id, "s-2-test");
        assert_eq!(latest().unwrap().todos.len(), 1);
    }
}
