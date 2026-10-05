//! Local sandbox: disposable QEMU virtual machines so the agent (and the
//! user's files) can live away from the host machine.
//!
//! Phase 1 covered the lifecycle around the VM itself: QEMU detection, a
//! creation wizard (image -> resources -> create) with resumable downloads
//! and progress reporting, and start/stop/delete backed by a small on-disk
//! store.
//!
//! Phase 2 makes the VM agent-ready: cloud images get a cloud-init seed
//! (`seed.rs` — CIDATA volume with the wizard login + a generated ssh key,
//! sudo per the root flag), the guest sshd is polled through the host ssh
//! client (`sshx`) and surfaces as a per-card ssh badge, a terminal window
//! can be opened into the VM, and the agent itself can be installed inside
//! the guest (config upload + rustup + cargo install, live log) and then
//! launched there as a TUI — so the agent works on VM files, never on yours.
//!
//! Storage layout under `<config dir>/hi-derola/sandboxes/`:
//! ```text
//! <id>/sandbox.json   the SandboxSpec the wizard produced
//! <id>/state.json     { "pid": <u32|null> } — qemu pid for crash recovery
//! <id>/image.qcow2    downloaded cloud image (debian / ubuntu kinds)
//! <id>/disk.qcow2     the VM disk (overlay on top of image.qcow2)
//! <id>/seed.img       cloud-init CIDATA volume (cloud kinds only)
//! <id>/id_ed25519(.pub)  the VM ssh keypair baked into the seed
//! <id>/known_hosts    per-sandbox host keys (accept-new)
//! <id>/qemu.log       stderr/stdout of the last qemu run (crash tail)
//! ```

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Debian 13 "trixie" minimal cloud image (genericcloud — the smallest
/// variant, built for virtual machines; boots both BIOS and UEFI)
pub const DEBIAN_TRIXIE_URL: &str =
    "https://cloud.debian.org/images/cloud/trixie/latest/debian-13-genericcloud-amd64.qcow2";
/// Ubuntu 24.04 LTS *minimal* cloud image — Canonical trims it to boot
/// smaller and faster than the standard server cloud image (a standard /
/// minimal switch can come later; the minimal one fits the sandbox story)
pub const UBUNTU_2404_URL: &str = "https://cloud-images.ubuntu.com/minimal/releases/24.04/release/ubuntu-24.04-minimal-cloudimg-amd64.img";
/// legacy NixOS entry: kept so old sandbox.json files still load, no longer
/// offered by the wizard (no cloud-init → no seed/ssh login wiring)
pub const NIXOS_URL: &str =
    "https://channels.nixos.org/nixos-25.05/latest-nixos-minimal-x86_64-linux.qcow2";

pub const DISK_MIN: u32 = 5;
pub const DISK_MAX: u32 = 512;
pub const RAM_MIN: u32 = 256;
pub const RAM_MAX: u32 = 65_536;
pub const CPU_MAX: u32 = 32;
pub const PORT_MIN: u16 = 1024;
pub const PORT_MAX: u16 = 65_535;
pub const DEFAULT_PORT: u16 = 2222;

/// how long the ssh-wait thread polls the guest sshd after boot
pub const SSH_WAIT_SECS: u64 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImageKind {
    DebianTrixie,
    #[serde(rename = "ubuntu-24.04")]
    Ubuntu2404,
    /// legacy: not offered by the wizard anymore, but old sandboxes must
    /// keep loading (see NIXOS_URL)
    Nixos,
    /// user-provided image file: .iso boots as install media (cdrom),
    /// anything else (qcow2/raw/vdi/...) is used as the VM disk directly
    Custom,
}

impl ImageKind {
    pub fn label(self) -> &'static str {
        match self {
            ImageKind::DebianTrixie => "Debian 13 (trixie) minimal",
            ImageKind::Ubuntu2404 => "Ubuntu 24.04 LTS minimal",
            ImageKind::Nixos => "NixOS minimal",
            ImageKind::Custom => "own image",
        }
    }

    pub fn url(self) -> Option<&'static str> {
        match self {
            ImageKind::DebianTrixie => Some(DEBIAN_TRIXIE_URL),
            ImageKind::Ubuntu2404 => Some(UBUNTU_2404_URL),
            ImageKind::Nixos => Some(NIXOS_URL),
            ImageKind::Custom => None,
        }
    }

    pub fn needs_download(self) -> bool {
        self.url().is_some()
    }

    /// kinds that ship cloud-init and therefore get a seed image, an ssh
    /// keypair and the wait/install tooling
    pub fn wants_seed(self) -> bool {
        matches!(self, ImageKind::DebianTrixie | ImageKind::Ubuntu2404)
    }
}

/// the VM shape the wizard produces; persisted verbatim as sandbox.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxSpec {
    pub id: String,
    pub name: String,
    /// in-VM ssh login the cloud-init seed creates (cloud kinds)
    #[serde(default = "default_login")]
    pub login: String,
    pub kind: ImageKind,
    /// absolute path to the user-provided image (kind = custom)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iso_path: Option<String>,
    pub disk_gib: u32,
    pub ram_mib: u32,
    pub cpus: u32,
    /// whether the in-VM agent may run as root: passwordless sudo comes
    /// from the cloud-init seed when on, no sudo at all when off
    #[serde(default)]
    pub root: bool,
    /// host port forwarded to guest ssh (22)
    pub ssh_port: u16,
    pub created_at: u64,
}

fn default_login() -> String {
    crate::seed::DEFAULT_LOGIN.to_string()
}

