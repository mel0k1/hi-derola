//! turn snapshots backing /undo and /redo.
//!
//! primary backend: a shadow git repo in the user's data dir, keyed by the
//! working directory — survives restarts, respects the project's .gitignore,
//! and never touches the user's own .git. fallback: the old in-memory store
//! when git is not available on PATH.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{bail, Result};

// memory-fallback caps
const MAX_TURNS: usize = 20;
const MAX_FILES: usize = 4000;
const MAX_FILE_BYTES: u64 = 1_000_000;
/// extra excludes for trees without a .gitignore, so a stray node_modules
/// does not balloon the shadow repo (they union with the project .gitignore)
const EXTRA_EXCLUDES: &str = "node_modules/\ntarget/\ndist/\nbuild/\nout/\n.git/\n";

// ---------- public API over the process working directory ----------

pub fn begin_turn() {
    let root = root();
    begin_turn_in(&root);
}

pub fn end_turn() {
    let root = root();
    end_turn_in(&root);
}

pub fn undo() -> Option<String> {
    undo_in(&root())
}

pub fn redo() -> Option<String> {
    redo_in(&root())
}

fn root() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

// ---------- backend choice ----------

fn git_available() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| {
        std::process::Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

fn snapshots_base() -> PathBuf {
    std::env::var_os("HI_DEROLA_SNAPSHOT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("hi-derola")
                .join("snapshots")
        })
}

/// one shadow repo per working directory, hex-hashed into the data dir
fn git_dir_for(root: &Path) -> PathBuf {
    let mut h = DefaultHasher::new();
    root.display().to_string().hash(&mut h);
    snapshots_base().join(format!("{:016x}.git", h.finish()))
}

