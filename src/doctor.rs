//! /doctor — one-shot environment self-check. With the sandbox feature ssh
//! became a hard requirement, so one command answers "what works here":
//! provider/config, the ssh client, qemu + accel, lsp servers, post-edit
//! formatters, live mcp states and the sandbox inventory. The report is a
//! plain text block (TUI note now, a tauri command can reuse `run` later).

use crate::fmt::find_on_path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone)]
pub struct Check {
    pub status: Status,
    pub topic: &'static str,
    pub detail: String,
}

impl Check {
    fn line(&self) -> String {
        let s = match self.status {
            Status::Ok => "ok",
            Status::Warn => "warn",
            Status::Fail => "fail",
        };
        format!("  {s:<4} {:<8} {}", self.topic, self.detail)
    }
}

/// the pieces doctor needs from the live app (never the api key itself)
pub struct DoctorInput {
    pub provider_kind: String,
    pub model: String,
    pub has_api_key: bool,
    pub config_path: String,
}

/// blocking environment checks: PATH scans plus a couple of instant
/// `--version` spawns — run off the async workers (spawn_blocking)
pub fn collect(input: &DoctorInput) -> Vec<Check> {
    vec![
        config_check(input),
        ssh_check(),
        qemu_check(),
        lsp_check(),
        fmt_check(),
    ]
}

fn config_check(input: &DoctorInput) -> Check {
    Check {
        status: if input.has_api_key {
            Status::Ok
        } else {
            Status::Fail
        },
        topic: "config",
        detail: format!(
            "provider {} · model {} · api key {} · {}",
            input.provider_kind,
            input.model,
            if input.has_api_key {
                "set"
            } else {
                "missing (config.toml, HI_DEROLA_API_KEY or the provider's env var)"
            },
            input.config_path,
        ),
    }
}

fn ssh_check() -> Check {
    match crate::sshx::find_ssh() {
        Some(bin) => Check {
            status: Status::Ok,
            topic: "ssh",
            detail: format!("{} · {}", ssh_version(&bin), bin.display()),
        },
        None => Check {
            status: Status::Fail,
            topic: "ssh",
            detail: "no ssh client — sandbox VMs, the in-VM agent and the bash route are unusable (windows: Settings > Apps > Optional features > OpenSSH client)".into(),
        },
    }
}

fn ssh_version(bin: &std::path::Path) -> String {
    match std::process::Command::new(bin).arg("-V").output() {
        Ok(o) => parse_ssh_version_text(&format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        )),
        Err(_) => "version probe failed".into(),
    }
}

/// `ssh -V` prints to stderr: "OpenSSH_9.7p1 Debian-7, OpenSSL ..." — the
/// first comma token is the version
fn parse_ssh_version_text(text: &str) -> String {
    let first = text
        .lines()
        .next()
        .unwrap_or("")
        .split(',')
        .next()
        .unwrap_or("")
        .trim();
    if first.is_empty() {
        "unknown version".into()
    } else {
        first.to_string()
    }
}

fn qemu_check() -> Check {
    let q = crate::sandbox::detect_qemu();
    if q.ok() {
        Check {
            // tcg is the pure-software fallback: VMs boot but crawl
            status: if q.accel.as_deref() == Some("tcg") {
                Status::Warn
            } else {
                Status::Ok
            },
            topic: "qemu",
            detail: format!(
                "{} · {} · accel {}",
                q.system_version.as_deref().unwrap_or("?"),
                q.img_version.as_deref().unwrap_or("?"),
                q.accel.as_deref().unwrap_or("n/a"),
            ),
        }
    } else {
        let missing = match (q.system_path.is_some(), q.img_path.is_some()) {
            (false, false) => "qemu-system-x86_64 + qemu-img",
            (false, _) => "qemu-system-x86_64",
            (_, false) => "qemu-img",
            _ => "qemu",
        };
        Check {
            status: Status::Warn,
            topic: "qemu",
            detail: format!(
                "{missing} not found — the sandbox tab stays disabled (host work is unaffected)"
            ),
        }
    }
}