/// wizard request; every resource field is optional and clamped
#[derive(Debug, Clone, Deserialize)]
pub struct NewSandbox {
    pub name: String,
    pub kind: ImageKind,
    #[serde(default)]
    pub login: Option<String>,
    #[serde(default)]
    pub iso_path: Option<String>,
    #[serde(default)]
    pub disk_gib: Option<u32>,
    #[serde(default)]
    pub ram_mib: Option<u32>,
    #[serde(default)]
    pub cpus: Option<u32>,
    #[serde(default)]
    pub root: bool,
    #[serde(default)]
    pub ssh_port: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VmState {
    Stopped,
    Downloading,
    Running,
    Failed,
}

/// QEMU availability snapshot shown by the wizard banner
#[derive(Debug, Clone, Serialize)]
pub struct QemuInfo {
    pub system_path: Option<String>,
    pub system_version: Option<String>,
    pub img_path: Option<String>,
    pub img_version: Option<String>,
    /// whpx | kvm | hvf | tcg (tcg = slow software emulation fallback)
    pub accel: Option<String>,
}

impl QemuInfo {
    pub fn ok(&self) -> bool {
        self.system_path.is_some() && self.img_path.is_some()
    }
}

/// serializable download progress snapshot (the live one lives in atomics)
#[derive(Debug, Clone, Serialize)]
pub struct DownloadProgress {
    pub url: String,
    pub total: u64,
    pub downloaded: u64,
    pub done: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SandboxStatus {
    pub spec: SandboxSpec,
    pub state: VmState,
    pub pid: Option<u32>,
    pub dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download: Option<DownloadProgress>,
    /// ssh/agent reachability (seed kinds only, meaningful while running)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssh: Option<SshInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ------------------------------------------------------------ ssh & agent

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SshState {
    /// VM not running (or the wait thread was cancelled) — the GUI hides it
    Idle,
    /// polling the guest sshd
    Waiting,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentState {
    /// not probed / not installed
    Unknown,
    Installing,
    Installed,
    Failed,
}

/// install progress for the in-VM agent; `log` is the tail of the remote
/// bootstrap output (rustup/cargo lines stream in while it runs)
#[derive(Debug, Clone, Serialize)]
pub struct AgentInfo {
    pub state: AgentState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub log: Vec<String>,
}

/// the per-sandbox ssh snapshot the GUI polls (state machine owned by the
/// wait/install threads, guarded by one mutex)
#[derive(Debug, Clone, Serialize)]
pub struct SshInfo {
    pub state: SshState,
    pub user: String,
    pub port: u16,
    /// seconds the last ready-wait took (or is ticking while waiting)
    pub elapsed_secs: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub agent: AgentInfo,
}

/// live ssh/agent bookkeeping for one sandbox (in-memory only)
#[derive(Debug)]
pub struct SshLive {
    /// stop() / delete() / a new wait run flip this to interrupt the threads
    pub cancel: AtomicBool,
    pub info: Mutex<SshInfo>,
}

impl SshLive {
    fn new(login: &str, port: u16) -> Self {
        Self {
            cancel: AtomicBool::new(false),
            info: Mutex::new(SshInfo {
                state: SshState::Idle,
                user: login.to_string(),
                port,
                elapsed_secs: 0,
                error: None,
                agent: AgentInfo {
                    state: AgentState::Unknown,
                    version: None,
                    error: None,
                    log: Vec::new(),
                },
            }),
        }
    }

    fn snapshot(&self) -> SshInfo {
        self.info.lock().unwrap().clone()
    }

    fn set(&self, f: impl FnOnce(&mut SshInfo)) {
        f(&mut self.info.lock().unwrap());
    }

    /// keep at most the last N bootstrap log lines
    const LOG_TAIL: usize = 200;
    fn push_log(&self, line: &str) {
        self.set(|i| {
            i.agent.log.push(line.to_string());
            let excess = i.agent.log.len().saturating_sub(Self::LOG_TAIL);
            if excess > 0 {
                i.agent.log.drain(0..excess);
            }
        });
    }
}

// ---------------------------------------------------------------- detection

/// search PATH (plus a few well-known install dirs on Windows) for a binary
pub fn find_binary(name: &str) -> Option<PathBuf> {
    let names: Vec<String> = if cfg!(windows) {
        vec![format!("{name}.exe")]
    } else {
        vec![name.to_string()]
    };
    let mut dirs: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        if let Ok(p) = std::env::var("ProgramFiles") {
            dirs.push(PathBuf::from(p).join("qemu"));
        }
        if let Ok(p) = std::env::var("ProgramFiles(x86)") {
            dirs.push(PathBuf::from(p).join("qemu"));
        }
    }
    if let Ok(paths) = std::env::var("PATH") {
        dirs.extend(std::env::split_paths(&paths));
    }
    for dir in dirs {
        for n in &names {
            let candidate = dir.join(n);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// "QEMU emulator version 8.2.0 (Debian 1:8.2.0+ds-1)" -> the whole first line
pub fn parse_version_line(output: &str) -> Option<String> {
    output
        .lines()
        .map(|l| l.trim())
        .find(|l| {
            if l.is_empty() {
                return false;
            }
            let low = l.to_lowercase();
            match low.find("version") {
                Some(i) => low[i + 7..].chars().any(|c| c.is_ascii_digit()),
                None => false,
            }
        })
        .map(|l| l.to_string())
}

/// parse `qemu-system-x86_64 -accel help` output; falls back to "tcg"
pub fn parse_accel_list(output: &str) -> String {
    let low = output.to_lowercase();
    for accel in ["whpx", "kvm", "hvf"] {
        if low.lines().any(|l| l.trim() == accel) {
            return accel.to_string();
        }
    }
    "tcg".to_string()
}

fn run_version(path: &Path) -> Option<String> {
    let out = silent(&mut Command::new(path).arg("--version")).output().ok()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    parse_version_line(&text)
}

fn detect_accel(system: &Path) -> String {
    let out = silent(&mut Command::new(system).arg("-accel").arg("help")).output();
    match out {
        Ok(o) => {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            );
            parse_accel_list(&text)
        }
        Err(_) => "tcg".to_string(),
    }
}

/// probe qemu-system-x86_64 + qemu-img and the best hardware accelerator
pub fn detect_qemu() -> QemuInfo {
    let system = find_binary("qemu-system-x86_64");
    let img = find_binary("qemu-img");
    let system_version = system.as_deref().and_then(run_version);
    let img_version = img.as_deref().and_then(run_version);
    let accel = system.as_deref().map(detect_accel);
    QemuInfo {
        system_path: system.map(|p| p.display().to_string()),
        system_version,
        img_path: img.map(|p| p.display().to_string()),
        img_version,
        accel,
    }
}

// ------------------------------------------------------------- arg builders

/// pure qemu-system arg list for a spec (program path is prepended by the caller)
pub fn build_qemu_args(spec: &SandboxSpec, dir: &Path, accel: &str) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-machine".into(),
        "q35".into(),
        "-accel".into(),
        accel.into(),
    ];
    // tcg (software emulation) benefits from the widest cpu model
    if accel == "tcg" {
        a.extend(["-cpu".into(), "max".into()]);
    }
    a.extend(["-m".into(), spec.ram_mib.to_string()]);
    a.extend(["-smp".into(), spec.cpus.to_string()]);

    match spec.kind {
        ImageKind::Custom if is_iso_path(spec.iso_path.as_deref()) => {
            a.extend([
                "-drive".into(),
                format!(
                    "file={},format=qcow2,if=virtio",
                    dir.join("disk.qcow2").display()
                ),
                "-cdrom".into(),
                spec.iso_path.clone().unwrap_or_default(),
                "-boot".into(),
                "d".into(),
            ]);
        }
        ImageKind::Custom => {
            // user's own disk image is booted directly, no local disk needed
            a.extend([
                "-drive".into(),
                format!(
                    "file={},format=auto,if=virtio",
                    spec.iso_path.clone().unwrap_or_default()
                ),
            ]);
        }
        _ => {
            a.extend([
                "-drive".into(),
                format!(
                    "file={},format=qcow2,if=virtio",
                    dir.join("disk.qcow2").display()
                ),
            ]);
            if spec.kind.wants_seed() {
                // cloud-init NoCloud seed: a tiny FAT volume labeled CIDATA
                // carrying user-data/meta-data (login + ssh key)
                a.extend([
                    "-drive".into(),
                    format!(
                        "file={},format=raw,if=virtio",
                        dir.join("seed.img").display()
                    ),
                ]);
            }
        }
    }

    a.extend([
        "-netdev".into(),
        // loopback bind: the guest ssh is reachable from this machine only
        // (and no firewall prompt on windows)
        format!(
            "user,id=n0,hostfwd=tcp:127.0.0.1:{}-:22",
            spec.ssh_port
        ),
        "-device".into(),
        "virtio-net-pci,netdev=n0".into(),
        "-device".into(),
        "virtio-rng-pci".into(),
        "-rtc".into(),
        "base=utc".into(),
        "-name".into(),
        format!("hiderola-{}", spec.id),
    ]);
    a
}

/// pure qemu-img arg list: overlay on the cloud image or a fresh disk
pub fn build_img_args(kind: ImageKind, dir: &Path, disk_gib: u32) -> Vec<String> {
    let disk = dir.join("disk.qcow2");
    match kind {
        ImageKind::DebianTrixie | ImageKind::Ubuntu2404 | ImageKind::Nixos => vec![
            "create".into(),
            "-f".into(),
            "qcow2".into(),
            "-b".into(),
            dir.join("image.qcow2").display().to_string(),
            "-F".into(),
            "qcow2".into(),
            disk.display().to_string(),
            format!("{disk_gib}G"),
        ],
        ImageKind::Custom => vec![
            "create".into(),
            "-f".into(),
            "qcow2".into(),
            disk.display().to_string(),
            format!("{disk_gib}G"),
        ],
    }
}

fn is_iso_path(path: Option<&str>) -> bool {
    match path {
        Some(p) => Path::new(p)
            .extension()
            .map(|e| e.to_ascii_lowercase() == "iso")
            .unwrap_or(false),
        None => false,
    }
}

// ------------------------------------------------------------------ manager

/// live download state kept in atomics; snapshot() renders it for the UI
#[derive(Debug)]
struct ProgShared {
    url: String,
    total: AtomicU64,
    downloaded: AtomicU64,
    done: AtomicBool,
    cancel: AtomicBool,
}

impl ProgShared {
    fn new(url: String) -> Self {
        Self {
            url,
            total: AtomicU64::new(0),
            downloaded: AtomicU64::new(0),
            done: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
        }
    }

    fn snapshot(&self) -> DownloadProgress {
        DownloadProgress {
            url: self.url.clone(),
            total: self.total.load(Ordering::Relaxed),
            downloaded: self.downloaded.load(Ordering::Relaxed),
            done: self.done.load(Ordering::Relaxed),
            error: None,
        }
    }
}

#[derive(Debug)]
struct Entry {
    spec: SandboxSpec,
    state: VmState,
    pid: Option<u32>,
    /// pid was recovered from state.json after an app restart — the OS may
    /// have recycled it for an unrelated process, so verify before killing
    pid_from_disk: bool,
    prog: Option<Arc<ProgShared>>,
    /// ssh/agent state machine (seed kinds only)
    ssh: Option<Arc<SshLive>>,
    error: Option<String>,
    /// set right before we kill qemu so the watchdog reports Stopped, not Failed
    stopping: bool,
}

/// persisted as <id>/state.json so a running VM survives (and is detected
/// after) an app restart
#[derive(Debug, Serialize, Deserialize)]
struct PersistState {
    #[serde(default)]
    pid: Option<u32>,
}

pub struct SandboxManager {
    dir: PathBuf,
    inner: Mutex<BTreeMap<String, Entry>>,
}

impl SandboxManager {
    /// manager rooted at <config>/hi-derola/sandboxes (shared app instance)
    pub fn global() -> &'static Arc<SandboxManager> {
        static M: OnceLock<Arc<SandboxManager>> = OnceLock::new();
        M.get_or_init(|| {
            let dir = crate::config::config_path()
                .parent()
                .map(|p| p.join("sandboxes"))
                .unwrap_or_else(|| PathBuf::from("sandboxes"));
            let m = Arc::new(SandboxManager::new(dir));
            // VMs found running after an app restart resume their ssh probing
            m.spawn_ssh_wait_all();
            m
        })
    }

    pub fn new(dir: PathBuf) -> Self {
        let m = Self {
            dir,
            inner: Mutex::new(BTreeMap::new()),
        };
        m.reload();
        m
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn sandbox_dir(&self, id: &str) -> PathBuf {
        self.dir.join(id)
    }

    /// rescan the storage dir; a stored pid that is still alive means the VM
    /// survived an app restart and shows up as running
    pub fn reload(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let mut map = self.inner.lock().unwrap();
        map.clear();
        for ent in entries.flatten() {
            let path = ent.path();
            if !path.is_dir() {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(path.join("sandbox.json")) else {
                continue;
            };
            let Ok(spec) = serde_json::from_str::<SandboxSpec>(&raw) else {
                continue;
            };
            let mut pid = None;
            if let Ok(s) = std::fs::read_to_string(path.join("state.json")) {
                if let Ok(st) = serde_json::from_str::<PersistState>(&s) {
                    pid = st.pid;
                }
            }
            let state = match pid {
                Some(p) if pid_alive(p) => VmState::Running,
                _ => {
                    pid = None;
                    VmState::Stopped
                }
            };
            let ssh = if spec.kind.wants_seed() {
                Some(Arc::new(SshLive::new(&spec.login, spec.ssh_port)))
            } else {
                None
            };
            map.insert(
                spec.id.clone(),
                Entry {
                    spec,
                    state,
                    pid,
                    pid_from_disk: pid.is_some(),
                    prog: None,
                    ssh,
                    error: None,
                    stopping: false,
                },
            );
        }
    }

    pub fn list(&self) -> Vec<SandboxStatus> {
        let map = self.inner.lock().unwrap();
        let mut out: Vec<SandboxStatus> = map
            .values()
            .map(|e| SandboxStatus {
                spec: e.spec.clone(),
                state: e.state,
                pid: e.pid,
                dir: self.dir.join(&e.spec.id).display().to_string(),
                download: e.prog.as_ref().map(|p| p.snapshot()),
                ssh: e.ssh.as_ref().map(|s| s.snapshot()),
                error: e.error.clone(),
            })
            .collect();
        out.sort_by_key(|s| s.spec.created_at);
        out
    }

    fn persist_pid(&self, id: &str, pid: Option<u32>) {
        let dir = self.sandbox_dir(id);
        let Ok(raw) = serde_json::to_string_pretty(&PersistState { pid }) else {
            return;
        };
        let _ = std::fs::write(dir.join("state.json"), raw);
    }

    fn with_entry(&self, id: &str, f: impl FnOnce(&mut Entry)) {
        let mut map = self.inner.lock().unwrap();
        if let Some(e) = map.get_mut(id) {
            f(e);
        }
    }

    /// validate + clamp a wizard request into a spec (no side effects)
    fn prepare_spec(&self, req: &NewSandbox) -> Result<SandboxSpec> {
        let name = req.name.trim();
        if name.is_empty() {
            bail!("name is required");
        }
        if name.len() > 64 {
            bail!("name is too long (max 64 chars)");
        }
        let login = match req.login.as_deref().map(str::trim) {
            None | Some("") => crate::seed::DEFAULT_LOGIN.to_string(),
            Some(l) => {
                crate::seed::validate_login(l)?;
                l.to_string()
            }
        };
        let disk_gib = req.disk_gib.unwrap_or(20).clamp(DISK_MIN, DISK_MAX);
        let ram_mib = req.ram_mib.unwrap_or(2048).clamp(RAM_MIN, RAM_MAX);
        let cpus = req.cpus.unwrap_or(2).clamp(1, CPU_MAX);
        let ssh_port = req.ssh_port.unwrap_or(DEFAULT_PORT).clamp(PORT_MIN, PORT_MAX);
        let map = self.inner.lock().unwrap();
        if map.values().any(|e| e.spec.name == name) {
            bail!("a sandbox named \"{name}\" already exists");
        }
        if let Some(e) = map.values().find(|e| e.spec.ssh_port == ssh_port) {
            bail!(
                "port {ssh_port} is already used by sandbox \"{}\" — pick another",
                e.spec.name
            );
        }
        drop(map);

        if req.kind == ImageKind::Custom {
            let p = req
                .iso_path
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow!("an image file path is required for the own-image kind"))?;
            if !Path::new(p).is_file() {
                bail!("image file not found: {p}");
            }
        }

        let id = {
            let map = self.inner.lock().unwrap();
            unique_id(&map_id_slug(name), |cand| map.contains_key(cand))
        };
        Ok(SandboxSpec {
            id,
            name: name.to_string(),
            login,
            kind: req.kind,
            iso_path: req.iso_path.as_deref().map(str::trim).map(String::from),
            disk_gib,
            ram_mib,
            cpus,
            root: req.root,
            ssh_port,
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        })
    }

    /// create a sandbox from the wizard request; cloud kinds start
    /// downloading immediately (progress via list()) and get their
    /// cloud-init seed written before the first boot can ever happen
    pub fn create(self: &Arc<Self>, req: &NewSandbox) -> Result<SandboxStatus> {
        let spec = self.prepare_spec(req)?;
        let dir = self.sandbox_dir(&spec.id);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("create {}", dir.display()))?;
        let raw = serde_json::to_string_pretty(&spec)?;
        std::fs::write(dir.join("sandbox.json"), raw)?;
        self.persist_pid(&spec.id, None);

        // the seed (login + ssh key + sudo per the root flag) must be on
        // disk before the first boot; a failure surfaces as a failed sandbox
        let seed_err = if spec.kind.wants_seed() {
            crate::seed::ensure_seed(&dir, &spec.id, &spec.login, spec.root).err()
        } else {
            None
        };

        let ssh = if spec.kind.wants_seed() {
            Some(Arc::new(SshLive::new(&spec.login, spec.ssh_port)))
        } else {
            None
        };
        self.inner.lock().unwrap().insert(
            spec.id.clone(),
            Entry {
                spec: spec.clone(),
                state: VmState::Stopped,
                pid: None,
                pid_from_disk: false,
                prog: None,
                ssh,
                error: None,
                stopping: false,
            },
        );

        if let Some(err) = seed_err {
            let msg = format!("{err:#}");
            self.with_entry(&spec.id, |e| {
                e.state = VmState::Failed;
                e.error = Some(msg);
            });
        } else if spec.kind.needs_download() {
            let url = spec.kind.url().unwrap_or_default().to_string();
            let dest = dir.join("image.qcow2");
            self.spawn_download(spec.id.clone(), url, dest);
        }
        Ok(self.status_of(&spec.id)?)
    }

    fn status_of(&self, id: &str) -> Result<SandboxStatus> {
        self.inner
            .lock()
            .unwrap()
            .get(id)
            .map(|e| SandboxStatus {
                spec: e.spec.clone(),
                state: e.state,
                pid: e.pid,
                dir: self.dir.join(id).display().to_string(),
                download: e.prog.as_ref().map(|p| p.snapshot()),
                ssh: e.ssh.as_ref().map(|s| s.snapshot()),
                error: e.error.clone(),
            })
            .ok_or_else(|| anyhow!("sandbox \"{id}\" not found"))
    }

    /// start the VM (or, for a cloud kind whose image is missing, restart
    /// the download — the same button doubles as retry)
    pub fn start(self: &Arc<Self>, id: &str) -> Result<SandboxStatus> {
        let spec = {
            let map = self.inner.lock().unwrap();
            let e = map.get(id).ok_or_else(|| anyhow!("sandbox \"{id}\" not found"))?;
            match e.state {
                VmState::Downloading => bail!("download still in progress"),
                VmState::Running => bail!("sandbox is already running"),
                _ => e.spec.clone(),
            }
        };
        let dir = self.sandbox_dir(id);
        let qemu = detect_qemu();
        if !qemu.ok() {
            bail!("QEMU not found — install it and press re-check");
        }
        let system = PathBuf::from(qemu.system_path.unwrap());
        let img = PathBuf::from(qemu.img_path.unwrap());
        let accel = qemu.accel.unwrap_or_else(|| "tcg".to_string());

        // ensure the disk exists (custom disk images skip this entirely)
        let disk = dir.join("disk.qcow2");
        let custom_disk_boot = spec.kind == ImageKind::Custom && !is_iso_path(spec.iso_path.as_deref());
        if !custom_disk_boot {
            if spec.kind.needs_download() && !dir.join("image.qcow2").is_file() {
                // image not there (deleted / failed download) -> retry the download
                let url = spec.kind.url().unwrap_or_default().to_string();
                self.spawn_download(id.to_string(), url, dir.join("image.qcow2"));
                self.with_entry(id, |e| {
                    e.state = VmState::Downloading;
                    e.error = None;
                });
                return Ok(self.status_of(id)?);
            }
            if !disk.is_file() {
                let out = silent(&mut Command::new(&img).args(build_img_args(spec.kind, &dir, spec.disk_gib)))
                    .output()
                    .map_err(|e| anyhow!("qemu-img: {e}"))?;
                if !out.status.success() {
                    let err = String::from_utf8_lossy(&out.stderr);
                    bail!("qemu-img failed: {}", err.trim());
                }
            }
        }

        // make sure the cloud-init seed exists (heals sandboxes created
        // before phase 2 and ones whose files were wiped; the VM must not
        // boot without it or the login/key never materialize)
        if spec.kind.wants_seed() {
            crate::seed::ensure_seed(&dir, &spec.id, &spec.login, spec.root)
                .map_err(|e| anyhow!("seed: {e:#}"))?;
        }

        let args = build_qemu_args(&spec, &dir, &accel);
        let log = std::fs::File::create(dir.join("qemu.log"))
            .with_context(|| format!("open qemu.log in {}", dir.display()))?;
        let mut cmd = Command::new(&system);
        silent(&mut cmd);
        cmd.args(&args)
            .stdout(Stdio::from(log.try_clone().context("clone log handle")?))
            .stderr(Stdio::from(log));
        let child = cmd
            .spawn()
            .with_context(|| format!("spawn {}", system.display()))?;
        let pid = child.id();
        self.with_entry(id, |e| {
            e.state = VmState::Running;
            e.pid = Some(pid);
            e.pid_from_disk = false;
            e.error = None;
            e.stopping = false;
        });
        self.persist_pid(id, Some(pid));
        self.spawn_watchdog(id.to_string(), child);
        if spec.kind.wants_seed() {
            {
                let mut map = self.inner.lock().unwrap();
                if let Some(e) = map.get_mut(id) {
                    if e.ssh.is_none() {
                        e.ssh = Some(Arc::new(SshLive::new(&spec.login, spec.ssh_port)));
                    }
                }
            }
            self.clone().spawn_ssh_wait(id.to_string());
        }
        self.status_of(id)
    }

    /// kill only when the pid is ours from this session or really a qemu
    /// process (protects against pid reuse after an app restart)
    fn kill_trusted(&self, id: &str, pid: u32) {
        let trusted = {
            let map = self.inner.lock().unwrap();
            map.get(id).map(|e| !e.pid_from_disk).unwrap_or(false)
        };
        if !trusted && !pid_is_qemu(pid) {
            return;
        }
        if pid_alive(pid) {
            let _ = platform_kill(pid);
        }
    }

    /// stop the VM (or clear a failure / cancel nothing else)
    pub fn stop(&self, id: &str) -> Result<SandboxStatus> {
        let pid = {
            let mut map = self.inner.lock().unwrap();
            let e = map.get_mut(id).ok_or_else(|| anyhow!("sandbox \"{id}\" not found"))?;
            e.stopping = true;
            e.state = VmState::Stopped;
            e.error = None;
            if let Some(live) = &e.ssh {
                live.cancel.store(true, Ordering::Relaxed);
                live.set(|i| {
                    i.state = SshState::Idle;
                    i.elapsed_secs = 0;
                });
            }
            let pid = e.pid;
            e.pid = None;
            pid
        };
        if let Some(p) = pid {
            self.kill_trusted(id, p);
        }
        self.persist_pid(id, None);
        self.status_of(id)
    }

    /// delete the sandbox dir; a running VM is stopped first, an in-flight
    /// download is cancelled, ssh/install threads are interrupted
    pub fn delete(self: &Arc<Self>, id: &str) -> Result<()> {
        let pid = {
            let map = self.inner.lock().unwrap();
            let e = map.get(id).ok_or_else(|| anyhow!("sandbox \"{id}\" not found"))?;
            if let Some(p) = &e.prog {
                p.cancel.store(true, Ordering::Relaxed);
            }
            if let Some(live) = &e.ssh {
                live.cancel.store(true, Ordering::Relaxed);
            }
            e.pid
        };
        if let Some(p) = pid {
            self.kill_trusted(id, p);
            // give the OS a moment to release the file handles before rm -rf
            std::thread::sleep(Duration::from_millis(150));
        }
        map_remove(&self.inner, id);
        self.persist_pid(id, None); // keep state.json consistent if dir removal races
        let dir = self.sandbox_dir(id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .with_context(|| format!("remove {}", dir.display()))?;
        }
        Ok(())
    }
}

fn map_remove(map: &Mutex<BTreeMap<String, Entry>>, id: &str) -> bool {
    map.lock().unwrap().remove(id).is_some()
}

// ----------------------------------------------------------------- download

impl SandboxManager {
    /// stream the image to `dest` on a dedicated thread (its own tiny tokio
    /// runtime; reqwest::chunk needs no extra features), reporting progress
    fn spawn_download(self: &Arc<Self>, id: String, url: String, dest: PathBuf) {
        let prog = Arc::new(ProgShared::new(url.clone()));
        self.with_entry(&id, |e| {
            e.state = VmState::Downloading;
            e.error = None;
            e.prog = Some(prog.clone());
        });
        let mgr = self.clone();
        std::thread::spawn(move || {
            let part = dest.with_file_name("image.qcow2.part");
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("runtime: {e}"))
                .and_then(|rt| rt.block_on(download_to_file(&url, &part, &prog)));
            let ok = result.is_ok() && !prog.cancel.load(Ordering::Relaxed);
            prog.done.store(true, Ordering::Relaxed);
            let mut error = result.err();
            if ok {
                if let Err(e) = std::fs::rename(&part, &dest) {
                    error = Some(format!("rename: {e}"));
                }
            } else {
                let _ = std::fs::remove_file(&part);
            }
            let final_state = if ok {
                VmState::Stopped
            } else {
                // a cancelled download belongs to a deleted sandbox in the
                // common case; if the entry survives, allow a clean retry
                match error.as_deref() {
                    Some(e) if e.contains("cancelled") => VmState::Stopped,
                    _ => VmState::Failed,
                }
            };
            mgr.with_entry(&id, |e| {
                e.prog = None;
                e.state = final_state;
                e.error = if final_state == VmState::Failed {
                    Some(
                        error.clone().unwrap_or_else(|| "download failed".to_string()),
                    )
                } else {
                    None
                };
            });
        });
    }
}

async fn download_to_file(url: &str, dest: &Path, prog: &ProgShared) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("client: {e}"))?;
    let mut resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("request: {e}"))?
        .error_for_status()
        .map_err(|e| format!("http: {e}"))?;
    prog.total
        .store(resp.content_length().unwrap_or(0), Ordering::Relaxed);
    let mut file = std::fs::File::create(dest).map_err(|e| format!("create: {e}"))?;
    let mut downloaded: u64 = 0;
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("read: {e}"))?
    {
        if prog.cancel.load(Ordering::Relaxed) {
            return Err("cancelled".to_string());
        }
        file.write_all(&chunk).map_err(|e| format!("write: {e}"))?;
        downloaded += chunk.len() as u64;
        prog.downloaded.store(downloaded, Ordering::Relaxed);
    }
    file.flush().map_err(|e| format!("flush: {e}"))?;
    let _ = file.sync_all();
    Ok(())
}

