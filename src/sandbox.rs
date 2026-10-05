//! Local sandbox: disposable QEMU virtual machines so the agent (and the
//! user's files) can live away from the host machine.
//!
//! Phase 1 covers the full lifecycle around the VM itself: QEMU detection,
//! a creation wizard (image -> resources -> create) with resumable-ish
//! downloads and progress reporting, and start/stop/delete backed by a
//! small on-disk store. Booting an agent-ready OS (cloud-init seed images,
//! SSH key injection) is the next phase.
//!
//! Storage layout under `<config dir>/hi-derola/sandboxes/`:
//! ```text
//! <id>/sandbox.json   the SandboxSpec the wizard produced
//! <id>/state.json     { "pid": <u32|null> } — qemu pid for crash recovery
//! <id>/image.qcow2    downloaded cloud image (debian / nixos kinds)
//! <id>/disk.qcow2     the VM disk (overlay on top of image.qcow2)
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
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Debian 13 "trixie" minimal cloud image (genericcloud — the smallest
/// variant, built for virtual machines; boots both BIOS and UEFI)
pub const DEBIAN_TRIXIE_URL: &str =
    "https://cloud.debian.org/images/cloud/trixie/latest/debian-13-genericcloud-amd64.qcow2";
/// NixOS minimal qcow2 from the current stable channel
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImageKind {
    DebianTrixie,
    Nixos,
    /// user-provided image file: .iso boots as install media (cdrom),
    /// anything else (qcow2/raw/vdi/...) is used as the VM disk directly
    Custom,
}

impl ImageKind {
    pub fn label(self) -> &'static str {
        match self {
            ImageKind::DebianTrixie => "Debian 13 (trixie) minimal",
            ImageKind::Nixos => "NixOS minimal",
            ImageKind::Custom => "own image",
        }
    }

    pub fn url(self) -> Option<&'static str> {
        match self {
            ImageKind::DebianTrixie => Some(DEBIAN_TRIXIE_URL),
            ImageKind::Nixos => Some(NIXOS_URL),
            ImageKind::Custom => None,
        }
    }

    pub fn needs_download(self) -> bool {
        self.url().is_some()
    }
}

/// the VM shape the wizard produces; persisted verbatim as sandbox.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxSpec {
    pub id: String,
    pub name: String,
    pub kind: ImageKind,
    /// absolute path to the user-provided image (kind = custom)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iso_path: Option<String>,
    pub disk_gib: u32,
    pub ram_mib: u32,
    pub cpus: u32,
    /// reserved for the agent profile: whether the in-VM agent may run as
    /// root (cloud-init wiring arrives in the next phase)
    #[serde(default)]
    pub root: bool,
    /// host port forwarded to guest ssh (22)
    pub ssh_port: u16,
    pub created_at: u64,
}