fn lsp_check() -> Check {
    let names = crate::lsp::server_names();
    let found: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| find_on_path(n).is_some())
        .collect();
    if found.is_empty() {
        Check {
            status: Status::Warn,
            topic: "lsp",
            detail: format!(
                "none found — diagnostics and go-to-definition stay off (candidates: {})",
                names.join(", ")
            ),
        }
    } else {
        Check {
            status: Status::Ok,
            topic: "lsp",
            detail: format!("{} ({} of {})", found.join(", "), found.len(), names.len()),
        }
    }
}

/// formatter binaries used after write_file/edit (ruff/black are py
/// alternatives, prettier can also come from node_modules/.bin)
const FMT_TOOLS: &[&str] = &[
    "rustfmt",
    "gofmt",
    "ruff",
    "black",
    "clang-format",
    "shfmt",
    "ktlint",
    "prettier",
];

fn fmt_check() -> Check {
    let found: Vec<&str> = FMT_TOOLS
        .iter()
        .copied()
        .filter(|n| find_on_path(n).is_some())
        .collect();
    if found.is_empty() {
        Check {
            status: Status::Warn,
            topic: "fmt",
            detail: "none found — post-edit formatting stays off".into(),
        }
    } else {
        Check {
            status: Status::Ok,
            topic: "fmt",
            detail: found.join(", "),
        }
    }
}

/// live mcp states → one doctor line (a thin summary over /mcpstatus)
pub fn mcp_check(entries: &[crate::mcp::McpStatusEntry], configured: usize) -> Check {
    if configured == 0 {
        return Check {
            status: Status::Ok,
            topic: "mcp",
            detail: "none configured ([[mcp]] in config.toml)".into(),
        };
    }
    let connected = entries.iter().filter(|e| e.state == "connected").count();
    let rest: Vec<String> = entries
        .iter()
        .filter(|e| e.state != "connected")
        .map(|e| format!("{} ({})", e.name, e.state))
        .collect();
    if rest.is_empty() {
        Check {
            status: Status::Ok,
            topic: "mcp",
            detail: format!("{connected}/{configured} connected"),
        }
    } else {
        Check {
            status: Status::Warn,
            topic: "mcp",
            detail: format!("{connected}/{configured} connected · {}", rest.join(", ")),
        }
    }
}

/// sandbox inventory plus where the bash route currently points
pub fn sandbox_check() -> Check {
    let m = crate::sandbox::SandboxManager::global();
    let list = m.list();
    let running = list
        .iter()
        .filter(|s| s.state == crate::sandbox::VmState::Running)
        .count();
    let downloading = list
        .iter()
        .filter(|s| s.download.as_ref().map(|d| !d.done).unwrap_or(false))
        .count();
    let route = crate::sandbox::shell_route()
        .and_then(|id| {
            list.iter()
                .find(|s| s.spec.id == id)
                .map(|s| s.spec.name.clone())
        })
        .unwrap_or_else(|| "host".into());
    let mut detail = format!(
        "{} sandbox{} ({running} running",
        list.len(),
        if list.len() == 1 { "" } else { "es" }
    );
    if downloading > 0 {
        detail.push_str(&format!(", {downloading} downloading"));
    }
    detail.push_str(&format!("), bash route → {route}"));
    Check {
        status: Status::Ok,
        topic: "sandbox",
        detail,
    }
}

pub fn render(checks: &[Check]) -> String {
    let mut out = String::from("doctor:");
    for c in checks {
        out.push('\n');
        out.push_str(&c.line());
    }
    out
}