// -------------------------------------------------------------- vm watchdog

impl SandboxManager {
    /// owns the qemu child; when it exits on its own (crash, busy port,
    /// bad image) the sandbox flips to failed with the qemu.log tail
    fn spawn_watchdog(self: &Arc<Self>, id: String, mut child: std::process::Child) {
        let mgr = self.clone();
        std::thread::spawn(move || {
            let pid = child.id();
            let st = child.wait();
            let reason = match st {
                Ok(s) => match s.code() {
                    Some(0) => "qemu exited cleanly".to_string(),
                    Some(c) => format!("qemu exited with code {c}"),
                    None => "qemu was terminated".to_string(),
                },
                Err(e) => format!("qemu wait failed: {e}"),
            };
            mgr.with_entry(&id, |e| {
                if e.pid != Some(pid) {
                    return; // a newer run owns this entry
                }
                e.pid = None;
                if let Some(live) = &e.ssh {
                    live.cancel.store(true, Ordering::Relaxed);
                    live.set(|i| {
                        i.state = SshState::Idle;
                        i.elapsed_secs = 0;
                    });
                }
                if e.stopping {
                    e.stopping = false;
                    e.state = VmState::Stopped;
                } else {
                    e.state = VmState::Failed;
                    let mut msg = reason;
                    let log = mgr.sandbox_dir(&id).join("qemu.log");
                    let tail = tail_file(&log, 300);
                    if !tail.is_empty() {
                        msg.push_str(": ");
                        msg.push_str(&tail);
                    }
                    e.error = Some(msg);
                }
            });
            mgr.persist_pid(&id, None);
        });
    }
}

