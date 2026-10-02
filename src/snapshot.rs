use std::collections::BTreeSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const MAX_FILES: usize = 4000;
const MAX_FILE_BYTES: u64 = 1_000_000;
const MAX_TURNS: usize = 20;

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

fn root() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn rel_of(root: &Path, path: &str) -> String {
    Path::new(path)
        .strip_prefix(root)
        .map(|r| r.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string())
}

fn capture() -> Snap {
    let root = root();
    let root_str = root.display().to_string();
    let mut all = Vec::new();
    crate::files::walk_files(&root_str, &mut all);
    let mut files: Vec<(String, Option<Vec<u8>>)> = Vec::new();
    let mut hasher = DefaultHasher::new();
    for p in all {
        if files.len() >= MAX_FILES {
            break;
        }
        let rel = rel_of(&root, &p);
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

fn restore(snap: &Snap) {
    let root = root();
    let listed: BTreeSet<String> = snap.files.iter().map(|(r, _)| r.clone()).collect();
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
        let rel = rel_of(&root, &p);
        if !listed.contains(&rel) {
            let _ = std::fs::remove_file(&p);
        }
    }
    let mut dirs = Vec::new();
    collect_dirs(&root, 0, &mut dirs);
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

pub fn begin_turn() {
    let mut s = store().lock().unwrap();
    s.pending = Some(capture());
}

pub fn end_turn() {
    let mut s = store().lock().unwrap();
    if let Some(before) = s.pending.take() {
        let after = capture();
        if after.hash != before.hash {
            s.undo.push(Turn { before, after });
            if s.undo.len() > MAX_TURNS {
                s.undo.remove(0);
            }
            s.redo.clear();
        }
    }
}

pub fn undo() -> Option<String> {
    let mut s = store().lock().unwrap();
    let t = s.undo.pop()?;
    let n = t.before.files.len();
    restore(&t.before);
    s.redo.push(t);
    Some(format!("undo: restored {n} files"))
}

pub fn redo() -> Option<String> {
    let mut s = store().lock().unwrap();
    let t = s.redo.pop()?;
    let n = t.after.files.len();
    restore(&t.after);
    s.undo.push(t);
    Some(format!("redo: reapplied {n} files"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn undo_redo_flow() {
        let tmp = std::env::temp_dir().join(format!("hiderola-snap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let old = std::env::current_dir().unwrap();
        std::env::set_current_dir(&tmp).unwrap();

        std::fs::write("a.txt", "one\n").unwrap();
        super::begin_turn();
        std::fs::write("a.txt", "two\n").unwrap();
        std::fs::write("b.txt", "new\n").unwrap();
        super::end_turn();

        assert_eq!(std::fs::read_to_string("a.txt").unwrap(), "two\n");
        assert!(std::path::Path::new("b.txt").exists());

        super::undo().unwrap();
        assert_eq!(std::fs::read_to_string("a.txt").unwrap(), "one\n");
        assert!(!std::path::Path::new("b.txt").exists());

        super::redo().unwrap();
        assert_eq!(std::fs::read_to_string("a.txt").unwrap(), "two\n");
        assert!(std::path::Path::new("b.txt").exists());

        super::undo().unwrap();
        super::redo().unwrap();
        assert_eq!(std::fs::read_to_string("a.txt").unwrap(), "two\n");

        let undo_len = {
            let s = super::store().lock().unwrap();
            s.undo.len()
        };
        super::begin_turn();
        super::end_turn();
        let undo_len2 = {
            let s = super::store().lock().unwrap();
            s.undo.len()
        };
        assert_eq!(undo_len, undo_len2);

        std::env::set_current_dir(old).unwrap();
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
