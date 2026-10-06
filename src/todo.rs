use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex, OnceLock};

pub const STATUSES: [&str; 4] = ["pending", "in_progress", "completed", "cancelled"];
pub const PRIORITIES: [&str; 3] = ["high", "medium", "low"];
const MAX_ITEMS: usize = 100;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Todo {
    pub content: String,
    pub status: String,
    pub priority: String,
}

static TODOS: OnceLock<Arc<Mutex<Vec<Todo>>>> = OnceLock::new();

fn state() -> &'static Arc<Mutex<Vec<Todo>>> {
    TODOS.get_or_init(|| Arc::new(Mutex::new(Vec::new())))
}

pub fn get() -> Vec<Todo> {
    state().lock().unwrap().clone()
}

pub fn set_list(list: Vec<Todo>) {
    *state().lock().unwrap() = list;
}

pub fn clear() {
    state().lock().unwrap().clear();
}

pub fn validate(list: &[Todo]) -> Result<()> {
    if list.len() > MAX_ITEMS {
        bail!("todowrite: too many items (max {MAX_ITEMS})");
    }
    for t in list {
        if t.content.trim().is_empty() {
            bail!("todowrite: item content is empty");
        }
        if !STATUSES.contains(&t.status.as_str()) {
            bail!(
                "todowrite: bad status '{}' (expected one of: {})",
                t.status,
                STATUSES.join(", ")
            );
        }
        if !PRIORITIES.contains(&t.priority.as_str()) {
            bail!(
                "todowrite: bad priority '{}' (expected one of: {})",
                t.priority,
                PRIORITIES.join(", ")
            );
        }
    }
    Ok(())
}

pub fn parse(list: &Value) -> Result<Vec<Todo>> {
    let Some(arr) = list["todos"].as_array() else {
        bail!("todowrite: 'todos' array required");
    };
    let mut out = Vec::new();
    for t in arr {
        let content = t["content"].as_str().unwrap_or("").trim().to_string();
        let status = t["status"]
            .as_str()
            .unwrap_or("pending")
            .trim()
            .to_lowercase();
        let priority = t["priority"]
            .as_str()
            .unwrap_or("medium")
            .trim()
            .to_lowercase();
        out.push(Todo {
            content,
            status,
            priority,
        });
    }
    Ok(out)
}

pub fn write(list: Vec<Todo>) -> Result<String> {
    validate(&list)?;
    set_list(list);
    Ok(render(&get()))
}

pub fn render(list: &[Todo]) -> String {
    if list.is_empty() {
        return "(todo list is empty)".into();
    }
    let rank = |s: &str| match s {
        "in_progress" => 0,
        "pending" => 1,
        "completed" => 2,
        _ => 3,
    };
    let mut items: Vec<&Todo> = list.iter().collect();
    items.sort_by_key(|t| rank(&t.status));
    let mut out = String::new();
    for t in items {
        let mark = match t.status.as_str() {
            "completed" => "[x]",
            "in_progress" => "[~]",
            "cancelled" => "[-]",
            _ => "[ ]",
        };
        out.push_str(&format!("{mark} {}\n", t.content));
    }
    let done = list.iter().filter(|t| t.status == "completed").count();
    out.push_str(&format!("{}/{} completed", done, list.len()));
    out
}

pub fn write_from_args(args: &str) -> Result<String> {
    let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
    let list = parse(&v)?;
    write(list)
}

pub fn read_render() -> String {
    render(&get())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(content: &str, status: &str, priority: &str) -> Todo {
        Todo {
            content: content.into(),
            status: status.into(),
            priority: priority.into(),
        }
    }

    #[test]
    fn validate_and_defaults() {
        assert!(validate(&[t("a", "pending", "high")]).is_ok());
        assert!(validate(&[t("a", "bogus", "high")]).is_err());
        assert!(validate(&[t("a", "pending", "bogus")]).is_err());
        assert!(validate(&[t("  ", "pending", "low")]).is_err());
        let v: Value = serde_json::from_str(
            r#"{"todos":[{"content":"only content"},{"content":"x","status":"COMPLETED","priority":" LOW "}]}"#,
        )
        .unwrap();
        let list = parse(&v).unwrap();
        assert_eq!(list[0].status, "pending");
        assert_eq!(list[0].priority, "medium");
        assert_eq!(list[1].status, "completed");
        assert_eq!(list[1].priority, "low");
        assert!(validate(&list).is_ok());
        let bad: Value = serde_json::from_str(r#"{"items":[]}"#).unwrap();
        assert!(parse(&bad).is_err());
    }

    #[test]
    fn render_sorts_by_status() {
        let list = vec![
            t("done thing", "completed", "low"),
            t("current thing", "in_progress", "high"),
            t("later thing", "pending", "medium"),
            t("dropped", "cancelled", "low"),
        ];
        let r = render(&list);
        assert!(r.contains("[~] current thing"));
        assert!(r.contains("[ ] later thing"));
        assert!(r.contains("[x] done thing"));
        assert!(r.contains("[-] dropped"));
        let lines: Vec<&str> = r.lines().collect();
        assert!(lines[0].contains("current thing"));
        assert!(lines[3].contains("dropped"));
        assert!(r.ends_with("1/4 completed"));
        assert_eq!(render(&[]), "(todo list is empty)");
    }

    #[test]
    fn write_updates_state() {
        let out = write(vec![
            t("probe a", "in_progress", "high"),
            t("probe b", "pending", "low"),
        ])
        .unwrap();
        assert!(out.contains("[~] probe a"));
        let r = read_render();
        assert!(r.contains("probe b"));
        clear();
        assert_eq!(read_render(), "(todo list is empty)");
        assert!(write(vec![t("x", "nope", "low")]).is_err());
        assert_eq!(read_render(), "(todo list is empty)");
    }
}