// --------------------------------------------------------------- ssh bridge

impl SandboxManager {
    fn ssh_target(dir: &Path, spec: &SandboxSpec) -> crate::sshx::SshTarget {
        crate::sshx::SshTarget::new(dir, spec.ssh_port, &spec.login)
    }

    /// (spec, dir, live ssh state) for a sandbox that has one
    fn ssh_parts(&self, id: &str) -> Option<(SandboxSpec, PathBuf, Arc<SshLive>)> {
        let map = self.inner.lock().unwrap();
        map.get(id).and_then(|e| {
            e.ssh
                .as_ref()
                .map(|s| (e.spec.clone(), self.sandbox_dir(id), s.clone()))
        })
    }

    /// resume ssh probing for every VM that reload() found still running
    /// (the app was restarted under a live guest)
    pub fn spawn_ssh_wait_all(self: &Arc<Self>) {
        let ids: Vec<String> = {
            let map = self.inner.lock().unwrap();
            map.values()
                .filter(|e| e.state == VmState::Running && e.spec.kind.wants_seed())
                .map(|e| e.spec.id.clone())
                .collect()
        };
        for id in ids {
            self.spawn_ssh_wait(id);
        }
    }

    /// background thread: poll the guest sshd until it answers, then probe
    /// for an already-installed agent (the disk survives restarts); the
    /// whole ride is visible on the card as a waiting -> ready badge
    fn spawn_ssh_wait(self: &Arc<Self>, id: String) {
        let mgr = self.clone();
        std::thread::spawn(move || {
            let Some((spec, dir, live)) = mgr.ssh_parts(&id) else {
                return;
            };
            live.cancel.store(false, Ordering::Relaxed);
            live.set(|i| {
                i.state = SshState::Waiting;
                i.elapsed_secs = 0;
                i.error = None;
            });
            let Some(bin) = crate::sshx::find_ssh() else {
                live.set(|i| {
                    i.state = SshState::Failed;
                    i.error = Some(
                        "no ssh client on the host — windows: Settings > Apps > Optional features > OpenSSH client"
                            .to_string(),
                    );
                });
                return;
            };
            let target = Self::ssh_target(&dir, &spec);
            let started = Instant::now();
            let deadline = started + Duration::from_secs(SSH_WAIT_SECS);

            // a 1s ticker keeps the "waiting… Ns" counter alive while the
            // poller blocks on ssh attempts
            let stop_ticker = Arc::new(AtomicBool::new(false));
            let ticker = {
                let live = live.clone();
                let stop = stop_ticker.clone();
                std::thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_secs(1));
                        if stop.load(Ordering::Relaxed) {
                            break;
                        }
                        live.set(|i| {
                            if i.state == SshState::Waiting {
                                i.elapsed_secs = started.elapsed().as_secs();
                            }
                        });
                    }
                })
            };
            let res = crate::sshx::wait_ready(&bin, &target, &live.cancel, deadline);
            stop_ticker.store(true, Ordering::Relaxed);
            let _ = ticker.join();

            match res {
                Err(e) if e == "cancelled" => {
                    live.set(|i| i.state = SshState::Idle);
                }
                Err(e) => {
                    live.set(|i| {
                        i.state = SshState::Failed;
                        i.error = Some(e);
                    });
                }
                Ok(secs) => {
                    live.set(|i| {
                        i.state = SshState::Ready;
                        i.elapsed_secs = secs;
                        i.error = None;
                    });
                    // reflect an agent that is already installed; an install
                    // in flight is never touched by the probe
                    if let Ok(p) = crate::sshx::probe_agent(&bin, &target, &live.cancel) {
                        live.set(|i| {
                            if i.agent.state != AgentState::Installing {
                                if p.present {
                                    i.agent.state = AgentState::Installed;
                                    i.agent.version = p.version;
                                    i.agent.error = None;
                                } else {
                                    i.agent.state = AgentState::Unknown;
                                    i.agent.version = None;
                                }
                            }
                        });
                    }
                }
            }
        });
    }

    fn fail_install(&self, id: &str, msg: String) {
        if let Some((_, _, live)) = self.ssh_parts(id) {
            live.set(|i| {
                i.agent.state = AgentState::Failed;
                i.agent.error = Some(msg.clone());
            });
            live.push_log(&format!("[x] {msg}"));
        }
    }

    /// install (or reinstall) the agent inside the running VM: upload the
    /// host config (the agent needs its provider + api key), stream the
    /// bootstrap script (apt tools per the root flag -> rustup -> cargo
    /// install from the public repo), then verify the binary. Runs on its
    /// own thread; the card follows the log.
    pub fn install_agent(self: &Arc<Self>, id: &str) -> Result<SandboxStatus> {
        let (spec, _dir, live) = self
            .ssh_parts(id)
            .ok_or_else(|| anyhow!("sandbox \"{id}\" has no ssh session"))?;
        {
            let state = self.inner.lock().unwrap().get(id).map(|e| e.state);
            if state != Some(VmState::Running) {
                bail!("start the VM first");
            }
            let info = live.info.lock().unwrap();
            if info.state != SshState::Ready {
                bail!("ssh is not ready yet (state: {:?})", info.state);
            }
            if info.agent.state == AgentState::Installing {
                bail!("agent install is already running");
            }
        }
        live.cancel.store(false, Ordering::Relaxed);
        live.set(|i| {
            i.agent.state = AgentState::Installing;
            i.agent.error = None;
            i.agent.version = None;
            i.agent.log.clear();
        });

        let mgr = self.clone();
        let tid = id.to_string();
        std::thread::spawn(move || {
            let id = tid;
            let Some(bin) = crate::sshx::find_ssh() else {
                mgr.fail_install(&id, "no ssh client on the host".into());
                return;
            };
            let target = Self::ssh_target(&mgr.sandbox_dir(&id), &spec);
            let cancel = &live.cancel;

            // 1. the agent needs the host config (provider + api key) to run
            match std::fs::read_to_string(crate::config::config_path()) {
                Ok(cfg) => {
                    live.push_log("[*] uploading the host config (provider, api key)");
                    let remote = "mkdir -p \"$HOME/.config/hi-derola\" && cat > \"$HOME/.config/hi-derola/config.toml\"";
                    match crate::sshx::stream(&bin, &target, remote, Some(cfg.as_bytes()), cancel, |_| {}) {
                        Ok(0) => {}
                        Ok(c) => {
                            mgr.fail_install(&id, format!("config upload exited with {c}"));
                            return;
                        }
                        Err(e) if e == "cancelled" => {}
                        Err(e) => {
                            mgr.fail_install(&id, format!("config upload: {e}"));
                            return;
                        }
                    }
                }
                Err(_) => {
                    live.push_log("[!] no host config found — the agent will start without an api key");
                }
            }

            // 2. the bootstrap itself
            let res = crate::sshx::stream(
                &bin,
                &target,
                "sh -s 2>&1",
                Some(crate::sshx::INSTALL_SH.as_bytes()),
                cancel,
                |line| live.push_log(line),
            );
            match res {
                Ok(0) => match crate::sshx::probe_agent(&bin, &target, cancel) {
                    Ok(p) if p.present => {
                        live.set(|i| {
                            i.agent.state = AgentState::Installed;
                            i.agent.version = p.version;
                            i.agent.error = None;
                        });
                        live.push_log("[ok] agent is ready — \"run agent\" opens it inside the VM");
                    }
                    Ok(_) => {
                        mgr.fail_install(&id, "install reported success but the binary is missing".into())
                    }
                    Err(e) => mgr.fail_install(&id, format!("verify: {e}")),
                },
                Ok(c) => mgr.fail_install(&id, format!("install script exited with code {c}")),
                Err(e) if e == "cancelled" => {
                    live.set(|i| i.agent.state = AgentState::Unknown);
                    live.push_log("[i] install cancelled");
                }
                Err(e) => mgr.fail_install(&id, e),
            }
        });
        self.status_of(&id)
    }

    /// open a terminal window with an interactive ssh session into the VM
    /// (plain shell, or the agent TUI when `agent` is on)
    pub fn open_terminal(&self, id: &str, agent: bool) -> Result<()> {
        let (spec, dir, live) = self
            .ssh_parts(id)
            .ok_or_else(|| anyhow!("sandbox \"{id}\" has no ssh session"))?;
        if live.info.lock().unwrap().state != SshState::Ready {
            bail!("ssh is not ready yet — wait for the ready badge first");
        }
        let bin = crate::sshx::find_ssh().ok_or_else(|| {
            anyhow!(
                "no ssh client on the host — windows: Settings > Apps > Optional features > OpenSSH client"
            )
        })?;
        let target = Self::ssh_target(&dir, &spec);
        let args = crate::sshx::terminal_cmdline(&target, agent);
        crate::sshx::spawn_terminal(&bin, &args).map_err(|e| anyhow!("{e}"))
    }

    /// run one command in the VM and return its output (power-user path;
    /// later phases route agent tools through this)
    pub fn ssh_exec(
        &self,
        id: &str,
        command: &str,
        timeout_secs: Option<u64>,
    ) -> Result<crate::sshx::SshOut> {
        let (spec, dir, live) = self
            .ssh_parts(id)
            .ok_or_else(|| anyhow!("sandbox \"{id}\" has no ssh session"))?;
        if live.info.lock().unwrap().state != SshState::Ready {
            bail!("ssh is not ready yet");
        }
        let bin =
            crate::sshx::find_ssh().ok_or_else(|| anyhow!("no ssh client on the host"))?;
        let target = Self::ssh_target(&dir, &spec);
        let cancel = AtomicBool::new(false);
        crate::sshx::exec(
            &bin,
            &target,
            command,
            Duration::from_secs(timeout_secs.unwrap_or(15).clamp(1, 300)),
            &cancel,
        )
        .map_err(|e| anyhow!("{e}"))
    }
}