fn redo_file(root: &Path) -> PathBuf {
    git_dir_for(root).join("redo.txt")
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let dir = git_dir_for(root);
    let out = std::process::Command::new("git")
        .arg(format!("--git-dir={}", dir.display()))
        .arg(format!("--work-tree={}", root.display()))
        .args(args)
        .output()?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.first().unwrap_or(&"?"),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn ensure_git_repo(root: &Path) -> Result<()> {
    let dir = git_dir_for(root);
    if dir.join("HEAD").exists() {
        return Ok(());
    }
    std::fs::create_dir_all(&dir)?;
    let out = std::process::Command::new("git")
        .arg("init")
        .arg("--bare")
        .arg("--quiet")
        .arg(&dir)
        .output()?;
    if !out.status.success() {
        bail!(
            "git init failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    // self-contained identity: commits must work without a global git config
    let excludes = dir.join("excludes");
    let excludes_str = excludes.to_string_lossy().to_string();
    let cfg: [(&str, &str); 4] = [
        ("user.email", "hi-derola@local"),
        ("user.name", "hi-derola"),
        ("commit.gpgsign", "false"),
        ("core.excludesFile", excludes_str.as_str()),
    ];
    for (k, v) in cfg {
        let _ = std::process::Command::new("git")
            .arg(format!("--git-dir={}", dir.display()))
            .arg("config")
            .arg(k)
            .arg(v)
            .output();
    }
    std::fs::write(dir.join("excludes"), EXTRA_EXCLUDES)?;
    Ok(())
}

/// stage everything and commit; returns true when a commit was made
fn git_commit(root: &Path, msg: &str) -> Result<bool> {
    ensure_git_repo(root)?;
    git(root, &["add", "-A"])?;
    let status = git(root, &["status", "--porcelain"])?;
    if status.trim().is_empty() {
        return Ok(false);
    }
    git(root, &["commit", "--quiet", "-m", msg])?;
    // a fresh turn invalidates the redo stack
    let _ = std::fs::write(redo_file(root), "");
    Ok(true)
}

fn changed_files(root: &Path, from: &str, to: &str) -> usize {
    git(root, &["diff", "--name-only", from, to])
        .map(|out| out.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0)
}

// ---------- git backend ----------

fn begin_turn_in(root: &Path) {
    if !git_available() {
        let mut s = store().lock().unwrap();
        s.pending = Some(capture_in(root));
        return;
    }
    // a baseline commit makes the very first turn undoable
    if git(root, &["rev-parse", "--verify", "HEAD"]).is_err() {
        let _ = git_commit(root, "hi-derola baseline");
    }
}

fn end_turn_in(root: &Path) {
    if !git_available() {
        let mut s = store().lock().unwrap();
        if let Some(before) = s.pending.take() {
            let after = capture_in(root);
            if after.hash != before.hash {
                s.undo.push(Turn { before, after });
                if s.undo.len() > MAX_TURNS {
                    s.undo.remove(0);
                }
                s.redo.clear();
            }
        }
        return;
    }
    let _ = git_commit(root, "hi-derola turn");
}

fn undo_in(root: &Path) -> Option<String> {
    if !git_available() {
        let mut s = store().lock().unwrap();
        let t = s.undo.pop()?;
        let n = t.before.files.len();
        restore_in(root, &t.before);
        s.redo.push(t);
        return Some(format!("undo: restored {n} files"));
    }
    ensure_git_repo(root).ok()?;
    let heads = git(root, &["rev-parse", "HEAD", "HEAD~1"]).ok()?;
    let mut lines = heads.lines();
    let cur = lines.next()?.trim().to_string();
    let prev = lines.next()?.trim().to_string();
    let n = changed_files(root, "HEAD", &prev);
    git(root, &["reset", "--hard", &prev]).ok()?;
    let mut log = std::fs::read_to_string(redo_file(root)).unwrap_or_default();
    log.push_str(&cur);
    log.push('\n');
    let _ = std::fs::write(redo_file(root), log);
    Some(format!("undo: restored {n} files"))
}

fn redo_in(root: &Path) -> Option<String> {
    if !git_available() {
        let mut s = store().lock().unwrap();
        let t = s.redo.pop()?;
        let n = t.after.files.len();
        restore_in(root, &t.after);
        s.undo.push(t);
        return Some(format!("redo: reapplied {n} files"));
    }
    ensure_git_repo(root).ok()?;
    let log = std::fs::read_to_string(redo_file(root)).ok()?;
    let all: Vec<&str> = log.lines().filter(|l| !l.trim().is_empty()).collect();
    let target = all.last()?.to_string();
    let n = changed_files(root, "HEAD", &target);
    git(root, &["reset", "--hard", &target]).ok()?;
    let remaining = &all[..all.len() - 1];
    let body = if remaining.is_empty() {
        String::new()
    } else {
        format!("{}\n", remaining.join("\n"))
    };
    let _ = std::fs::write(redo_file(root), body);
    Some(format!("redo: reapplied {n} files"))
}

// ---------- in-memory fallback ----------

struct Snap {
    files: Vec<(String, Option<Vec<u8>>)>,
    hash: u64,
}

struct Turn {
    before: Snap,
    after: Snap,
}

struct Store {
    pending: Option<Snap>,
    undo: Vec<Turn>,
    redo: Vec<Turn>,
}

static STORE: OnceLock<Mutex<Store>> = OnceLock::new();

fn store() -> &'static Mutex<Store> {
    STORE.get_or_init(|| {
        Mutex::new(Store {
            pending: None,
            undo: Vec::new(),
            redo: Vec::new(),
        })
    })
}

fn rel_of(root: &Path, path: &str) -> String {
    Path::new(path)
        .strip_prefix(root)
        .map(|r| r.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string())
}

fn capture_in(root: &Path) -> Snap {
    let root_str = root.display().to_string();
    let mut all = Vec::new();
    crate::files::walk_files(&root_str, &mut all);
    let mut files: Vec<(String, Option<Vec<u8>>)> = Vec::new();
    let mut hasher = DefaultHasher::new();
    for p in all {
        if files.len() >= MAX_FILES {
            break;
        }
        let rel = rel_of(root, &p);
        rel.hash(&mut hasher);
        let meta = std::fs::metadata(&p).ok();
        if meta.as_ref().is_some_and(|m| m.len() > MAX_FILE_BYTES) || meta.is_none() {
            files.push((rel, None));
            continue;
        }
        match std::fs::read(&p) {
            Ok(data) => {
                data.hash(&mut hasher);
                files.push((rel, Some(data)));
            }
            Err(_) => files.push((rel, None)),
        }
    }
    Snap {
        files,
        hash: hasher.finish(),
    }
}

fn restore_in(root: &Path, snap: &Snap) {
    let listed: std::collections::BTreeSet<String> =
        snap.files.iter().map(|(r, _)| r.clone()).collect();
    for (rel, data) in &snap.files {
        let Some(content) = data else { continue };
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&p, content);
    }
    let mut now = Vec::new();
    crate::files::walk_files(&root.display().to_string(), &mut now);
    for p in now {
        let rel = rel_of(root, &p);
        if !listed.contains(&rel) {
            let _ = std::fs::remove_file(&p);
        }
    }
    let mut dirs = Vec::new();
    collect_dirs(root, 0, &mut dirs);
    dirs.sort_by_key(|d| std::cmp::Reverse(d.as_os_str().len()));
    for d in dirs {
        let _ = std::fs::remove_dir(&d);
    }
}

fn collect_dirs(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 10 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.filter_map(|e| e.ok()) {
        let name = e.file_name().to_string_lossy().to_string();
        if matches!(name.as_str(), ".git" | "target" | "node_modules") {
            continue;
        }
        let p = e.path();
        if p.is_dir() {
            out.push(p.clone());
            collect_dirs(&p, depth + 1, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn git_snapshot_flow() {
        if !git_available() {
            return;
        }
        let tmp = tmpdir("hiderola-snapgit");
        let base = tmpdir("hiderola-snapgit-base");
        std::env::set_var("HI_DEROLA_SNAPSHOT_DIR", &base);
        let w = |rel: &str, data: &str| std::fs::write(tmp.join(rel), data).unwrap();
        let r = |rel: &str| std::fs::read_to_string(tmp.join(rel)).unwrap();

        // baseline turn: begin commits the pre-agent state
        w("a.txt", "one\n");
        begin_turn_in(&tmp);
        w("a.txt", "two\n");
        w("b.txt", "new\n");
        end_turn_in(&tmp);
        assert_eq!(r("a.txt"), "two\n");
        assert!(tmp.join("b.txt").exists());

        // undo reverts both the edit and the new file
        let note = undo_in(&tmp).unwrap();
        assert!(note.contains("restored"), "{note}");
        assert_eq!(r("a.txt"), "one\n");
        assert!(!tmp.join("b.txt").exists());

        // redo reapplies
        redo_in(&tmp).unwrap();
        assert_eq!(r("a.txt"), "two\n");
        assert!(tmp.join("b.txt").exists());

        // an unchanged turn does not clear the redo stack (nothing committed)
        end_turn_in(&tmp);
        assert!(undo_in(&tmp).is_some());
        assert!(redo_in(&tmp).is_some());

        // turn 2, then walk back to the baseline and exhaust the stack
        w("c.txt", "third\n");
        end_turn_in(&tmp);
        assert!(undo_in(&tmp).is_some());
        assert!(undo_in(&tmp).is_some());
        assert_eq!(r("a.txt"), "one\n");
        assert!(!tmp.join("b.txt").exists());
        assert!(!tmp.join("c.txt").exists());
        assert!(undo_in(&tmp).is_none());
        assert!(redo_in(&tmp).is_some());

        // the shadow repo lives in the sandboxed base, not the project
        assert!(!tmp.join(".git").exists());
        assert!(git_dir_for(&tmp).join("HEAD").exists());

        std::env::remove_var("HI_DEROLA_SNAPSHOT_DIR");
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn memory_fallback_flow() {
        let tmp = tmpdir("hiderola-snapmem");
        let w = |rel: &str, data: &str| std::fs::write(tmp.join(rel), data).unwrap();
        let mut s = Store {
            pending: None,
            undo: Vec::new(),
            redo: Vec::new(),
        };

        w("a.txt", "one\n");
        s.pending = Some(capture_in(&tmp));
        w("a.txt", "two\n");
        w("b.txt", "new\n");
        let after = capture_in(&tmp);
        s.undo.push(Turn {
            before: s.pending.take().unwrap(),
            after,
        });

        let t = s.undo.pop().unwrap();
        restore_in(&tmp, &t.before);
        assert_eq!(std::fs::read_to_string(tmp.join("a.txt")).unwrap(), "one\n");
        assert!(!tmp.join("b.txt").exists());
        s.redo.push(t);
        let t = s.redo.pop().unwrap();
        restore_in(&tmp, &t.after);
        assert_eq!(std::fs::read_to_string(tmp.join("a.txt")).unwrap(), "two\n");
        assert!(tmp.join("b.txt").exists());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