/// full report: blocking env checks off the async workers, then the live mcp
/// states and the sandbox inventory
pub async fn run(
    input: DoctorInput,
    mcp: crate::mcp::McpSlot,
    mcp_cfgs: &[crate::config::McpConfig],
) -> String {
    let mut checks = tokio::task::spawn_blocking(move || collect(&input))
        .await
        .unwrap_or_else(|_| {
            vec![Check {
                status: Status::Fail,
                topic: "doctor",
                detail: "internal error".into(),
            }]
        });
    let entries = crate::mcp::status(&mcp, mcp_cfgs).await;
    checks.push(mcp_check(&entries, mcp_cfgs.len()));
    checks.push(sandbox_check());
    render(&checks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, state: &str) -> crate::mcp::McpStatusEntry {
        crate::mcp::McpStatusEntry {
            name: name.into(),
            state: state.into(),
            detail: String::new(),
        }
    }

    #[test]
    fn ssh_version_takes_first_comma_token() {
        assert_eq!(
            parse_ssh_version_text("OpenSSH_9.7p1 Debian-7ubuntu1, OpenSSL 3.0.13 19 Jan 2026\r\n"),
            "OpenSSH_9.7p1 Debian-7ubuntu1"
        );
        assert_eq!(
            parse_ssh_version_text("OpenSSH_for_Windows_9.5"),
            "OpenSSH_for_Windows_9.5"
        );
        assert_eq!(parse_ssh_version_text(""), "unknown version");
    }

    #[test]
    fn render_lines_are_aligned() {
        let checks = vec![
            Check {
                status: Status::Ok,
                topic: "ssh",
                detail: "OpenSSH_9.7p1".into(),
            },
            Check {
                status: Status::Warn,
                topic: "lsp",
                detail: "none found".into(),
            },
            Check {
                status: Status::Fail,
                topic: "config",
                detail: "api key missing".into(),
            },
        ];
        let out = render(&checks);
        assert!(out.starts_with("doctor:\n"), "{out}");
        assert!(out.contains("  ok   ssh      OpenSSH_9.7p1"), "{out}");
        assert!(out.contains("  warn lsp      none found"), "{out}");
        assert!(out.contains("  fail config   api key missing"), "{out}");
    }

    #[test]
    fn mcp_check_summarizes_states() {
        let none: Vec<crate::mcp::McpStatusEntry> = vec![];
        let c = mcp_check(&none, 0);
        assert_eq!(c.status, Status::Ok);
        assert!(c.detail.contains("none configured"), "{}", c.detail);

        let ok = mcp_check(&[entry("a", "connected"), entry("b", "connected")], 2);
        assert_eq!(ok.status, Status::Ok);
        assert!(ok.detail.contains("2/2 connected"), "{}", ok.detail);

        let mixed = mcp_check(
            &[
                entry("a", "connected"),
                entry("b", "needs auth"),
                entry("c", "failed"),
            ],
            3,
        );
        assert_eq!(mixed.status, Status::Warn);
        assert!(mixed.detail.contains("1/3 connected"), "{}", mixed.detail);
        assert!(mixed.detail.contains("b (needs auth)"), "{}", mixed.detail);
        assert!(mixed.detail.contains("c (failed)"), "{}", mixed.detail);
    }

    #[test]
    fn lsp_server_names_are_unique_candidates() {
        let names = crate::lsp::server_names();
        assert!(names.contains(&"rust-analyzer"));
        let uniq: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(uniq.len(), names.len());
    }

    #[test]
    fn config_check_fails_without_api_key() {
        let with_key = DoctorInput {
            provider_kind: "openai".into(),
            model: "gpt-4o".into(),
            has_api_key: true,
            config_path: "/c/config.toml".into(),
        };
        let c = config_check(&with_key);
        assert_eq!(c.status, Status::Ok);
        assert!(c.detail.contains("api key set"), "{}", c.detail);

        let no_key = DoctorInput {
            has_api_key: false,
            ..with_key
        };
        let c = config_check(&no_key);
        assert_eq!(c.status, Status::Fail);
        assert!(c.detail.contains("api key missing"), "{}", c.detail);
    }
}
