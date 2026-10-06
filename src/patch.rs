use anyhow::{anyhow, bail, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Add {
        path: String,
        content: String,
    },
    Update {
        path: String,
        move_to: Option<String>,
        hunks: Vec<Hunk>,
    },
    Delete {
        path: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hunk {
    rows: Vec<(u8, String)>, // 0 context, 1 add, 2 delete
}

impl Hunk {
    fn old(&self) -> Vec<&str> {
        self.rows
            .iter()
            .filter(|(k, _)| *k != 1)
            .map(|(_, s)| s.as_str())
            .collect()
    }

    fn new_from_patch(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|(k, _)| *k != 2)
            .map(|(_, s)| s.clone())
            .collect()
    }
}

enum Active {
    Add {
        path: String,
        lines: Vec<String>,
    },
    Update {
        path: String,
        move_to: Option<String>,
        hunks: Vec<Hunk>,
        cur: Hunk,
    },
    Delete {
        path: String,
    },
}

impl Default for Hunk {
    fn default() -> Self {
        Self { rows: Vec::new() }
    }
}

fn clean(line: &str) -> &str {
    line.strip_suffix('\r').unwrap_or(line)
}

impl Active {
    fn close(self) -> Result<Op> {
        Ok(match self {
            Active::Add { path, lines } => Op::Add {
                content: if lines.is_empty() {
                    String::new()
                } else {
                    format!("{}\n", lines.join("\n"))
                },
                path,
            },
            Active::Update {
                path,
                move_to,
                mut hunks,
                cur,
            } => {
                if !cur.rows.is_empty() {
                    hunks.push(cur);
                }
                if hunks.is_empty() {
                    bail!(
                        "apply_patch: Update File: {path}: no hunks (expected ' '/-/+ lines or @@ sections)"
                    );
                }
                Op::Update {
                    path,
                    move_to,
                    hunks,
                }
            }
            Active::Delete { path } => Op::Delete { path },
        })
    }
}

/// Parse the V4A patch format: *** Begin Patch / *** Add File / *** Update File
/// (*** Move to) with ' ', '-' and '+' body lines, '@@' hunk separators and
/// *** Delete File / *** End Patch. Structural only: contents are applied in plan().
pub fn parse(text: &str) -> Result<Vec<Op>> {
    let mut ops: Vec<Op> = Vec::new();
    let mut active: Option<Active> = None;
    for raw in text.lines() {
        let line = clean(raw);
        let header = line.strip_prefix("*** ").unwrap_or("");
        if header == "Begin Patch" || header == "End Patch" {
            continue;
        }
        if line.trim().is_empty() && active.is_none() {
            continue;
        }
        if let Some(rest) = header.strip_prefix("Add File:") {
            if let Some(a) = active.take() {
                ops.push(a.close()?);
            }
            let p = rest.trim();
            if p.is_empty() {
                bail!("apply_patch: Add File: path required");
            }
            active = Some(Active::Add {
                path: p.to_string(),
                lines: Vec::new(),
            });
            continue;
        }
        if let Some(rest) = header.strip_prefix("Delete File:") {
            if let Some(a) = active.take() {
                ops.push(a.close()?);
            }
            let p = rest.trim();
            if p.is_empty() {
                bail!("apply_patch: Delete File: path required");
            }
            active = Some(Active::Delete {
                path: p.to_string(),
            });
            continue;
        }
        if let Some(rest) = header.strip_prefix("Update File:") {
            if let Some(a) = active.take() {
                ops.push(a.close()?);
            }
            let p = rest.trim();
            if p.is_empty() {
                bail!("apply_patch: Update File: path required");
            }
            active = Some(Active::Update {
                path: p.to_string(),
                move_to: None,
                hunks: Vec::new(),
                cur: Hunk::default(),
            });
            continue;
        }
        if let Some(rest) = header.strip_prefix("Move to:") {
            match &mut active {
                Some(Active::Update { move_to, .. }) => {
                    let p = rest.trim();
                    if p.is_empty() {
                        bail!("apply_patch: Move to: path required");
                    }
                    *move_to = Some(p.to_string());
                }
                _ => bail!("apply_patch: Move to: is only valid inside an Update File section"),
            }
            continue;
        }
        if line.starts_with("***") {
            bail!("apply_patch: unknown patch header: {}", line.trim());
        }
        match &mut active {
            Some(Active::Add { lines, .. }) => {
                let Some(body) = line.strip_prefix('+') else {
                    bail!("apply_patch: Add File: every content line must start with '+'");
                };
                lines.push(body.to_string());
            }
            Some(Active::Update { hunks, cur, .. }) => {
                if line.starts_with("@@") {
                    if !cur.rows.is_empty() {
                        hunks.push(std::mem::replace(cur, Hunk::default()));
                    }
                    continue;
                }
                if let Some(body) = line.strip_prefix('+') {
                    cur.rows.push((1, body.to_string()));
                } else if let Some(body) = line.strip_prefix('-') {
                    cur.rows.push((2, body.to_string()));
                } else {
                    let ctx = line.strip_prefix(' ').unwrap_or(line);
                    cur.rows.push((0, ctx.to_string()));
                }
            }
            Some(Active::Delete { path }) => {
                if !line.trim().is_empty() {
                    bail!("apply_patch: Delete File: {path}: no body lines allowed");
                }
            }
            None => bail!(
                "apply_patch: line outside of any file section: \"{}\" (expected *** Add/Update/Delete File)",
                line.trim()
            ),
        }
    }
    if let Some(a) = active.take() {
        ops.push(a.close()?);
    }
    if ops.is_empty() {
        bail!("apply_patch: empty patch");
    }
    Ok(ops)
}

/// returns (offset, fuzzy) where fuzzy means the match only held after
/// trimming trailing whitespace from both sides
fn find_hunk(body: &[String], h: &Hunk, path: &str, idx: usize) -> Result<(usize, bool)> {
    let old = h.old();
    if old.is_empty() {
        return Ok((0, false));
    }
    if body.len() < old.len() {
        bail!(
            "apply_patch: hunk {idx} of {path}: context not found (the file is shorter than the hunk)"
        );
    }
    for off in 0..=body.len() - old.len() {
        if old
            .iter()
            .zip(&body[off..off + old.len()])
            .all(|(a, b)| *a == b)
        {
            return Ok((off, false));
        }
    }
    for off in 0..=body.len() - old.len() {
        if old
            .iter()
            .zip(&body[off..off + old.len()])
            .all(|(a, b)| a.trim_end() == b.trim_end())
        {
            return Ok((off, true));
        }
    }
    bail!(
        "apply_patch: hunk {idx} of {path}: context not found; ' ' and '-' lines must match the file exactly"
    )
}

/// returns (display, write target, patched content) — pure: the raw file
/// content comes in, callers decide where it is read from (host fs or VM)
fn update_content(
    raw: &str,
    path: &str,
    move_to: &Option<String>,
    hunks: &[Hunk],
) -> Result<(String, String, String)> {
    let crlf = raw.contains("\r\n");
    let had_nl = raw.ends_with('\n');
    let mut body: Vec<String> = raw
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .collect();
    if had_nl {
        body.pop();
    }
    let mut out: Vec<String> = Vec::with_capacity(body.len());
    let mut pos = 0usize;
    for (i, h) in hunks.iter().enumerate() {
        let (at, fuzzy) = find_hunk(&body[pos..], h, path, i + 1)?;
        out.extend_from_slice(&body[pos..pos + at]);
        if fuzzy {
            // context lines keep their original trailing whitespace,
            // only '+' lines come from the patch
            let mut oi = 0usize;
            for (k, s) in &h.rows {
                match k {
                    0 => {
                        out.push(body[pos + at + oi].clone());
                        oi += 1;
                    }
                    1 => out.push(s.clone()),
                    _ => oi += 1,
                }
            }
        } else {
            out.extend(h.new_from_patch());
        }
        pos += at + h.old().len();
    }
    out.extend_from_slice(&body[pos..]);
    let mut content = out.join("\n");
    if had_nl && !content.ends_with('\n') {
        content.push('\n');
    }
    if crlf && !content.is_empty() {
        content = content.replace('\n', "\r\n");
    }
    let display = match move_to {
        Some(to) => format!("{path} -> {to}"),
        None => path.to_string(),
    };
    Ok((
        display,
        move_to.clone().unwrap_or_else(|| path.to_string()),
        content,
    ))
}

pub struct Planned {
    pub write: Vec<(String, String)>,
    pub delete: Vec<String>,
    pub items: Vec<(char, String)>,
}

/// Validate ops against the filesystem and compute patched contents without
/// writing anything; commit() applies the plan.
pub fn plan(ops: Vec<Op>) -> Result<Planned> {
    plan_with(
        ops,
        &|p| std::fs::read_to_string(p).map_err(|e| anyhow!("apply_patch: {p}: {e}")),
        &|p| Ok(std::path::Path::new(p).exists()),
    )
}

/// same as plan(), but reads file contents and existence through the given
/// closures — the sandbox guest tools use this to validate and patch VM
/// files over ssh instead of host paths
pub fn plan_with(
    ops: Vec<Op>,
    read: &dyn Fn(&str) -> Result<String>,
    exists: &dyn Fn(&str) -> Result<bool>,
) -> Result<Planned> {
    let mut write: Vec<(String, String)> = Vec::new();
    let mut delete: Vec<String> = Vec::new();
    let mut items: Vec<(char, String)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for op in ops {
        let (path, extra) = match &op {
            Op::Add { path, .. } | Op::Delete { path } => (path.clone(), None),
            Op::Update { path, move_to, .. } => (path.clone(), move_to.clone()),
        };
        for p in std::iter::once(&path).chain(extra.iter()) {
            if seen.iter().any(|s| s == p) {
                bail!("apply_patch: duplicate operation on {p}");
            }
            seen.push(p.to_string());
        }
        match op {
            Op::Add { path, content } => {
                if exists(&path)? {
                    bail!("apply_patch: Add File: {path}: already exists");
                }
                write.push((path.clone(), content));
                items.push(('A', path));
            }
            Op::Delete { path } => {
                if !exists(&path)? {
                    bail!("apply_patch: Delete File: {path}: not found");
                }
                delete.push(path.clone());
                items.push(('D', path));
            }
            Op::Update {
                path,
                move_to,
                hunks,
            } => {
                let raw = read(&path)
                    .map_err(|e| anyhow!("apply_patch: Update File: {path}: {:#}", e))?;
                let (display, target, content) = update_content(&raw, &path, &move_to, &hunks)?;
                write.push((target, content));
                if let Some(from) = (&move_to).as_deref().filter(|t| *t != path) {
                    delete.push(path.clone());
                    let _ = from;
                }
                items.push(('M', display));
            }
        }
    }
    Ok(Planned {
        write,
        delete,
        items,
    })
}

/// applies the plan and returns the written file paths (moves target the new path)
pub fn commit(p: Planned) -> Result<Vec<String>> {
    for (path, content) in &p.write {
        let fp = std::path::Path::new(path);
        if let Some(parent) = fp.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| anyhow!("apply_patch: {path}: {e}"))?;
            }
        }
        std::fs::write(fp, content).map_err(|e| anyhow!("apply_patch: {path}: {e}"))?;
    }
    for path in &p.delete {
        std::fs::remove_file(path).map_err(|e| anyhow!("apply_patch: {path}: {e}"))?;
    }
    Ok(p.write.into_iter().map(|(p, _)| p).collect())
}

