use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(true);

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

const MAX_FMT_BYTES: u64 = 2 * 1024 * 1024;

/// Format a file in place after an edit. Returns the formatter name on success.
pub async fn format_file(path: &str) -> Option<&'static str> {
    if !enabled() {
        return None;
    }
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let (name, prog, args): (&'static str, String, Vec<&str>) = match ext {
        "rs" => ("rustfmt", "rustfmt".into(), vec!["--edition", "2021"]),
        "go" => ("gofmt", "gofmt".into(), vec!["-w"]),
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "html" | "css" | "scss" | "less" | "md"
        | "json" | "jsonc" | "yaml" | "yml" | "vue" | "svelte" | "astro" => match prettier_bin() {
            Some(bin) => ("prettier", bin, vec!["--write", "--log-level", "warn"]),
            None => return None,
        },
        _ => return None,
    };
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_FMT_BYTES {
        return None;
    }
    run(&prog, &args, path).await.then_some(name)
}

fn prettier_bin() -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    let local = cwd.join("node_modules/.bin/prettier");
    if local.is_file() {
        return Some(local.display().to_string());
    }
    find_on_path("prettier")
}

pub fn find_on_path(bin: &str) -> Option<String> {
    let names = if cfg!(windows) {
        vec![format!("{bin}.exe"), format!("{bin}.cmd"), bin.to_string()]
    } else {
        vec![bin.to_string()]
    };
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for name in &names {
            let p = dir.join(name);
            if p.is_file() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if p.metadata().ok()?.permissions().mode() & 0o111 == 0 {
                        continue;
                    }
                }
                return Some(p.display().to_string());
            }
        }
    }
    None
}

async fn run(prog: &str, args: &[&str], path: &str) -> bool {
    let mut cmd = tokio::process::Command::new(prog);
    cmd.args(args).arg(path);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    let wait = async {
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(_) => return false,
        };
        match child.wait().await {
            Ok(st) => st.success(),
            Err(_) => false,
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(15), wait)
        .await
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn formats_rust() {
        if !std::process::Command::new("rustfmt")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return;
        }
        let dir = std::env::temp_dir().join(format!("hiderola-fmt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.rs");
        std::fs::write(&p, "fn main( ){let x=1;}\n").unwrap();
        let path = p.display().to_string();
        let name = format_file(&path).await;
        assert_eq!(name, Some("rustfmt"));
        let out = std::fs::read_to_string(&p).unwrap();
        assert!(out.contains("let x = 1;"), "{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn skips_unknown_ext() {
        let dir = std::env::temp_dir().join(format!("hiderola-fmt2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.xyz");
        std::fs::write(&p, "data\n").unwrap();
        assert_eq!(format_file(&p.display().to_string()).await, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn finds_binary_on_path() {
        assert!(find_on_path(if cfg!(windows) { "cmd" } else { "sh" }).is_some());
        assert!(find_on_path("definitely-not-a-real-binary-xyz").is_none());
    }
}