/// wizard request; every resource field is optional and clamped
#[derive(Debug, Clone, Deserialize)]
pub struct NewSandbox {
    pub name: String,
    pub kind: ImageKind,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
        }
    }

    a.extend([
        "-netdev".into(),
        format!("user,id=n0,hostfwd=tcp::{}-:22", spec.ssh_port),
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
        ImageKind::DebianTrixie | ImageKind::Nixos => vec![
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
            Arc::new(SandboxManager::new(dir))
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
            map.insert(
                spec.id.clone(),
                Entry {
                    spec,
                    state,
                    pid,
                    pid_from_disk: pid.is_some(),
                    prog: None,
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
    /// downloading immediately (progress via list())
    pub fn create(self: &Arc<Self>, req: &NewSandbox) -> Result<SandboxStatus> {
        let spec = self.prepare_spec(req)?;
        let dir = self.sandbox_dir(&spec.id);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("create {}", dir.display()))?;
        let raw = serde_json::to_string_pretty(&spec)?;
        std::fs::write(dir.join("sandbox.json"), raw)?;
        self.persist_pid(&spec.id, None);
        self.inner.lock().unwrap().insert(
            spec.id.clone(),
            Entry {
                spec: spec.clone(),
                state: VmState::Stopped,
                pid: None,
                pid_from_disk: false,
                prog: None,
                error: None,
                stopping: false,
            },
        );

        let state = if spec.kind.needs_download() {
            let url = spec.kind.url().unwrap_or_default().to_string();
            let dest = dir.join("image.qcow2");
            self.spawn_download(spec.id.clone(), url, dest);
            VmState::Downloading
        } else {
            VmState::Stopped
        };
        self.with_entry(&spec.id, |e| {
            e.state = state;
        });
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
    /// download is cancelled
    pub fn delete(self: &Arc<Self>, id: &str) -> Result<()> {
        let pid = {
            let map = self.inner.lock().unwrap();
            let e = map.get(id).ok_or_else(|| anyhow!("sandbox \"{id}\" not found"))?;
            if let Some(p) = &e.prog {
                p.cancel.store(true, Ordering::Relaxed);
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
fn silent(cmd: &mut Command) -> &mut Command {
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
        assert_eq!(serde_json::to_string(&ImageKind::Nixos).unwrap(), "\"nixos\"");
        assert_eq!(serde_json::to_string(&ImageKind::Custom).unwrap(), "\"custom\"");
        let k: ImageKind = serde_json::from_str("\"nixos\"").unwrap();
        assert_eq!(k, ImageKind::Nixos);
        assert_eq!(ImageKind::DebianTrixie.url(), Some(DEBIAN_TRIXIE_URL));
        assert!(ImageKind::DebianTrixie.needs_download());
        assert!(!ImageKind::Custom.needs_download());
        assert!(DEBIAN_TRIXIE_URL.starts_with("https://cloud.debian.org/images/cloud/trixie/"));
        assert!(NIXOS_URL.starts_with("https://channels.nixos.org/nixos-"));
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
        assert!(s.contains("hostfwd=tcp::2222-:22"));
        assert!(s.contains("-device virtio-net-pci,netdev=n0"));
        assert!(s.contains("-name hiderola-test-01"));
        assert!(!s.contains("-cpu max"), "hw accel keeps the default cpu");
        assert!(!s.contains("-cdrom"));

        let iso = spec("ins", ImageKind::Custom, Some("/imgs/debian.iso"));
        let a = build_qemu_args(&iso, dir, "tcg");
        let s = a.join(" ");
        assert!(s.contains("-cdrom /imgs/debian.iso"));
        assert!(s.contains("-boot d"));
        assert!(s.contains("file=/vm/test/disk.qcow2,format=qcow2,if=virtio"));
        assert!(s.contains("-cpu max"), "tcg gets the widest cpu model");

        let diskimg = spec("own", ImageKind::Custom, Some("/imgs/preinstalled.qcow2"));
        let a = build_qemu_args(&diskimg, dir, "kvm");
        let s = a.join(" ");
        assert!(s.contains("file=/imgs/preinstalled.qcow2,format=auto,if=virtio"));
        assert!(!s.contains("disk.qcow2"), "own disk image boots directly");
        assert!(!s.contains("-boot d"));
    }

    #[test]
    fn img_args_overlay_vs_plain() {
        let dir = Path::new("/vm/test");
        let overlay = build_img_args(ImageKind::Nixos, dir, 20);
        let s = overlay.join(" ");
        assert!(s.starts_with("create -f qcow2"));
        assert!(s.contains("-b /vm/test/image.qcow2"));
        assert!(s.contains("-F qcow2"));
        assert!(s.contains("/vm/test/disk.qcow2"));
        assert!(s.ends_with("20G"));

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
            error: None,
        };
        let v: serde_json::Value = serde_json::to_value(&st).unwrap();
        assert_eq!(v["spec"]["name"], "gui");
        assert_eq!(v["spec"]["kind"], "custom");
        assert_eq!(v["spec"]["ssh_port"], 2222);
        assert_eq!(v["state"], "stopped");
        assert_eq!(v["dir"], "/d/test-01");
        assert!(v.get("download").is_none());
        assert!(v.get("error").is_none());
        let v: serde_json::Value =
            serde_json::to_value(&st.clone()).unwrap();
        assert!(v["spec"].is_object());
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