/// last `max` bytes of a file, char-boundary safe, as one trimmed line block
fn tail_file(path: &Path, max: usize) -> String {
    let Ok(raw) = std::fs::read(path) else {
        return String::new();
    };
    if raw.is_empty() {
        return String::new();
    }
    let mut start = raw.len().saturating_sub(max);
    while start < raw.len() && (raw[start] & 0xC0) == 0x80 {
        start += 1;
    }
    let text = String::from_utf8_lossy(&raw[start..]);
    text.lines().map(|l| l.trim_end()).collect::<Vec<_>>().join(" | ")
}

// ------------------------------------------------------------- ids & misc

/// lowercase ascii slug of the sandbox name (fallback "sandbox")
pub fn map_id_slug(name: &str) -> String {
    let mut out = String::new();
    for ch in name.trim().to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('-') {
            out.push('-');
        }
        if out.len() >= 24 {
            break;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "sandbox".to_string()
    } else {
        out
    }
}

/// slug + short random suffix, uniquified with `taken`
fn unique_id(base: &str, mut taken: impl FnMut(&str) -> bool) -> String {
    for _ in 0..64 {
        let mut b = [0u8; 2];
        getrandom::fill(&mut b).expect("os rng");
        let cand = format!("{base}-{:02x}{:02x}", b[0], b[1]);
        if !taken(&cand) {
            return cand;
        }
    }
    format!("{base}-{}", SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0))
}

