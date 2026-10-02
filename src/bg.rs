use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone)]
pub struct Job {
    pub id: String,
    pub kind: &'static str,
    pub description: String,
    pub status: &'static str,
    pub result: Option<String>,
}

static JOBS: OnceLock<Arc<Mutex<Vec<Job>>>> = OnceLock::new();

fn jobs() -> &'static Arc<Mutex<Vec<Job>>> {
    JOBS.get_or_init(|| Arc::new(Mutex::new(Vec::new())))
}

pub fn start(kind: &str, description: &str) -> String {
    let kind = match kind {
        "bash" => "bash",
        _ => "subagent",
    };
    let mut j = jobs().lock().unwrap();
    let id = format!("bg-{}", j.len() + 1);
    j.push(Job {
        id: id.clone(),
        kind,
        description: description.to_string(),
        status: "running",
        result: None,
    });
    id
}

pub fn finish(id: &str, result: Option<String>) {
    if let Some(job) = jobs().lock().unwrap().iter_mut().find(|j| j.id == id) {
        job.status = if result.is_some() { "done" } else { "failed" };
        job.result = result;
    }
}

pub fn status(id: Option<&str>) -> String {
    let j = jobs().lock().unwrap();
    match id {
        Some(id) => match j.iter().find(|j| j.id == id) {
            Some(job) => match (job.status, &job.result) {
                ("done", Some(r)) => format!("{} [done]\n{}", job.id, r),
                _ => format!("{} [{}] {}", job.id, job.status, job.description),
            },
            None => format!("no such task: {id}"),
        },
        None => {
            if j.is_empty() {
                return "no background tasks".into();
            }
            let mut out = Vec::new();
            for job in j.iter() {
                out.push(format!("{} [{}] {}", job.id, job.status, job.description));
            }
            out.join("\n")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle() {
        let id = start("subagent", "probe task");
        assert!(id.starts_with("bg-"));
        assert!(status(None).contains("running"));
        assert!(status(None).contains("probe task"));
        assert_eq!(status(Some(&id)), format!("{id} [running] probe task"));
        assert!(status(Some("bg-999")).contains("no such task"));
        finish(&id, Some("all good".into()));
        assert!(status(Some(&id)).contains("[done]"));
        assert!(status(Some(&id)).contains("all good"));
        assert!(status(None).contains("[done]"));
        let id2 = start("bash", "failing task");
        finish(&id2, None);
        assert!(status(Some(&id2)).contains("[failed]"));
        assert!(status(None).contains("no background tasks") == false);
    }
}
