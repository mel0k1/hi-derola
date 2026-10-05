//! ssh access to a running sandbox VM through the system ssh client.
//!
//! The host ships an ssh client everywhere we care about (Windows 10+ has
//! OpenSSH in System32\OpenSSH, Linux/macOS always do), so instead of
//! pulling a full ssh library we drive the binary directly — the same way
//! the sandbox module drives qemu. Every invocation is batch/key-only:
//!
//! ```text
//! ssh -i <sandbox>/id_ed25519 -p <port>
//!     -o BatchMode=yes -o StrictHostKeyChecking=accept-new
//!     -o UserKnownHostsFile=<sandbox>/known_hosts -o ConnectTimeout=5
//!     -o LogLevel=ERROR -o IdentitiesOnly=yes  <login>@127.0.0.1
//! ```
//!
//! `known_hosts` lives in the sandbox directory so recreated VMs never trip
//! over a changed host key, and nothing leaks into the user's real file.
//!
//! Building blocks: [`wait_ready`] (poll until the guest sshd answers),
//! [`exec`] (run a command, capture output), [`stream`] (pipe stdin through
//! a remote shell, line by line — used for the agent install script and for
//! file uploads via `cat >`), plus the [`INSTALL_SH`] bootstrap that turns a
//! freshly booted cloud VM into a machine with the hi-derola agent built
//! from source at `~/.cargo/bin/hi-derola`.

use crate::sandbox::silent;
use serde::Serialize;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// guest-side bootstrap: system build tools (when root or passwordless sudo
/// is available — the wizard's root flag maps to exactly that), rustup, then
/// `cargo install` of hi-derola from the public repository. Every meaningful
/// step echoes a `[tag]` line so the GUI log can follow along.
pub const INSTALL_SH: &str = r#"set -e
echo "[*] agent install started"
if [ "$(id -u)" = "0" ]; then
  SUD=""
elif sudo -n true 2>/dev/null; then
  SUD="sudo"
else
  SUD=""
  NOROOT=1
  echo "[i] no root access: system packages are skipped (building needs a C compiler)"
fi
if command -v apt-get >/dev/null 2>&1 && [ -z "${NOROOT:-}" ]; then
  echo "[*] apt-get: ca-certificates curl git build-essential pkg-config"
  export DEBIAN_FRONTEND=noninteractive
  $SUD apt-get update -y || echo "[!] apt-get update failed, trying to continue"
  $SUD apt-get install -y --no-install-recommends ca-certificates curl git build-essential pkg-config || echo "[!] apt-get install failed, trying to continue"
else
  echo "[i] apt-get skipped"
fi
command -v curl >/dev/null 2>&1 || { echo "[x] curl is missing — install it in the VM and retry"; exit 30; }
if [ ! -x "$HOME/.cargo/bin/cargo" ] && ! command -v cargo >/dev/null 2>&1; then
  echo "[*] rustup: installing the rust toolchain (a few minutes)"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
fi
export PATH="$HOME/.cargo/bin:$PATH"
command -v cargo >/dev/null 2>&1 || { echo "[x] cargo is still unavailable after rustup"; exit 31; }
echo "[*] fetching hi-derola sources (github.com/mel0k1/hi-derola)"
rm -rf "$HOME/hi-derola-src"
git clone --depth 1 https://github.com/mel0k1/hi-derola "$HOME/hi-derola-src" || { echo "[x] git clone failed"; exit 32; }
echo "[*] cargo install: building the agent from source — the long step, watch the ram/cpu you gave this VM"
cargo install --path "$HOME/hi-derola-src" --force || { echo "[x] cargo install failed"; exit 33; }
[ -x "$HOME/.cargo/bin/hi-derola" ] || { echo "[x] build finished but the binary is missing"; exit 34; }
echo "[ok] agent installed: $HOME/.cargo/bin/hi-derola"
"#;

/// one-off probe run right after the guest sshd answers: is the agent
/// binary already there (survived an app/VM restart) and which source
/// version built it
pub const PROBE_SH: &str = r#"if [ -x "$HOME/.cargo/bin/hi-derola" ]; then echo AGENT_PRESENT; sed -n "s/^version *= *\"\(.*\)\"/version:\1/p" "$HOME/hi-derola-src/Cargo.toml" 2>/dev/null | head -n 1; fi"#;

