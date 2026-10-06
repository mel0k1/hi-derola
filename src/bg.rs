use std::sync::{Arc, Mutex, OnceLock};

const BUF_CAP: usize = 256 * 1024;
const LIVE_TAIL: usize = 1200;

/// custom kill for jobs that are not a host process — sandbox VM background
/// tasks register a hook that kills the remote process group over ssh;
/// receives the task id
pub type KillHook = Arc<dyn Fn(&str) -> String + Send + Sync>;

#[derive(Clone)]
pub struct Job {
    pub id: String,
    pub kind: &'static str,
    pub description: String,
    pub status: &'static str,
    pub result: Option<String>,
    pid: Option<u32>,
    abort: Option<tokio::task::AbortHandle>,
    out: Option<Arc<Mutex<String>>>,
    kill: Option<KillHook>,
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
        pid: None,
        abort: None,
        out: None,
        kill: None,
    });
    id
}

pub fn attach_kill(id: &str, f: KillHook) {
    if let Some(job) = jobs().lock().unwrap().iter_mut().find(|j| j.id == id) {
        job.kill = Some(f);
    }
}

pub fn attach_pid(id: &str, pid: u32) {
    if let Some(job) = jobs().lock().unwrap().iter_mut().find(|j| j.id == id) {
        job.pid = Some(pid);
    }
}

pub fn attach_abort(id: &str, handle: tokio::task::AbortHandle) {
    if let Some(job) = jobs().lock().unwrap().iter_mut().find(|j| j.id == id) {
        job.abort = Some(handle);
    }
}

pub fn attach_out(id: &str, buf: Arc<Mutex<String>>) {
    if let Some(job) = jobs().lock().unwrap().iter_mut().find(|j| j.id == id) {
        job.out = Some(buf);
    }
}

pub fn finish(id: &str, result: Option<String>) {
    if let Some(job) = jobs().lock().unwrap().iter_mut().find(|j| j.id == id) {
        if job.status == "running" {
            job.status = if result.is_some() { "done" } else { "failed" };
        }
        job.result = result;
    }
}

pub fn killed(id: &str) -> bool {
    jobs()
        .lock()
        .unwrap()
        .iter()
        .any(|j| j.id == id && j.status == "killed")
}

pub fn append(id: &str, chunk: &str) {
    let buf = {
        let j = jobs().lock().unwrap();
        let Some(job) = j.iter().find(|j| j.id == id) else {
            return;
        };
        match &job.out {
            Some(b) => b.clone(),
            None => return,
        }
    };
    let mut b = buf.lock().unwrap();
    b.push_str(chunk);
    if b.len() > BUF_CAP {
        let mut cut = b.len() - BUF_CAP;
        while cut < b.len() && !b.is_char_boundary(cut) {
            cut += 1;
        }
        b.drain(..cut);
    }
}

pub fn output(id: &str) -> Option<String> {
    let buf = {
        let j = jobs().lock().unwrap();
        let job = j.iter().find(|j| j.id == id)?;
        job.out.clone()?
    };
    let b = buf.lock().unwrap().clone();
    Some(b)
}

pub fn kill(id: &str) -> String {
    let mut j = jobs().lock().unwrap();
    let Some(job) = j.iter_mut().find(|j| j.id == id) else {
        return format!("no such task: {id}");
    };
    if job.status != "running" {
        return format!("task {id} is not running ({})", job.status);
    }
    let mut able = false;
    if let Some(f) = job.kill.clone() {
        let msg = f(&id);
        job.status = "killed";
        return msg;
    }
    if let Some(h) = &job.abort {
        h.abort();
        able = true;
    }
    if let Some(p) = job.pid {
        kill_pid(p);
        able = true;
    }
    if !able {
        return format!("task {id} cannot be killed (no handle)");
    }
    job.status = "killed";
    format!("task {id} killed")
}

#[cfg(unix)]
fn kill_pid(pid: u32) {
    let _ = std::process::Command::new("kill")
        .args(["-9", &format!("-{pid}")])
        .output();
    let _ = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .output();
}

#[cfg(windows)]
fn kill_pid(pid: u32) {
    let _ = std::process::Command::new("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .output();
}

fn tail_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    s[start..].to_string()
}

pub fn status(id: Option<&str>) -> String {
    let j = jobs().lock().unwrap();
    match id {
        Some(id) => match j.iter().find(|j| j.id == id) {
            Some(job) => match (job.status, &job.result) {
                ("done", Some(r)) => format!("{} [done]\n{}", job.id, r),
                _ => {
                    let mut s = format!("{} [{}] {}", job.id, job.status, job.description);
                    if job.status == "running" {
                        if let Some(buf) = &job.out {
                            let b = buf.lock().unwrap();
                            if !b.trim().is_empty() {
                                s.push_str(&format!(
                                    "\n--- output so far ---\n{}",
                                    tail_str(b.as_str(), LIVE_TAIL)
                                ));
                            }
                        }
                    }
                    s
                }
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

    #[test]
    fn live_output() {
        let id = start("bash", "live task");
        let buf = Arc::new(Mutex::new(String::new()));
        attach_out(&id, buf);
        assert_eq!(output(&id).as_deref(), Some(""));
        append(&id, "line one\n");
        append(&id, "line two\n");
        assert_eq!(output(&id).as_deref(), Some("line one\nline two\n"));
        assert!(status(Some(&id)).contains("output so far"));
        assert!(status(Some(&id)).contains("line two"));
        finish(&id, Some("done".into()));
        let id3 = start("bash", "no buf task");
        assert_eq!(output(&id3), None);
        append(&id3, "ignored");
        assert_eq!(output(&id3), None);
    }

    #[test]
    fn kill_flow() {
        let id = start("bash", "kill me");
        assert!(kill("bg-99999").contains("no such task"));
        let out = kill(&id);
        assert!(out.contains("cannot be killed"), "{out}");
        let id2 = start("bash", "with pid");
        attach_pid(&id2, u32::MAX - 1);
        let out = kill(&id2);
        assert!(out.contains("killed"), "{out}");
        assert!(killed(&id2));
        assert!(status(Some(&id2)).contains("[killed]"));
        assert!(kill(&id2).contains("is not running"));
        finish(&id2, None);
        assert!(
            status(Some(&id2)).contains("[killed]"),
            "finish must not override killed"
        );
    }
}