/// every path mentioned by the patch, in order of appearance (Move to included)
pub fn paths(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.lines() {
        let line = clean(raw);
        let header = line.strip_prefix("*** ").unwrap_or("");
        let p = header
            .strip_prefix("Add File:")
            .or_else(|| header.strip_prefix("Update File:"))
            .or_else(|| header.strip_prefix("Delete File:"))
            .or_else(|| header.strip_prefix("Move to:"));
        if let Some(p) = p {
            let p = p.trim();
            if !p.is_empty() {
                out.push(p.to_string());
            }
        }
    }
    out
}

pub struct Preview {
    pub label: String,
    pub old: Option<String>,
    pub new: Option<String>,
}

/// best-effort per-file before/after for the diff preview; entries are skipped
/// when the patch or the file is unreadable
pub fn preview(text: &str) -> Vec<Preview> {
    let Ok(ops) = parse(text) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for op in ops {
        match op {
            Op::Add { path, content } => out.push(Preview {
                label: path,
                old: None,
                new: Some(content),
            }),
            Op::Delete { path } => {
                let old = std::fs::read_to_string(&path).ok();
                out.push(Preview {
                    label: path,
                    old,
                    new: None,
                })
            }
            Op::Update { path, hunks, .. } => {
                let old = std::fs::read_to_string(&path).ok();
                let new = plan(vec![Op::Update {
                    path: path.clone(),
                    move_to: None,
                    hunks,
                }])
                .ok()
                .and_then(|p| p.write.into_iter().next().map(|(_, c)| c));
                if new.is_some() {
                    out.push(Preview {
                        label: path,
                        old,
                        new,
                    });
                }
            }
        }
    }
    out
}