/// locate the host ssh client (PATH first, then the Windows system copy)
pub fn find_ssh() -> Option<PathBuf> {
    if let Some(p) = crate::sandbox::find_binary("ssh") {
        return Some(p);
    }
    #[cfg(windows)]
    {
        let windir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        let cand = PathBuf::from(windir).join(r"System32\OpenSSH\ssh.exe");
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

/// everything the ssh argv needs to reach one sandbox
#[derive(Debug, Clone)]
pub struct SshTarget {
    pub key: PathBuf,
    pub known_hosts: PathBuf,
    pub port: u16,
    pub user: String,
}

impl SshTarget {
    pub fn new(dir: &Path, port: u16, user: &str) -> Self {
        Self {
            key: dir.join("id_ed25519"),
            known_hosts: dir.join("known_hosts"),
            port,
            user: user.to_string(),
        }
    }
}

/// argv for one ssh invocation; `remote` is appended after `--` (empty for
/// an interactive login shell). `force_pty` adds -t so the guest TUI gets a
/// terminal when a remote command is given.
pub fn ssh_argv(t: &SshTarget, force_pty: bool, remote: &[&str]) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-i".into(),
        t.key.display().to_string(),
        "-p".into(),
        t.port.to_string(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        "-o".into(),
        format!("UserKnownHostsFile={}", t.known_hosts.display()),
        "-o".into(),
        "ConnectTimeout=5".into(),
        "-o".into(),
        "LogLevel=ERROR".into(),
        "-o".into(),
        "IdentitiesOnly=yes".into(),
    ];
    if force_pty {
        a.push("-t".into());
    }
    a.push(format!("{}@127.0.0.1", t.user));
    if !remote.is_empty() {
        a.push("--".into());
        a.extend(remote.iter().map(|s| s.to_string()));
    }
    a
}

/// the full ssh argv for a user-visible terminal window (interactive shell,
/// or the agent TUI when `agent` is set)
pub fn terminal_cmdline(t: &SshTarget, agent: bool) -> Vec<String> {
    ssh_argv(t, agent, if agent { &["~/.cargo/bin/hi-derola"] } else { &[] })
}

/// true while the ssh child should keep running
fn cancelled(cancel: &AtomicBool) -> bool {
    cancel.load(Ordering::Relaxed)
}

/// poll the guest sshd until it answers (or the deadline/cancel hits);
/// returns the seconds the wait took (for the "booted in Ns" badge)
pub fn wait_ready(
    bin: &Path,
    t: &SshTarget,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<u64, String> {
    let started = Instant::now();
    loop {
        if cancelled(cancel) {
            return Err("cancelled".to_string());
        }
        let res = silent(&mut Command::new(bin))
            .args(ssh_argv(t, false, &["true"]))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if matches!(res, Ok(s) if s.success()) {
            return Ok(started.elapsed().as_secs());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "ssh did not answer within {}s — is this a cloud image with cloud-init?",
                started.elapsed().as_secs()
            ));
        }
        // ~2s between attempts, cancel-checked
        for _ in 0..20 {
            if cancelled(cancel) {
                return Err("cancelled".to_string());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SshOut {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// run one remote command through the guest shell and capture the output;
/// reader threads keep the pipes drained so chatty commands cannot deadlock
pub fn exec(
    bin: &Path,
    t: &SshTarget,
    remote: &str,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<SshOut, String> {
    let mut child = silent(&mut Command::new(bin))
        .args(ssh_argv(t, false, &[remote]))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn ssh: {e}"))?;

    fn drain<R: Read + Send + 'static>(pipe: R) -> mpsc::Receiver<Vec<u8>> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let mut reader = pipe;
            let _ = reader.read_to_end(&mut buf);
            let _ = tx.send(buf);
        });
        rx
    }
    let out_rx = drain(child.stdout.take().expect("stdout piped"));
    let err_rx = drain(child.stderr.take().expect("stderr piped"));

    let started = Instant::now();
    let status;
    loop {
        if cancelled(cancel) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("cancelled".to_string());
        }
        match child.try_wait() {
            Ok(Some(st)) => {
                status = st;
                break;
            }
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "remote command timed out after {}s",
                        timeout.as_secs()
                    ));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("ssh wait: {e}")),
        }
    }
    let code = status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&out_rx.recv().unwrap_or_default()).to_string();
    let stderr = String::from_utf8_lossy(&err_rx.recv().unwrap_or_default()).to_string();
    Ok(SshOut {
        code,
        stdout,
        stderr,
    })
}

