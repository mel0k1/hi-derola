use anyhow::{bail, Result};

pub const MAX_RESULTS: usize = 100;

pub fn expand_braces(pattern: &str) -> Vec<String> {
    let chars: Vec<char> = pattern.chars().collect();
    expand_rec(&chars)
}

fn expand_rec(p: &[char]) -> Vec<String> {
    let Some(start) = p.iter().position(|&c| c == '{') else {
        return vec![p.iter().collect()];
    };
    let mut depth = 0i32;
    let mut close = None;
    for (i, &c) in p.iter().enumerate().skip(start) {
        if c == '{' {
            depth += 1;
        } else if c == '}' {
            depth -= 1;
            if depth == 0 {
                close = Some(i);
                break;
            }
        }
    }
    let Some(close) = close else {
        return vec![p.iter().collect()];
    };
    let (head, rest) = (&p[..start], &p[close + 1..]);
    let body = &p[start + 1..close];
    let mut alts: Vec<Vec<char>> = vec![Vec::new()];
    let mut depth = 0i32;
    for &c in body {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                alts.push(Vec::new());
                continue;
            }
            _ => {}
        }
        alts.last_mut().unwrap().push(c);
    }
    let mut out = Vec::new();
    for alt in alts {
        let mut full = head.to_vec();
        full.extend(alt);
        full.extend_from_slice(rest);
        out.extend(expand_rec(&full));
    }
    out
}

pub fn glob_match(pattern: &str, path: &str) -> bool {
    let p: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let f: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match_segments(&p, &f)
}

fn match_segments(p: &[&str], f: &[&str]) -> bool {
    match p.first() {
        None => f.is_empty(),
        Some(&"**") => {
            for i in 0..=f.len() {
                if match_segments(&p[1..], &f[i..]) {
                    return true;
                }
            }
            false
        }
        Some(seg) => {
            let Some(name) = f.first() else {
                return false;
            };
            match_one(seg, name) && match_segments(&p[1..], &f[1..])
        }
    }
}

fn match_one(pat: &str, name: &str) -> bool {
    match_chars(
        &pat.chars().collect::<Vec<_>>(),
        &name.chars().collect::<Vec<_>>(),
    )
}

fn match_chars(p: &[char], n: &[char]) -> bool {
    match p.first() {
        None => n.is_empty(),
        Some('*') => {
            for i in 0..=n.len() {
                if match_chars(&p[1..], &n[i..]) {
                    return true;
                }
            }
            false
        }
        Some('?') => !n.is_empty() && match_chars(&p[1..], &n[1..]),
        Some('[') => {
            let mut i = 1;
            let neg = i < p.len() && (p[i] == '!' || p[i] == '^');
            if neg {
                i += 1;
            }
            let mut k = i;
            while k < p.len() && (p[k] != ']' || k == i) {
                k += 1;
            }
            if k >= p.len() {
                return !n.is_empty() && n[0] == '[' && match_chars(&p[1..], &n[1..]);
            }
            let Some(&c) = n.first() else {
                return false;
            };
            let hit = p[i..k].contains(&c);
            if hit != neg {
                match_chars(&p[k + 1..], &n[1..])
            } else {
                false
            }
        }
        Some(&c) => !n.is_empty() && n[0] == c && match_chars(&p[1..], &n[1..]),
    }
}

pub struct GrepHit {
    pub path: String,
    pub line: usize,
    pub text: String,
}

pub fn glob(root: &str, pattern: &str) -> Result<Vec<String>> {
    if !std::path::Path::new(root).is_dir() {
        bail!("glob: not a directory: {root}");
    }
    let root = &crate::files::norm(root);
    let pats = expand_braces(pattern);
    let mut files = Vec::new();
    crate::files::walk_files(root, &mut files);
    files.sort();
    let mut out = Vec::new();
    for f in &files {
        let rel = f.strip_prefix(root).unwrap_or(f);
        let rel = rel.trim_start_matches('/');
        if pats.iter().any(|p| glob_match(p, rel)) {
            out.push(f.clone());
            if out.len() >= MAX_RESULTS {
                break;
            }
        }
    }
    Ok(out)
}

pub fn grep(root: &str, pattern: &str, include: Option<&str>) -> Result<Vec<GrepHit>> {
    let root = &crate::files::norm(root);
    let re = regex::Regex::new(pattern).map_err(|e| anyhow::anyhow!("grep: {e}"))?;
    let mut files = Vec::new();
    crate::files::walk_files(root, &mut files);
    files.sort();
    let include = include.map(expand_braces);
    let mut hits = Vec::new();
    for f in &files {
        if hits.len() >= MAX_RESULTS {
            break;
        }
        if let Some(pats) = &include {
            let name = f.rsplit('/').next().unwrap_or(f);
            let rel = f.strip_prefix(root).unwrap_or(f);
            let rel = rel.trim_start_matches('/');
            if !pats
                .iter()
                .any(|p| glob_match(p, name) || glob_match(p, rel))
            {
                continue;
            }
        }
        let Ok(bytes) = std::fs::read(f) else {
            continue;
        };
        if bytes.len() > 1_000_000 || bytes.contains(&0) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        for (i, line) in text.lines().enumerate() {
            if re.is_match(line) {
                hits.push(GrepHit {
                    path: f.clone(),
                    line: i + 1,
                    text: line.chars().take(200).collect(),
                });
                if hits.len() >= MAX_RESULTS {
                    break;
                }
            }
        }
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_basics() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(!glob_match("*.rs", "src/main.rs"));
        assert!(glob_match("**/*.rs", "src/main.rs"));
        assert!(glob_match("**/*.rs", "main.rs"));
        assert!(glob_match("src/*.rs", "src/main.rs"));
        assert!(!glob_match("src/*.rs", "src/sub/main.rs"));
        assert!(glob_match("**", "a/b/c.txt"));
        assert!(glob_match("a/**/*.rs", "a/b/c.rs"));
        assert!(glob_match("a/**/*.rs", "a/c.rs"));
    }

    #[test]
    fn glob_wildcards() {
        assert!(glob_match("?at", "cat"));
        assert!(!glob_match("?at", "at"));
        assert!(glob_match("[ch]at", "cat"));
        assert!(!glob_match("[ch]at", "bat"));
        assert!(glob_match("[!c]at", "bat"));
        assert!(glob_match("main.*", "main.rs"));
    }

    #[test]
    fn braces() {
        assert_eq!(expand_braces("*.{ts,tsx}"), vec!["*.ts", "*.tsx"]);
        assert_eq!(expand_braces("a.rs"), vec!["a.rs"]);
        assert_eq!(
            expand_braces("{a,b}/{c,d}.txt"),
            vec!["a/c.txt", "a/d.txt", "b/c.txt", "b/d.txt"]
        );
    }
}