/// One-line-per-file summary for the detail view: "A a.rs, M b.rs -> c.rs, D d.rs"
pub fn summarize(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut pending: Option<(char, String)> = None;
    for raw in text.lines() {
        let line = clean(raw);
        let header = line.strip_prefix("*** ").unwrap_or("");
        let entry = if let Some(rest) = header.strip_prefix("Add File:") {
            Some(('A', rest.trim().to_string()))
        } else if let Some(rest) = header.strip_prefix("Update File:") {
            Some(('M', rest.trim().to_string()))
        } else if let Some(rest) = header.strip_prefix("Delete File:") {
            Some(('D', rest.trim().to_string()))
        } else if let Some(rest) = header.strip_prefix("Move to:") {
            match pending.take() {
                Some(('M', p)) => Some(('M', format!("{p} -> {}", rest.trim()))),
                other => {
                    pending = other;
                    None
                }
            }
        } else {
            None
        };
        match (entry, pending.take()) {
            (Some(e), prev) => {
                if let Some((k, p)) = prev {
                    out.push(format!("{k} {p}"));
                }
                pending = Some(e);
            }
            (None, prev) => pending = prev,
        }
    }
    if let Some((k, p)) = pending {
        out.push(format!("{k} {p}"));
    }
    out.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> String {
        let d = std::env::temp_dir().join(format!("hiderola-patch-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("f.txt").display().to_string()
    }

    #[test]
    fn plan_with_closures_behave_like_plan() {
        // the guest apply_patch path plans against closures instead of the
        // host fs — semantics must be identical
        let mut files = std::collections::BTreeMap::new();
        files.insert("a.txt".to_string(), "one\ntwo\n".to_string());
        let read = |p: &str| -> Result<String> {
            files.get(p).cloned().ok_or_else(|| anyhow!("missing {p}"))
        };
        let exists = |p: &str| -> Result<bool> { Ok(files.contains_key(p)) };

        // deleting a missing file must fail before anything is planned
        let patch = "*** Begin Patch\n*** Add File: b.txt\n+hello\n*** Delete File: nope.txt\n*** End Patch";
        assert!(plan_with(parse(patch).unwrap(), &read, &exists).is_err());

        // add + update go through
        let patch =
            "*** Begin Patch\n*** Add File: b.txt\n+hello\n*** Update File: a.txt\n@@\n one\n-two\n+TWO\n*** End Patch";
        let planned = plan_with(parse(patch).unwrap(), &read, &exists).unwrap();
        assert_eq!(
            planned.write,
            vec![
                ("b.txt".to_string(), "hello\n".to_string()),
                ("a.txt".to_string(), "one\nTWO\n".to_string()),
            ]
        );
        assert_eq!(planned.delete, Vec::<String>::new());
        assert!(planned.items.contains(&('A', "b.txt".to_string())));
        assert!(planned.items.contains(&('M', "a.txt".to_string())));

        // moving a file writes the new path and deletes the old one
        files.insert("m.txt".to_string(), "x\n".to_string());
        let read2 = |p: &str| -> Result<String> {
            files.get(p).cloned().ok_or_else(|| anyhow!("missing {p}"))
        };
        let exists2 = |p: &str| -> Result<bool> { Ok(files.contains_key(p)) };
        let patch =
            "*** Begin Patch\n*** Update File: m.txt\n*** Move to: n.txt\n@@\n-x\n+y\n*** End Patch";
        let planned = plan_with(parse(patch).unwrap(), &read2, &exists2).unwrap();
        assert_eq!(
            planned.write,
            vec![("n.txt".to_string(), "y\n".to_string())]
        );
        assert_eq!(planned.delete, vec!["m.txt".to_string()]);
    }

    #[test]
    fn add_update_delete_roundtrip() {
        let p = tmp("round");
        let patch = format!("*** Begin Patch\n*** Add File: {p}\n+alpha\n+beta\n*** End Patch");
        commit(plan(parse(&patch).unwrap()).unwrap()).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "alpha\nbeta\n");

        let patch = format!(
            "*** Begin Patch\n*** Update File: {p}\n@@\n alpha\n-beta\n+GAMMA\n*** End Patch"
        );
        commit(plan(parse(&patch).unwrap()).unwrap()).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "alpha\nGAMMA\n");

        let patch = format!("*** Delete File: {p}\n");
        commit(plan(parse(&patch).unwrap()).unwrap()).unwrap();
        assert!(!std::path::Path::new(&p).exists());
    }

    #[test]
    fn multi_hunk_without_envelope() {
        let p = tmp("multi");
        std::fs::write(&p, "one\ntwo\nthree\nfour\nfive\n").unwrap();
        let patch = format!(
            "*** Update File: {p}\n@@ tag\n one\n-two\n+TWO\n@@\n four\n-five\n+FIVE\n+six\n"
        );
        commit(plan(parse(&patch).unwrap()).unwrap()).unwrap();
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            "one\nTWO\nthree\nfour\nFIVE\nsix\n"
        );
    }

    #[test]
    fn move_to_renames() {
        let a = tmp("mv-a");
        let b = tmp("mv-b");
        std::fs::write(&a, "x\n").unwrap();
        let patch = format!("*** Update File: {a}\n*** Move to: {b}\n@@\n-x\n+y\n");
        commit(plan(parse(&patch).unwrap()).unwrap()).unwrap();
        assert!(!std::path::Path::new(&a).exists());
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "y\n");
    }

    #[test]
    fn sloppy_trailing_whitespace_matches() {
        let p = tmp("sloppy");
        std::fs::write(&p, "keep  \ndrop\t\n").unwrap();
        let patch = format!("*** Update File: {p}\n@@\n keep\n-drop\n+clean\n");
        commit(plan(parse(&patch).unwrap()).unwrap()).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "keep  \nclean\n");
    }

    #[test]
    fn crlf_preserved() {
        let p = tmp("crlf");
        std::fs::write(&p, "a\r\nb\r\n").unwrap();
        let patch = format!("*** Update File: {p}\n@@\n a\n-b\n+B\n");
        commit(plan(parse(&patch).unwrap()).unwrap()).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "a\r\nB\r\n");
    }

    #[test]
    fn no_trailing_newline_kept() {
        let p = tmp("nonl");
        std::fs::write(&p, "a\nb").unwrap();
        let patch = format!("*** Update File: {p}\n@@\n a\n-b\n+B\n");
        commit(plan(parse(&patch).unwrap()).unwrap()).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "a\nB");
    }

    #[test]
    fn failures_reported() {
        let p = tmp("fail");
        std::fs::write(&p, "real line\n").unwrap();
        let patch = format!("*** Update File: {p}\n@@\n wrong\n-lines\n+new\n");
        assert!(plan(parse(&patch).unwrap()).is_err());
        let patch = format!("*** Add File: {p}\n+x\n");
        assert!(plan(parse(&patch).unwrap()).is_err());
        let dup = format!(
            "*** Update File: {p}\n@@\n real line\n-real line\n+one\n*** Update File: {p}\n@@\n one\n+two\n"
        );
        assert!(plan(parse(&dup).unwrap()).is_err());
        assert!(parse("hello world").is_err());
        assert!(parse("").is_err());
        let bad_add = format!("*** Add File: {p}2\nplain line\n");
        assert!(parse(&bad_add).is_err());
        let unknown = "*** Frobnicate File: x\n";
        assert!(parse(unknown).is_err());
    }

    #[test]
    fn summarize_kinds() {
        let s = summarize(
            "*** Begin Patch\n*** Add File: a.rs\n+x\n*** Update File: b.rs\n*** Move to: c.rs\n*** Delete File: d.rs\n*** End Patch",
        );
        assert_eq!(s, "A a.rs, M b.rs -> c.rs, D d.rs");
    }
}