/// pipe `stdin_data` through a remote shell command ("sh -s" for the install
/// script, "cat > file" for uploads), forwarding every output line to
/// `on_line` as it arrives. Returns the remote exit code.
pub fn stream(
    bin: &Path,
    t: &SshTarget,
    remote: &str,
    stdin_data: Option<&[u8]>,
    cancel: &AtomicBool,
    mut on_line: impl FnMut(&str),
) -> Result<i32, String> {
    let mut base = Command::new(bin);
    silent(&mut base);
    base.args(ssh_argv(t, false, &[remote]))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if stdin_data.is_some() {
        base.stdin(Stdio::piped());
    } else {
        base.stdin(Stdio::null());
    }
    let mut child = base.spawn().map_err(|e| format!("spawn ssh: {e}"))?;
    if let Some(data) = stdin_data {
        let mut stdin = child.stdin.take().expect("stdin piped");
        stdin
            .write_all(data)
            .map_err(|e| format!("ssh stdin: {e}"))?;
        // dropped here -> the remote command sees EOF
    }

    // both pipes -> one channel of lines (interleaving between them is fine
    // for a progress log); the loop ends when both pumps disconnect, which
    // happens after the remote side closed its stdout/stderr
    let (tx, rx) = mpsc::channel::<String>();
    fn pump<R: Read + Send + 'static>(pipe: R, tx: mpsc::Sender<String>) {
        std::thread::spawn(move || {
            let mut reader = BufReader::new(pipe);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if tx
                            .send(line.trim_end_matches(['\r', '\n']).to_string())
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        });
    }
    pump(child.stdout.take().expect("stdout piped"), tx.clone());
    pump(child.stderr.take().expect("stderr piped"), tx);

    loop {
        if cancelled(cancel) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("cancelled".to_string());
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => on_line(&line),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = child.wait().map_err(|e| format!("ssh wait: {e}"))?;
    Ok(status.code().unwrap_or(-1))
}

#[derive(Debug, Clone)]
pub struct AgentProbe {
    pub present: bool,
    pub version: Option<String>,
}

/// check the guest for an already-installed agent binary
pub fn probe_agent(bin: &Path, t: &SshTarget, cancel: &AtomicBool) -> Result<AgentProbe, String> {
    let out = exec(bin, t, PROBE_SH, Duration::from_secs(15), cancel)?;
    if out.code != 0 {
        return Err(format!(
            "probe failed (code {}): {}",
            out.code,
            out.stderr.trim()
        ));
    }
    Ok(parse_probe(&out.stdout))
}

fn parse_probe(stdout: &str) -> AgentProbe {
    AgentProbe {
        present: stdout.contains("AGENT_PRESENT"),
        version: stdout
            .lines()
            .find_map(|l| l.trim().strip_prefix("version:"))
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty()),
    }
}

/// quote one argv element for an intermediate `cmd /C start` line
#[cfg(windows)]
fn win_quote(s: &str) -> String {
    if s.is_empty() {
        "\"\"".to_string()
    } else if s.contains(' ') {
        format!("\"{s}\"")
    } else {
        s.to_string()
    }
}