/// hide the console window a helper process would flash on Windows
pub(crate) fn silent(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    // EPERM means "exists but not ours" — still alive
    let r = unsafe { libc::kill(pid as i32, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// true if the pid belongs to a qemu binary; guards against the OS recycling
/// a stored pid for an unrelated process between app restarts
fn pid_is_qemu(pid: u32) -> bool {
    #[cfg(unix)]
    {
        std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .map(|s| s.trim().starts_with("qemu"))
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        let Ok(out) =
            silent(&mut Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"]))
                .output()
        else {
            return false;
        };
        String::from_utf8_lossy(&out.stdout).to_lowercase().contains("qemu")
    }
}

#[cfg(windows)]
fn pid_alive(pid: u32) -> bool {
    let Ok(out) = silent(&mut Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH"]))
        .output()
    else {
        return false;
    };
    let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
    !text.contains("info:") && text.contains(&pid.to_string())
}

#[cfg(unix)]
fn platform_kill(pid: u32) -> Result<(), String> {
    let r = unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    if r != 0 {
        return Err(format!("kill: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(windows)]
fn platform_kill(pid: u32) -> Result<(), String> {
    let out = silent(&mut Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]))
        .output()
        .map_err(|e| format!("taskkill: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "taskkill failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "hiderola-sbx-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn req(name: &str, kind: ImageKind) -> NewSandbox {
        NewSandbox {
            name: name.to_string(),
            kind,
            login: None,
            iso_path: None,
            disk_gib: None,
            ram_mib: None,
            cpus: None,
            root: false,
            ssh_port: None,
        }
    }

    fn spec(name: &str, kind: ImageKind, iso: Option<&str>) -> SandboxSpec {
        SandboxSpec {
            id: "test-01".into(),
            name: name.into(),
            login: "derola".into(),
            kind,
            iso_path: iso.map(String::from),
            disk_gib: 20,
            ram_mib: 2048,
            cpus: 2,
            root: false,
            ssh_port: 2222,
            created_at: 0,
        }
    }

    #[test]
    fn image_kinds_roundtrip_and_urls() {
        assert_eq!(
            serde_json::to_string(&ImageKind::DebianTrixie).unwrap(),
            "\"debian-trixie\""
        );
        assert_eq!(
            serde_json::to_string(&ImageKind::Ubuntu2404).unwrap(),
            "\"ubuntu-24.04\""
        );
        assert_eq!(serde_json::to_string(&ImageKind::Nixos).unwrap(), "\"nixos\"");
        assert_eq!(serde_json::to_string(&ImageKind::Custom).unwrap(), "\"custom\"");
        let k: ImageKind = serde_json::from_str("\"ubuntu-24.04\"").unwrap();
        assert_eq!(k, ImageKind::Ubuntu2404);
        // legacy kinds keep loading
        let k: ImageKind = serde_json::from_str("\"nixos\"").unwrap();
        assert_eq!(k, ImageKind::Nixos);
        assert_eq!(ImageKind::DebianTrixie.url(), Some(DEBIAN_TRIXIE_URL));
        assert_eq!(ImageKind::Ubuntu2404.url(), Some(UBUNTU_2404_URL));
        assert!(ImageKind::DebianTrixie.needs_download());
        assert!(ImageKind::Ubuntu2404.needs_download());
        assert!(!ImageKind::Custom.needs_download());
        assert!(DEBIAN_TRIXIE_URL.starts_with("https://cloud.debian.org/images/cloud/trixie/"));
        assert!(UBUNTU_2404_URL.starts_with("https://cloud-images.ubuntu.com/minimal/releases/24.04/"));
        assert!(UBUNTU_2404_URL.ends_with("ubuntu-24.04-minimal-cloudimg-amd64.img"));
    }

    #[test]
    fn seed_only_for_cloud_init_kinds() {
        assert!(ImageKind::DebianTrixie.wants_seed());
        assert!(ImageKind::Ubuntu2404.wants_seed());
        // no cloud-init -> no seed/ssh wiring
        assert!(!ImageKind::Nixos.wants_seed());
        assert!(!ImageKind::Custom.wants_seed());
    }

    #[test]
    fn version_and_accel_parsing() {
        let v = parse_version_line("QEMU emulator version 8.2.0 (Debian 1:8.2.0+ds-1)\nCopyright (c) 2003-2023");
        assert!(v.unwrap().contains("8.2.0"));
        assert_eq!(parse_version_line("no version here"), None);
        assert_eq!(parse_version_line(""), None);
        assert_eq!(parse_accel_list("Accelerators supported with machine default:\nwhpx\ntcg\n"), "whpx");
        assert_eq!(parse_accel_list("kvm\ntcg\n"), "kvm");
        assert_eq!(parse_accel_list("hvf\ntcg\n"), "hvf");
        assert_eq!(parse_accel_list("tcg\n"), "tcg");
        assert_eq!(parse_accel_list(""), "tcg", "fallback keeps qemu bootable");
    }

    #[test]
    fn qemu_args_cloud_iso_and_custom_disk() {
        let dir = Path::new("/vm/test");

        let cloud = spec("deb", ImageKind::DebianTrixie, None);
        let a = build_qemu_args(&cloud, dir, "whpx");
        let s = a.join(" ");
        assert!(s.contains("-machine q35"));
        assert!(s.contains("-accel whpx"));
        assert!(s.contains("-m 2048"));
        assert!(s.contains("-smp 2"));
        assert!(s.contains("file=/vm/test/disk.qcow2,format=qcow2,if=virtio"));
        // loopback bind: the guest ssh is host-local only
        assert!(s.contains("hostfwd=tcp:127.0.0.1:2222-:22"));
        assert!(!s.contains("hostfwd=tcp::2222"));
        assert!(s.contains("-device virtio-net-pci,netdev=n0"));
        assert!(s.contains("-name hiderola-test-01"));
        assert!(!s.contains("-cpu max"), "hw accel keeps the default cpu");
        assert!(!s.contains("-cdrom"));
        // the cloud-init seed rides along as a second raw disk
        assert!(s.contains("file=/vm/test/seed.img,format=raw,if=virtio"));

        let ubuntu = spec("ub", ImageKind::Ubuntu2404, None);
        let a = build_qemu_args(&ubuntu, dir, "kvm");
        assert!(a.join(" ").contains("file=/vm/test/seed.img,format=raw,if=virtio"));

        let iso = spec("ins", ImageKind::Custom, Some("/imgs/debian.iso"));
        let a = build_qemu_args(&iso, dir, "tcg");
        let s = a.join(" ");
        assert!(s.contains("-cdrom /imgs/debian.iso"));
        assert!(s.contains("-boot d"));
        assert!(s.contains("file=/vm/test/disk.qcow2,format=qcow2,if=virtio"));
        assert!(s.contains("-cpu max"), "tcg gets the widest cpu model");
        assert!(!s.contains("seed.img"), "custom iso has no seed drive");

        let diskimg = spec("own", ImageKind::Custom, Some("/imgs/preinstalled.qcow2"));
        let a = build_qemu_args(&diskimg, dir, "kvm");
        let s = a.join(" ");
        assert!(s.contains("file=/imgs/preinstalled.qcow2,format=auto,if=virtio"));
        assert!(!s.contains("disk.qcow2"), "own disk image boots directly");
        assert!(!s.contains("-boot d"));
        assert!(!s.contains("seed.img"));

        // legacy nixos boots without a seed (no cloud-init inside)
        let nix = spec("nix", ImageKind::Nixos, None);
        let a = build_qemu_args(&nix, dir, "kvm");
        assert!(!a.join(" ").contains("seed.img"));
    }

    #[test]
    fn img_args_overlay_vs_plain() {
        let dir = Path::new("/vm/test");
        for kind in [ImageKind::DebianTrixie, ImageKind::Ubuntu2404, ImageKind::Nixos] {
            let overlay = build_img_args(kind, dir, 20);
            let s = overlay.join(" ");
            assert!(s.starts_with("create -f qcow2"));
            assert!(s.contains("-b /vm/test/image.qcow2"));
            assert!(s.contains("-F qcow2"));
            assert!(s.contains("/vm/test/disk.qcow2"));
            assert!(s.ends_with("20G"));
        }

        let plain = build_img_args(ImageKind::Custom, dir, 8);
        let s = plain.join(" ");
        assert!(!s.contains("-b"), "custom disk has no backing image");
        assert!(s.ends_with("8G"));
    }

    #[test]
    fn slug_and_unique_ids() {
        assert_eq!(map_id_slug("My Cool Sandbox!"), "my-cool-sandbox");
        assert_eq!(map_id_slug("---///###"), "sandbox");
        assert_eq!(map_id_slug(""), "sandbox");
        let long = map_id_slug("a very long sandbox name that goes on and on forever");
        assert!(long.len() <= 24, "{long}");
        assert!(!long.starts_with('-') && !long.ends_with('-'), "{long}");

        let id = unique_id("test", |_| false);
        assert!(id.starts_with("test-"));
        let mut calls = 0;
        let id = unique_id("test", |_| {
            calls += 1;
            calls <= 2 // first two candidates pretend to be taken
        });
        assert!(id.starts_with("test-"));
        assert_eq!(calls, 3);
    }

    #[test]
    fn create_list_delete_roundtrip() {
        let dir = temp_dir("roundtrip");
        let mgr = Arc::new(SandboxManager::new(dir.clone()));
        let iso = dir.join("mini.iso");
        std::fs::write(&iso, b"fake iso").unwrap();

        let st = mgr
            .create(&NewSandbox {
                name: "Test Box".into(),
                kind: ImageKind::Custom,
                login: None,
                iso_path: Some(iso.display().to_string()),
                disk_gib: Some(1000), // clamped
                ram_mib: Some(100),   // clamped
                cpus: Some(99),       // clamped
                root: true,
                ssh_port: Some(3000),
            })
            .unwrap();
        assert_eq!(st.spec.disk_gib, DISK_MAX);
        assert_eq!(st.spec.ram_mib, RAM_MIN);
        assert_eq!(st.spec.cpus, CPU_MAX);
        assert!(st.spec.root);
        assert_eq!(st.spec.ssh_port, 3000);
        assert_eq!(st.state, VmState::Stopped);
        assert!(dir.join(&st.spec.id).join("sandbox.json").is_file());
        assert_eq!(mgr.list().len(), 1);

        // duplicate name refused (checked before the iso path check)
        assert!(mgr.create(&req("Test Box", ImageKind::Custom)).is_err());
        // duplicate port refused
        let mut r2 = req("other", ImageKind::Custom);
        r2.iso_path = Some(iso.display().to_string());
        r2.ssh_port = Some(3000);
        assert!(mgr.create(&r2).is_err());
        // missing image file refused
        let mut r3 = req("noiso", ImageKind::Custom);
        r3.iso_path = Some("/definitely/not/here.iso".into());
        assert!(mgr.create(&r3).is_err());
        // empty name refused
        assert!(mgr.create(&req("   ", ImageKind::Custom)).is_err());

        mgr.delete(&st.spec.id).unwrap();
        assert!(mgr.list().is_empty());
        assert!(!dir.join(&st.spec.id).exists());
        // double delete reports cleanly
        assert!(mgr.delete(&st.spec.id).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cloud_create_smoke() {
        // network may or may not exist here; only assert the entry lands in
        // the manager with its wizard metadata persisted
        let dir = temp_dir("cloud");
        let mgr = Arc::new(SandboxManager::new(dir.clone()));
        let st = mgr.create(&req("deb box", ImageKind::DebianTrixie)).unwrap();
        assert!(matches!(
            st.state,
            VmState::Downloading | VmState::Failed | VmState::Stopped
        ));
        assert_eq!(st.spec.kind, ImageKind::DebianTrixie);
        assert_eq!(st.spec.disk_gib, 20);
        assert!(dir.join(&st.spec.id).join("sandbox.json").is_file());
        let _ = mgr.delete(&st.spec.id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cloud_create_generates_seed_keypair_and_ssh_state() {
        let dir = temp_dir("seed-create");
        let mgr = Arc::new(SandboxManager::new(dir.clone()));
        let st = mgr.create(&req("seeded", ImageKind::Ubuntu2404)).unwrap();
        let sdir = dir.join(&st.spec.id);
        // the seed must be on disk before the first boot can happen
        assert!(sdir.join("seed.img").is_file());
        assert!(sdir.join("id_ed25519").is_file());
        assert!(sdir.join("id_ed25519.pub").is_file());
        let pub_line = std::fs::read_to_string(sdir.join("id_ed25519.pub")).unwrap();
        assert!(pub_line.starts_with("ssh-ed25519 "));
        assert!(pub_line.contains("hiderola-"));
        // ssh snapshot is part of the status
        let st = mgr.list().into_iter().next().unwrap();
        let ssh = st.ssh.expect("seed kinds carry ssh info");
        assert_eq!(ssh.user, "derola");
        assert_eq!(ssh.port, 2222);
        assert_eq!(ssh.agent.state, AgentState::Unknown);
        assert!(st.error.is_none() || st.state == VmState::Failed);
        let _ = mgr.delete(&st.spec.id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn custom_create_has_no_seed_or_ssh_state() {
        let dir = temp_dir("noseed");
        let mgr = Arc::new(SandboxManager::new(dir.clone()));
        let iso = dir.join("mini.iso");
        std::fs::write(&iso, b"fake").unwrap();
        let mut r = req("plain", ImageKind::Custom);
        r.iso_path = Some(iso.display().to_string());
        let st = mgr.create(&r).unwrap();
        assert!(!dir.join(&st.spec.id).join("seed.img").exists());
        assert!(st.ssh.is_none(), "custom kinds carry no ssh state");
        let _ = mgr.delete(&st.spec.id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn login_defaults_and_validation() {
        let dir = temp_dir("login");
        let mgr = Arc::new(SandboxManager::new(dir.clone()));
        let iso = dir.join("a.iso");
        std::fs::write(&iso, b"x").unwrap();

        // default login kicks in
        let mut r = req("dflt", ImageKind::Custom);
        r.iso_path = Some(iso.display().to_string());
        let st = mgr.create(&r).unwrap();
        assert_eq!(st.spec.login, "derola");
        let _ = mgr.delete(&st.spec.id);

        // valid custom login passes through (trim included)
        let mut r = req("custom", ImageKind::Custom);
        r.iso_path = Some(iso.display().to_string());
        r.login = Some("  agent-1 ".into());
        let st = mgr.create(&r).unwrap();
        assert_eq!(st.spec.login, "agent-1");
        let _ = mgr.delete(&st.spec.id);

        // blank login falls back to the default
        let mut r = req("blank", ImageKind::Custom);
        r.iso_path = Some(iso.display().to_string());
        r.login = Some("   ".into());
        let st = mgr.create(&r).unwrap();
        assert_eq!(st.spec.login, "derola");
        let _ = mgr.delete(&st.spec.id);

        // invalid logins are refused before anything is written;
        // empty/blank logins are NOT errors — they fall back to the default
        for bad in ["root", "Root", "9lives", "has space", "x".repeat(33).as_str()] {
            let mut r = req("bad", ImageKind::Custom);
            r.iso_path = Some(iso.display().to_string());
            r.login = Some(bad.to_string());
            assert!(mgr.create(&r).is_err(), "{bad:?} must be refused");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_gates() {
        let dir = temp_dir("gates");
        let mgr = Arc::new(SandboxManager::new(dir.clone()));
        // unknown sandbox
        assert!(mgr.install_agent("missing").is_err());
        // known but not running
        let st = mgr.create(&req("gated", ImageKind::DebianTrixie)).unwrap();
        assert!(mgr.install_agent(&st.spec.id).is_err());
        let _ = mgr.delete(&st.spec.id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reload_detects_state_and_dead_pid() {
        let dir = temp_dir("reload");
        let mgr = Arc::new(SandboxManager::new(dir.clone()));
        let iso = dir.join("a.iso");
        std::fs::write(&iso, b"x").unwrap();
        let mut r = req("reload-me", ImageKind::Custom);
        r.iso_path = Some(iso.display().to_string());
        let st = mgr.create(&r).unwrap();
        let id = st.spec.id.clone();

        // a fresh manager instance picks the sandbox up from disk
        let mgr2 = Arc::new(SandboxManager::new(dir.clone()));
        let items = mgr2.list();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].state, VmState::Stopped);

        // dead pid -> stopped
        mgr2.persist_pid(&id, Some(4_000_000_000));
        mgr2.reload();
        assert_eq!(mgr2.list()[0].state, VmState::Stopped);

        // live pid (this test process) -> running
        std::fs::write(
            dir.join(&id).join("state.json"),
            format!("{{\"pid\":{}}}", std::process::id()),
        )
        .unwrap();
        mgr2.reload();
        let items = mgr2.list();
        assert_eq!(items[0].state, VmState::Running);
        assert_eq!(items[0].pid, Some(std::process::id()));

        let _ = mgr2.delete(&id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pid_alive_rejects_bogus_pid() {
        assert!(!pid_alive(4_000_000_000));
    }

    #[test]
    fn sandbox_status_json_shape_matches_gui() {
        // the GUI reads s.spec.*, s.state, s.download, s.error — lock the shape
        let st = SandboxStatus {
            spec: spec("gui", ImageKind::Custom, Some("/x.iso")),
            state: VmState::Stopped,
            pid: None,
            dir: "/d/test-01".into(),
            download: None,
            ssh: None,
            error: None,
        };
        let v: serde_json::Value = serde_json::to_value(&st).unwrap();
        assert_eq!(v["spec"]["name"], "gui");
        assert_eq!(v["spec"]["kind"], "custom");
        assert_eq!(v["spec"]["login"], "derola");
        assert_eq!(v["spec"]["ssh_port"], 2222);
        assert_eq!(v["state"], "stopped");
        assert_eq!(v["dir"], "/d/test-01");
        assert!(v.get("download").is_none());
        assert!(v.get("ssh").is_none());
        assert!(v.get("error").is_none());
        let v: serde_json::Value =
            serde_json::to_value(&st.clone()).unwrap();
        assert!(v["spec"].is_object());
    }

    #[test]
    fn ssh_info_json_shape_matches_gui() {
        // the GUI reads s.ssh.state/.user/.port/.elapsed_secs/.agent.*
        let info = SshInfo {
            state: SshState::Ready,
            user: "derola".into(),
            port: 2222,
            elapsed_secs: 37,
            error: None,
            agent: AgentInfo {
                state: AgentState::Installed,
                version: Some("0.1.0".into()),
                error: None,
                log: vec!["[ok] done".into()],
            },
        };
        let v: serde_json::Value = serde_json::to_value(&info).unwrap();
        assert_eq!(v["state"], "ready");
        assert_eq!(v["user"], "derola");
        assert_eq!(v["port"], 2222);
        assert_eq!(v["elapsed_secs"], 37);
        assert!(v.get("error").is_none());
        assert_eq!(v["agent"]["state"], "installed");
        assert_eq!(v["agent"]["version"], "0.1.0");
        assert_eq!(v["agent"]["log"], serde_json::json!(["[ok] done"]));

        let info = SshInfo {
            state: SshState::Waiting,
            user: "joe".into(),
            port: 2200,
            elapsed_secs: 3,
            error: None,
            agent: AgentInfo {
                state: AgentState::Unknown,
                version: None,
                error: None,
                log: Vec::new(),
            },
        };
        let v: serde_json::Value = serde_json::to_value(&info).unwrap();
        assert_eq!(v["state"], "waiting");
        assert_eq!(v["agent"]["state"], "unknown");
        assert!(v["agent"].get("version").is_none());
        assert!(v["agent"].get("log").is_none());

        // a failed wait carries the error
        let info = SshInfo {
            state: SshState::Failed,
            user: "joe".into(),
            port: 2200,
            elapsed_secs: 600,
            error: Some("ssh did not answer".into()),
            agent: AgentInfo {
                state: AgentState::Failed,
                version: None,
                error: Some("install script exited with code 33".into()),
                log: Vec::new(),
            },
        };
        let v: serde_json::Value = serde_json::to_value(&info).unwrap();
        assert_eq!(v["state"], "failed");
        assert_eq!(v["error"], "ssh did not answer");
        assert_eq!(v["agent"]["state"], "failed");
        assert_eq!(v["agent"]["error"], "install script exited with code 33");
    }

    #[test]
    fn tail_file_truncates_safely() {
        let dir = temp_dir("tail");
        let p = dir.join("qemu.log");
        std::fs::write(&p, "a".repeat(1000)).unwrap();
        assert_eq!(tail_file(&p, 300).len(), 300);
        std::fs::write(&p, "привет ".repeat(100)).unwrap();
        let t = tail_file(&p, 10);
        assert!(!t.is_empty());
        assert!(tail_file(&dir.join("missing.log"), 100).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