/// launch a terminal window with an interactive ssh session into the VM;
/// windows uses `cmd /C start`, unix tries the usual terminal emulators
pub fn spawn_terminal(bin: &Path, ssh_args: &[String]) -> Result<(), String> {
    #[cfg(windows)]
    {
        let line = ssh_args
            .iter()
            .map(|a| win_quote(a))
            .collect::<Vec<_>>()
            .join(" ");
        let child = silent(&mut Command::new("cmd"))
            .args([
                "/C",
                &format!(
                    "start \"hi-derola sandbox\" \"{}\" {}",
                    bin.display(),
                    line
                ),
            ])
            .spawn()
            .map_err(|e| format!("spawn terminal: {e}"))?;
        std::thread::spawn(move || {
            let mut c = child;
            let _ = c.wait();
        });
        return Ok(());
    }
    #[cfg(unix)]
    {
        const CANDIDATES: &[&str] = &[
            "x-terminal-emulator",
            "gnome-terminal",
            "konsole",
            "xfce4-terminal",
            "alacritty",
            "xterm",
        ];
        for name in CANDIDATES {
            let Some(term) = crate::sandbox::find_binary(name) else {
                continue;
            };
            // `-e` for the classic emulators, `--` for gnome-terminal
            let sep = if *name == "gnome-terminal" { "--" } else { "-e" };
            let mut argv: Vec<String> = vec![
                term.display().to_string(),
                sep.to_string(),
                bin.display().to_string(),
            ];
            argv.extend(ssh_args.iter().cloned());
            match Command::new(&argv[0]).args(&argv[1..]).spawn() {
                Ok(child) => {
                    std::thread::spawn(move || {
                        let mut c = child;
                        let _ = c.wait();
                    });
                    return Ok(());
                }
                Err(_) => continue, // wrong -e dialect etc. — try the next one
            }
        }
        Err("no terminal emulator found on the host".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> SshTarget {
        SshTarget::new(Path::new("/vm/test"), 2222, "derola")
    }

    #[test]
    fn argv_shape() {
        let t = target();
        let a = ssh_argv(&t, false, &["true"]);
        let s = a.join(" ");
        assert!(s.contains("-i /vm/test/id_ed25519"));
        assert!(s.contains("-p 2222"));
        assert!(s.contains("-o BatchMode=yes"));
        assert!(s.contains("-o StrictHostKeyChecking=accept-new"));
        assert!(s.contains("-o UserKnownHostsFile=/vm/test/known_hosts"));
        assert!(s.contains("-o ConnectTimeout=5"));
        assert!(s.contains("-o IdentitiesOnly=yes"));
        assert!(s.contains("derola@127.0.0.1"));
        assert!(s.ends_with("-- true"));
        assert!(!s.contains("-t "));

        let a = ssh_argv(&t, true, &[]);
        let s = a.join(" ");
        assert!(s.contains(" -t"));
        assert!(!s.contains("--"), "interactive session carries no remote command");
        assert_eq!(a.last().unwrap(), "derola@127.0.0.1");

        let a = ssh_argv(&t, true, &["~/.cargo/bin/hi-derola"]);
        let s = a.join(" ");
        assert!(s.ends_with("-- ~/.cargo/bin/hi-derola"));
    }

    #[test]
    fn terminal_cmdline_agent_and_shell() {
        let t = target();
        let shell = terminal_cmdline(&t, false);
        assert!(!shell.iter().any(|a| a == "-t"), "plain shell needs no forced pty");
        assert!(!shell.iter().any(|a| a.contains("hi-derola")));
        let agent = terminal_cmdline(&t, true);
        assert!(agent.iter().any(|a| a == "-t"));
        assert!(agent.iter().any(|a| a == "~/.cargo/bin/hi-derola"));
        assert!(agent.iter().any(|a| a == "derola@127.0.0.1"));
    }

    #[test]
    fn install_script_shape() {
        assert!(INSTALL_SH.contains("cargo install --path"));
        assert!(INSTALL_SH.contains("https://sh.rustup.rs"));
        assert!(INSTALL_SH.contains("github.com/mel0k1/hi-derola"));
        assert!(INSTALL_SH.contains("build-essential"));
        assert!(INSTALL_SH.contains("sudo -n true"));
        assert!(INSTALL_SH.contains("[ok] agent installed"));
        assert!(PROBE_SH.contains("AGENT_PRESENT"));
        assert!(PROBE_SH.contains("version:"));
    }

    #[test]
    fn probe_parse() {
        let p = parse_probe("AGENT_PRESENT\nversion:0.1.0\n");
        assert!(p.present);
        assert_eq!(p.version.as_deref(), Some("0.1.0"));
        // whitespace / \r tolerant
        let p = parse_probe("AGENT_PRESENT\r\nversion: 1.2.3\r\n");
        assert_eq!(p.version.as_deref(), Some("1.2.3"));
        let p = parse_probe("");
        assert!(!p.present);
        assert_eq!(p.version, None);
        // version line without AGENT_PRESENT cannot happen, but must not
        // fabricate a present flag
        let p = parse_probe("version:9.9.9");
        assert!(!p.present);
        assert_eq!(p.version.as_deref(), Some("9.9.9"));
    }
}
