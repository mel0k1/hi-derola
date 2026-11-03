use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const DEBIAN_TRIXIE_URL: &str =
    "https://cloud.debian.org/images/cloud/trixie/latest/debian-13-genericcloud-amd64.qcow2";

pub const DEBIAN_TRIXIE_STD_URL: &str =
    "https://cloud.debian.org/images/cloud/trixie/latest/debian-13-generic-amd64.qcow2";

pub const UBUNTU_2404_URL: &str = "https://cloud-images.ubuntu.com/minimal/releases/24.04/release/ubuntu-24.04-minimal-cloudimg-amd64.img";

pub const UBUNTU_2404_STD_URL: &str =
    "https://cloud-images.ubuntu.com/releases/24.04/release/ubuntu-24.04-server-cloudimg-amd64.img";

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

pub const SSH_WAIT_SECS: u64 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImageKind {
    DebianTrixie,
    #[serde(rename = "debian-trixie-std")]
    DebianTrixieStd,
    #[serde(rename = "ubuntu-24.04")]
    Ubuntu2404,
    #[serde(rename = "ubuntu-24.04-std")]
    Ubuntu2404Std,

    Nixos,

    Custom,
}

impl ImageKind {
    pub fn label(self) -> &'static str {
        match self {
            ImageKind::DebianTrixie => "Debian 13 (trixie) minimal",
            ImageKind::DebianTrixieStd => "Debian 13 (trixie) standard",
            ImageKind::Ubuntu2404 => "Ubuntu 24.04 LTS minimal",
            ImageKind::Ubuntu2404Std => "Ubuntu 24.04 LTS standard",
            ImageKind::Nixos => "NixOS minimal",
            ImageKind::Custom => "own image",
        }
    }

    pub fn url(self) -> Option<&'static str> {
        match self {
            ImageKind::DebianTrixie => Some(DEBIAN_TRIXIE_URL),
            ImageKind::DebianTrixieStd => Some(DEBIAN_TRIXIE_STD_URL),
            ImageKind::Ubuntu2404 => Some(UBUNTU_2404_URL),
            ImageKind::Ubuntu2404Std => Some(UBUNTU_2404_STD_URL),
            ImageKind::Nixos => Some(NIXOS_URL),
            ImageKind::Custom => None,
        }
    }

    pub fn needs_download(self) -> bool {
        self.url().is_some()
    }

    pub fn wants_seed(self) -> bool {
        matches!(
            self,
            ImageKind::DebianTrixie
                | ImageKind::DebianTrixieStd
                | ImageKind::Ubuntu2404
                | ImageKind::Ubuntu2404Std
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PortFwd {
    pub host_port: u16,
    pub guest_port: u16,
    #[serde(default = "default_fwd_host")]
    pub guest_host: String,
}

fn default_fwd_host() -> String {
    "127.0.0.1".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxSpec {
    pub id: String,
    pub name: String,

    #[serde(default = "default_login")]
    pub login: String,
    pub kind: ImageKind,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iso_path: Option<String>,
    pub disk_gib: u32,
    pub ram_mib: u32,
    pub cpus: u32,

    #[serde(default)]
    pub root: bool,

    pub ssh_port: u16,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forwards: Vec<PortFwd>,
    pub created_at: u64,
}

fn default_login() -> String {
    crate::seed::DEFAULT_LOGIN.to_string()
}

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

#[derive(Debug, Clone, Serialize)]
pub struct QemuInfo {
    pub system_path: Option<String>,
    pub system_version: Option<String>,
    pub img_path: Option<String>,
    pub img_version: Option<String>,

    pub accel: Option<String>,
}

impl QemuInfo {
    pub fn ok(&self) -> bool {
        self.system_path.is_some() && self.img_path.is_some()
    }
}

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
    pub ssh: Option<SshInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SshState {
    Idle,

    Waiting,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentState {
    Unknown,
    Installing,
    Installed,
    Failed,
}

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

#[derive(Debug, Clone, Serialize)]
pub struct SshInfo {
    pub state: SshState,
    pub user: String,
    pub port: u16,

    pub elapsed_secs: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub agent: AgentInfo,
}

#[derive(Debug)]
pub struct SshLive {
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
    let out = silent(&mut Command::new(path).arg("--version"))
        .output()
        .ok()?;
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

pub fn build_qemu_args(spec: &SandboxSpec, dir: &Path, accel: &str) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-machine".into(),
        "q35".into(),
        "-accel".into(),
        accel.into(),
    ];

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
        format!(
            "user,id=n0,hostfwd=tcp:127.0.0.1:{}-:22{}",
            spec.ssh_port,
            spec.forwards
                .iter()
                .map(|f| {
                    format!(
                        ",hostfwd=tcp:127.0.0.1:{}-{}:{}",
                        f.host_port, f.guest_host, f.guest_port
                    )
                })
                .collect::<String>()
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

pub fn build_img_args(kind: ImageKind, dir: &Path, disk_gib: u32) -> Vec<String> {
    let disk = dir.join("disk.qcow2");
    match kind {
        ImageKind::DebianTrixie
        | ImageKind::DebianTrixieStd
        | ImageKind::Ubuntu2404
        | ImageKind::Ubuntu2404Std
        | ImageKind::Nixos => vec![
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

    pid_from_disk: bool,
    prog: Option<Arc<ProgShared>>,

    ssh: Option<Arc<SshLive>>,
    error: Option<String>,

    stopping: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistState {
    #[serde(default)]
    pid: Option<u32>,
}

pub struct SandboxManager {
    dir: PathBuf,
    inner: Mutex<BTreeMap<String, Entry>>,
}

#[cfg(test)]
static TEST_DIR: OnceLock<PathBuf> = OnceLock::new();

#[cfg(test)]
pub fn test_global_dir(dir: PathBuf) {
    let _ = TEST_DIR.set(dir);
}

impl SandboxManager {
    pub fn global() -> &'static Arc<SandboxManager> {
        static M: OnceLock<Arc<SandboxManager>> = OnceLock::new();
        M.get_or_init(|| {
            let dir = Self::default_dir();
            #[cfg(test)]
            let dir = TEST_DIR.get().cloned().unwrap_or(dir);
            let m = Arc::new(SandboxManager::new(dir));

            m.spawn_ssh_wait_all();
            m
        })
    }

    fn default_dir() -> PathBuf {
        crate::config::config_path()
            .parent()
            .map(|p| p.join("sandboxes"))
            .unwrap_or_else(|| PathBuf::from("sandboxes"))
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

    fn persist_spec(&self, id: &str) {
        let dir = self.sandbox_dir(id);
        let spec = {
            let map = self.inner.lock().unwrap();
            map.get(id).map(|e| e.spec.clone())
        };
        if let Some(spec) = spec {
            if let Ok(raw) = serde_json::to_string_pretty(&spec) {
                let _ = std::fs::write(dir.join("sandbox.json"), raw);
            }
        }
    }

    pub fn fwd_add(
        &self,
        id: &str,
        host_port: u16,
        guest_port: u16,
        guest_host: Option<&str>,
    ) -> Result<SandboxStatus> {
        if host_port == 0 {
            bail!("host port must be > 0");
        }
        {
            let map = self.inner.lock().unwrap();
            if let Some(e) = map.values().find(|e| {
                e.spec.ssh_port == host_port
                    || e.spec.forwards.iter().any(|f| f.host_port == host_port)
            }) {
                bail!(
                    "host port {host_port} is already used by sandbox \"{}\"",
                    e.spec.name
                );
            }
        }
        let mut done = false;
        self.with_entry(id, |e| {
            if e.spec.forwards.iter().any(|f| f.host_port == host_port) {
                return;
            }
            e.spec.forwards.push(PortFwd {
                host_port,
                guest_port: guest_port.clamp(1, 65535),
                guest_host: guest_host
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(default_fwd_host),
            });
            done = true;
        });
        if !done {
            bail!("no sandbox \"{id}\" (or the forward already exists)");
        }
        self.persist_spec(id);
        self.status_of(id)
    }

    pub fn fwd_del(&self, id: &str, host_port: u16) -> Result<SandboxStatus> {
        let mut done = false;
        self.with_entry(id, |e| {
            let before = e.spec.forwards.len();
            e.spec.forwards.retain(|f| f.host_port != host_port);
            done = e.spec.forwards.len() != before;
        });
        if !done {
            bail!("no forward on host port {host_port} in sandbox \"{id}\"");
        }
        self.persist_spec(id);
        self.status_of(id)
    }

    fn with_entry(&self, id: &str, f: impl FnOnce(&mut Entry)) {
        let mut map = self.inner.lock().unwrap();
        if let Some(e) = map.get_mut(id) {
            f(e);
        }
    }

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
        let ssh_port = req
            .ssh_port
            .unwrap_or(DEFAULT_PORT)
            .clamp(PORT_MIN, PORT_MAX);
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
            forwards: Vec::new(),
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        })
    }

    pub fn create(self: &Arc<Self>, req: &NewSandbox) -> Result<SandboxStatus> {
        let spec = self.prepare_spec(req)?;
        let dir = self.sandbox_dir(&spec.id);
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let raw = serde_json::to_string_pretty(&spec)?;
        std::fs::write(dir.join("sandbox.json"), raw)?;
        self.persist_pid(&spec.id, None);

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

    pub fn start(self: &Arc<Self>, id: &str) -> Result<SandboxStatus> {
        let spec = {
            let map = self.inner.lock().unwrap();
            let e = map
                .get(id)
                .ok_or_else(|| anyhow!("sandbox \"{id}\" not found"))?;
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

        let disk = dir.join("disk.qcow2");
        let custom_disk_boot =
            spec.kind == ImageKind::Custom && !is_iso_path(spec.iso_path.as_deref());
        if !custom_disk_boot {
            if spec.kind.needs_download() && !dir.join("image.qcow2").is_file() {
                let url = spec.kind.url().unwrap_or_default().to_string();
                self.spawn_download(id.to_string(), url, dir.join("image.qcow2"));
                self.with_entry(id, |e| {
                    e.state = VmState::Downloading;
                    e.error = None;
                });
                return Ok(self.status_of(id)?);
            }
            if !disk.is_file() {
                let out = silent(&mut Command::new(&img).args(build_img_args(
                    spec.kind,
                    &dir,
                    spec.disk_gib,
                )))
                .output()
                .map_err(|e| anyhow!("qemu-img: {e}"))?;
                if !out.status.success() {
                    let err = String::from_utf8_lossy(&out.stderr);
                    bail!("qemu-img failed: {}", err.trim());
                }
            }
        }

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

    pub fn stop(&self, id: &str) -> Result<SandboxStatus> {
        let pid = {
            let mut map = self.inner.lock().unwrap();
            let e = map
                .get_mut(id)
                .ok_or_else(|| anyhow!("sandbox \"{id}\" not found"))?;
            if let Some(p) = &e.prog {
                p.cancel.store(true, Ordering::Relaxed);
            }
            e.stopping = true;
            if e.state != VmState::Downloading {
                e.state = VmState::Stopped;
            }
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

    pub fn delete(self: &Arc<Self>, id: &str) -> Result<()> {
        clear_shell_route_if(id);
        let pid = {
            let map = self.inner.lock().unwrap();
            let e = map
                .get(id)
                .ok_or_else(|| anyhow!("sandbox \"{id}\" not found"))?;
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

            std::thread::sleep(Duration::from_millis(150));
        }
        map_remove(&self.inner, id);
        self.persist_pid(id, None);
        let dir = self.sandbox_dir(id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).with_context(|| format!("remove {}", dir.display()))?;
        }
        Ok(())
    }
}

fn map_remove(map: &Mutex<BTreeMap<String, Entry>>, id: &str) -> bool {
    map.lock().unwrap().remove(id).is_some()
}

impl SandboxManager {
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
            let cancelled = prog.cancel.load(Ordering::Relaxed);
            let ok = result.is_ok() && !cancelled;
            prog.done.store(true, Ordering::Relaxed);
            let mut error = result.err();
            if ok {
                if let Err(e) = std::fs::rename(&part, &dest) {
                    error = Some(format!("rename: {e}"));
                }
            } else {
                let _ = std::fs::remove_file(&part);
            }
            let final_state = if ok || cancelled {
                VmState::Stopped
            } else {
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
                        error
                            .clone()
                            .unwrap_or_else(|| "download failed".to_string()),
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
        .read_timeout(Duration::from_secs(60))
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
    while let Some(chunk) = resp.chunk().await.map_err(|e| format!("read: {e}"))? {
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

impl SandboxManager {
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
                    return;
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

                    clear_shell_route_if(&id);
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

impl SandboxManager {
    fn ssh_target(dir: &Path, spec: &SandboxSpec) -> crate::sshx::SshTarget {
        crate::sshx::SshTarget::new(dir, spec.ssh_port, &spec.login)
    }

    fn ssh_parts(&self, id: &str) -> Option<(SandboxSpec, PathBuf, Arc<SshLive>)> {
        let map = self.inner.lock().unwrap();
        map.get(id).and_then(|e| {
            e.ssh
                .as_ref()
                .map(|s| (e.spec.clone(), self.sandbox_dir(id), s.clone()))
        })
    }

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

            match std::fs::read_to_string(crate::config::config_path()) {
                Ok(cfg) => {
                    live.push_log("[*] uploading the host config (provider, api key)");
                    let remote = "mkdir -p \"$HOME/.config/hi-derola\" && cat > \"$HOME/.config/hi-derola/config.toml\"";
                    match crate::sshx::stream(
                        &bin,
                        &target,
                        remote,
                        Some(cfg.as_bytes()),
                        Duration::ZERO,
                        cancel,
                        |_| {},
                    ) {
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
                    live.push_log(
                        "[!] no host config found — the agent will start without an api key",
                    );
                }
            }

            let res = crate::sshx::stream(
                &bin,
                &target,
                "sh -s 2>&1",
                Some(crate::sshx::INSTALL_SH.as_bytes()),
                Duration::ZERO,
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
                    Ok(_) => mgr.fail_install(
                        &id,
                        "install reported success but the binary is missing".into(),
                    ),
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
        let bin = crate::sshx::find_ssh().ok_or_else(|| anyhow!("no ssh client on the host"))?;
        let target = Self::ssh_target(&dir, &spec);
        let cancel = AtomicBool::new(false);
        crate::sshx::exec(
            &bin,
            &target,
            command,
            Duration::from_secs(timeout_secs.unwrap_or(15).clamp(1, 600)),
            &cancel,
        )
        .map_err(|e| anyhow!("{e}"))
    }

    pub fn ssh_stream(
        &self,
        id: &str,
        command: &str,
        stdin: &[u8],
        timeout_secs: Option<u64>,
    ) -> Result<(i32, String)> {
        let (spec, dir, live) = self
            .ssh_parts(id)
            .ok_or_else(|| anyhow!("sandbox \"{id}\" has no ssh session"))?;
        if live.info.lock().unwrap().state != SshState::Ready {
            bail!("ssh is not ready yet");
        }
        let bin = crate::sshx::find_ssh().ok_or_else(|| anyhow!("no ssh client on the host"))?;
        let target = Self::ssh_target(&dir, &spec);
        let cancel = AtomicBool::new(false);
        let mut tail: Vec<String> = Vec::new();
        let code = crate::sshx::stream(
            &bin,
            &target,
            command,
            Some(stdin),
            Duration::from_secs(timeout_secs.unwrap_or(30).clamp(1, 600)),
            &cancel,
            |line| {
                if line.trim().is_empty() {
                    return;
                }
                if tail.len() >= 8 {
                    tail.remove(0);
                }
                tail.push(line.to_string());
            },
        )
        .map_err(|e| anyhow!("{e}"))?;
        Ok((code, tail.join("\n")))
    }
}

static SHELL_ROUTE: Mutex<Option<String>> = Mutex::new(None);

pub fn shell_route() -> Option<String> {
    SHELL_ROUTE.lock().unwrap().clone()
}

pub fn shell_route_sbx() -> Option<String> {
    let r = SHELL_ROUTE.lock().unwrap().clone()?;
    let id = r.strip_prefix("sbx:").unwrap_or(&r).to_string();
    (r.starts_with("sbx:") || !r.starts_with("host:")).then_some(id)
}

pub fn shell_route_host() -> Option<String> {
    let r = SHELL_ROUTE.lock().unwrap().clone()?;
    r.strip_prefix("host:").map(|s| s.to_string())
}

pub fn set_shell_route(id: Option<String>) {
    *SHELL_ROUTE.lock().unwrap() = id;
}

fn clear_shell_route_if(id: &str) {
    let mut r = SHELL_ROUTE.lock().unwrap();
    let hit = match r.as_deref() {
        Some(v) => v == id || v == format!("sbx:{id}"),
        None => false,
    };
    if hit {
        *r = None;
    }
}

enum RouteTarget {
    Sbx(String, String),
    Host(String, String),
}

fn route_resolve() -> Result<RouteTarget> {
    let raw = shell_route().ok_or_else(|| anyhow!("no sandbox or host attached"))?;
    if let Some(id) = raw.strip_prefix("host:") {
        let m = crate::hosts::HostManager::global();
        let list = m.list();
        let Some(st) = list.into_iter().find(|h| h.host.id == id) else {
            set_shell_route(None);
            bail!("attached host \"{id}\" no longer exists — routing detached");
        };
        if st.state == "failed" {
            bail!(
                "host \"{}\" is not reachable ({}): /host check for details",
                st.host.name,
                st.error.unwrap_or_default()
            );
        }
        return Ok(RouteTarget::Host(id.to_string(), st.host.name));
    }
    let id = raw.strip_prefix("sbx:").unwrap_or(&raw).to_string();
    let m = SandboxManager::global();
    let Some(st) = m.list().into_iter().find(|s| s.spec.id == id) else {
        clear_shell_route_if(&id);
        bail!("attached sandbox \"{id}\" no longer exists — routing detached");
    };
    if st.state != VmState::Running {
        clear_shell_route_if(&id);
        bail!(
            "sandbox \"{}\" is not running — routing detached",
            st.spec.name
        );
    }
    Ok(RouteTarget::Sbx(id, st.spec.name))
}

pub fn route_name() -> Option<String> {
    match route_resolve() {
        Ok(RouteTarget::Sbx(_, n)) => Some(n),
        Ok(RouteTarget::Host(_, n)) => Some(format!("{n} (remote host)")),
        Err(_) => None,
    }
}

fn route_ssh_exec(command: &str, timeout_secs: Option<u64>) -> Result<crate::sshx::SshOut> {
    match route_resolve()? {
        RouteTarget::Sbx(id, _) => SandboxManager::global().ssh_exec(&id, command, timeout_secs),
        RouteTarget::Host(id, _) => {
            crate::hosts::HostManager::global().exec(&id, command, timeout_secs)
        }
    }
}

fn route_ssh_stream(
    command: &str,
    stdin: &[u8],
    timeout_secs: Option<u64>,
) -> Result<(i32, String)> {
    match route_resolve()? {
        RouteTarget::Sbx(id, _) => {
            SandboxManager::global().ssh_stream(&id, command, stdin, timeout_secs)
        }
        RouteTarget::Host(id, _) => {
            crate::hosts::HostManager::global().stream(&id, command, stdin, timeout_secs)
        }
    }
}

pub fn sandbox_bash(cmd: &str, timeout_secs: Option<u64>) -> Result<String> {
    let out = route_ssh_exec(cmd, Some(timeout_secs.unwrap_or(120).clamp(1, 600)))?;
    let mut text = out.stdout;
    if !out.stderr.trim().is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&out.stderr);
    }
    if text.trim().is_empty() {
        text.push_str("(no output)");
    }
    if out.code != 0 {
        text.push_str(&format!("\nexit code: {}", out.code));
    }
    Ok(text)
}

pub fn shell_route_addendum() -> Option<String> {
    let mode = match route_resolve() {
        Ok(RouteTarget::Sbx(_, name)) => format!(
            "SANDBOX MODE is active: bash AND the file tools (read_file, write_file, edit, \
             glob, grep, list_files, apply_patch) execute INSIDE the sandbox VM \"{name}\" (\
             linux {}) over ssh, not on the host.",
            std::env::consts::ARCH
        ),
        Ok(RouteTarget::Host(_, name)) => format!(
            "REMOTE HOST MODE is active: bash AND the file tools (read_file, write_file, edit, \
             glob, grep, list_files, apply_patch) execute ON the remote host \"{name}\" over \
             ssh, not on the local machine."
        ),
        Err(_) => return None,
    };
    Some(format!(
        "{mode} Use unix paths and unix commands; relative \
         paths resolve against the remote user's home directory. Image files are fetched over \
         ssh and attached like local ones; the formatter, LSP and diagnostics do not apply \
         to remote files. Background bash works too — it becomes a remote nohup task reported \
         through task_status and stoppable with task_kill. Everything you create stays \
         on the remote machine; the local working directory is out of reach."
    ))
}

fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn guest_rel(line: &str, dir: &str) -> String {
    let l = line.strip_prefix("./").unwrap_or(line);
    if dir == "." || dir.is_empty() {
        return l.to_string();
    }
    let d = dir.trim_end_matches('/');
    l.strip_prefix(d)
        .map(|r| r.trim_start_matches('/').to_string())
        .unwrap_or_else(|| l.to_string())
}

fn guest_fetch_cap(path: &str, cap: u64) -> Result<Vec<u8>> {
    let out = route_ssh_exec(&format!("wc -c < {}", sq(path)), Some(30))?;
    if out.code != 0 {
        bail!("{path}: {}", one_line(&out.stderr, out.code));
    }
    let size: u64 = out.stdout.trim().parse().unwrap_or(0);
    if size > cap {
        bail!("{path}: too large ({size} bytes)");
    }
    let out = route_ssh_exec(&format!("base64 -w0 -- {}", sq(path)), Some(60))?;
    if out.code != 0 {
        bail!("{path}: {}", one_line(&out.stderr, out.code));
    }
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(out.stdout.trim())
        .map_err(|e| anyhow!("{path}: base64 decode failed: {e}"))
}

fn guest_fetch(path: &str) -> Result<Vec<u8>> {
    guest_fetch_cap(path, crate::tools::MAX_ATTACH_BYTES)
}

pub fn guest_apply_patch(patch_text: &str) -> Result<String> {
    route_resolve()?;
    let read = |p: &str| -> Result<String> {
        let bytes = guest_fetch_cap(p, 32 * 1024 * 1024)?;
        String::from_utf8(bytes).map_err(|_| anyhow!("{p}: not valid utf-8"))
    };
    let exists = |p: &str| -> Result<bool> {
        Ok(route_ssh_exec(&format!("test -e {}", sq(p)), Some(15))?.code == 0)
    };
    let planned = crate::patch::plan_with(crate::patch::parse(patch_text)?, &read, &exists)?;
    let items = planned.items.clone();
    for (path, content) in &planned.write {
        record_vm_overwrite(path);
        guest_push(path, content.as_bytes())?;
    }
    for path in &planned.delete {
        record_vm_overwrite(path);
        let out = route_ssh_exec(&format!("rm -f -- {}", sq(path)), Some(15))?;
        if out.code != 0 {
            bail!("apply_patch: {path}: {}", one_line(&out.stderr, out.code));
        }
    }
    let mut out = String::from("Success. Updated the following files:");
    for (k, p) in &items {
        out.push_str(&format!("\n{k} {p}"));
    }
    Ok(out)
}

pub fn guest_read_image(path: &str) -> Result<(String, String)> {
    route_resolve()?;
    let out = route_ssh_exec(&format!("wc -c < {}", sq(path)), Some(30))?;
    if out.code != 0 {
        bail!("{path}: {}", one_line(&out.stderr, out.code));
    }
    let size: u64 = out.stdout.trim().parse().unwrap_or(0);
    if size > crate::files::MAX_IMAGE_BYTES {
        bail!("{path}: too large for an image ({size} bytes)");
    }
    let out = route_ssh_exec(&format!("base64 -w0 -- {}", sq(path)), Some(60))?;
    if out.code != 0 {
        bail!("{path}: {}", one_line(&out.stderr, out.code));
    }
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    Ok((
        crate::files::image_mime(&ext).to_string(),
        out.stdout.trim().to_string(),
    ))
}

pub fn fetch_from_vm(id: &str, vm_path: &str, host_path: &str) -> Result<String> {
    let out = SandboxManager::global().ssh_exec(
        id,
        &format!("base64 -w0 -- {}", sq(vm_path)),
        Some(60),
    )?;
    if out.code != 0 {
        bail!("{vm_path}: {}", one_line(&out.stderr, out.code));
    }
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(out.stdout.trim())
        .map_err(|e| anyhow!("{vm_path}: base64 decode failed: {e}"))?;
    let host = Path::new(host_path);
    if let Some(p) = host.parent() {
        if !p.as_os_str().is_empty() {
            std::fs::create_dir_all(p)?;
        }
    }
    std::fs::write(host, &bytes).map_err(|e| anyhow!("{host_path}: {e}"))?;
    Ok(format!(
        "fetched {vm_path} -> {host_path} ({} bytes)",
        bytes.len()
    ))
}

pub fn push_to_vm(id: &str, host_path: &str, vm_path: &str) -> Result<String> {
    const MAX_PUSH: u64 = 256 * 1024 * 1024;
    let meta = std::fs::metadata(host_path).map_err(|e| anyhow!("{host_path}: {e}"))?;
    if !meta.is_file() {
        bail!("{host_path}: not a file");
    }
    if meta.len() > MAX_PUSH {
        bail!(
            "{host_path}: {} bytes exceeds the 256 MiB push cap — use git or scp instead",
            meta.len()
        );
    }
    let bytes = std::fs::read(host_path).map_err(|e| anyhow!("{host_path}: {e}"))?;
    let dir = match vm_path.rsplit_once('/') {
        Some((d, _)) if !d.is_empty() => d,
        _ => ".",
    };
    let remote = format!("mkdir -p -- {} && cat > {}", sq(dir), sq(vm_path));
    let (code, msg) = SandboxManager::global().ssh_stream(id, &remote, &bytes, Some(60))?;
    if code != 0 {
        bail!(
            "{vm_path}: {}",
            if msg.trim().is_empty() {
                format!("write failed (exit {code})")
            } else {
                msg.trim().to_string()
            }
        );
    }
    Ok(format!(
        "pushed {host_path} -> {vm_path} ({} bytes)",
        bytes.len()
    ))
}

type BgVms = HashMap<String, (String, u32, String, String)>;

static BG_VMS: OnceLock<Arc<Mutex<BgVms>>> = OnceLock::new();

fn bg_vms() -> &'static Arc<Mutex<BgVms>> {
    BG_VMS.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

pub fn vm_bg_register(bg_id: &str, sandbox: &str, pid: u32, log: &str, code_file: &str) {
    bg_vms().lock().unwrap().insert(
        bg_id.to_string(),
        (
            sandbox.to_string(),
            pid,
            log.to_string(),
            code_file.to_string(),
        ),
    );
}

pub fn vm_bg_unregister(bg_id: &str) {
    bg_vms().lock().unwrap().remove(bg_id);
}

fn vm_bg_lookup(bg_id: &str) -> Option<(String, u32)> {
    bg_vms()
        .lock()
        .unwrap()
        .get(bg_id)
        .map(|(s, p, _, _)| (s.clone(), *p))
}

pub fn vm_bg_spawn(cmd: &str) -> Result<(String, u32, String, String)> {
    let route = shell_route().ok_or_else(|| anyhow!("no sandbox or host attached"))?;
    let name = route_name().unwrap_or_else(|| route.clone());
    let inner = format!(
        "{}; echo $? > \"$HI_BG_CODE\"",
        cmd.trim_end_matches(['\n', ';', ' '])
    );
    let remote = format!(
        "d=\"$HOME/.hi-derola-bg\"; mkdir -p \"$d\"; l=\"$d/$(date +%s%N).log\"; \
         c=\"$l.code\"; HI_BG_CODE=\"$c\" setsid nohup sh -c {} >\"$l\" 2>&1 & p=$!; \
         echo \"$p\"; echo \"$l\"",
        sq(&inner)
    );
    let out = route_ssh_exec(&remote, Some(15))?;
    if out.code != 0 {
        bail!("{}: {}", name, one_line(&out.stderr, out.code));
    }
    let (pid, log) = parse_vm_bg_spawn(&out.stdout)?;
    Ok((route, pid, log.clone(), format!("{log}.code")))
}

fn parse_vm_bg_spawn(stdout: &str) -> Result<(u32, String)> {
    let mut lines = stdout.lines();
    let pid = lines
        .next()
        .unwrap_or("")
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|p| *p > 0)
        .ok_or_else(|| anyhow!("could not parse the background pid"))?;
    let log = lines.next().unwrap_or("").trim().to_string();
    if log.is_empty() {
        bail!("could not parse the background log path");
    }
    Ok((pid, log))
}

fn route_exec_by(
    route: &str,
    command: &str,
    timeout_secs: Option<u64>,
) -> Result<crate::sshx::SshOut> {
    if let Some(id) = route.strip_prefix("host:") {
        return crate::hosts::HostManager::global().exec(id, command, timeout_secs);
    }
    let id = route.strip_prefix("sbx:").unwrap_or(route);
    SandboxManager::global().ssh_exec(id, command, timeout_secs)
}

pub fn vm_bg_kill(route: &str, pid: u32) -> Result<()> {
    route_exec_by(
        route,
        &format!("kill -- -{pid} 2>/dev/null || kill {pid} 2>/dev/null; true"),
        Some(10),
    )?;
    Ok(())
}

pub fn vm_bg_kill_by_id(bg_id: &str) -> String {
    match vm_bg_lookup(bg_id) {
        Some((sandbox, pid)) => match vm_bg_kill(&sandbox, pid) {
            Ok(()) => format!("task {bg_id} killed (remote process group)"),
            Err(e) => format!("task {bg_id}: remote kill failed: {e:#}"),
        },
        None => format!("task {bg_id} has no remote process to kill"),
    }
}

pub struct VmBgPoll {
    pub size: u64,
    pub chunk: String,
    pub alive: bool,
    pub code: Option<i32>,
}

pub fn vm_bg_poll(
    route: &str,
    pid: u32,
    log: &str,
    code_file: &str,
    offset: u64,
) -> Result<VmBgPoll> {
    let remote = format!(
        "s=$(wc -c < {} 2>/dev/null || echo 0); echo \"$s\"; \
         [ \"$s\" -gt {} ] && tail -c +{} {} 2>/dev/null; true",
        sq(log),
        offset,
        offset + 1,
        sq(log)
    );
    let out = route_exec_by(route, &remote, Some(15))?;
    if out.code != 0 && out.stdout.trim().is_empty() {
        bail!("log read failed: {}", one_line(&out.stderr, out.code));
    }
    let (size_line, chunk) = match out.stdout.split_once('\n') {
        Some((a, b)) => (a, b.to_string()),
        None => (out.stdout.trim(), String::new()),
    };
    let size: u64 = size_line.trim().parse().unwrap_or(0);
    let remote2 = format!(
        "if kill -0 {pid} 2>/dev/null; then echo RUNNING; else echo DONE; \
         cat {} 2>/dev/null || echo 0; fi",
        sq(code_file)
    );
    let out2 = route_exec_by(route, &remote2, Some(10))?;
    let (alive, code) = parse_vm_bg_alive(&out2.stdout);
    Ok(VmBgPoll {
        size,
        chunk,
        alive,
        code,
    })
}

fn parse_vm_bg_alive(stdout: &str) -> (bool, Option<i32>) {
    if stdout.contains("RUNNING") {
        return (true, None);
    }
    let code = stdout
        .lines()
        .skip(1)
        .find_map(|l| l.trim().parse::<i32>().ok());
    (false, code)
}

fn guest_push(path: &str, bytes: &[u8]) -> Result<()> {
    let dir = match path.rsplit_once('/') {
        Some((d, _)) if !d.is_empty() => d,
        _ => ".",
    };
    let remote = format!("mkdir -p -- {} && cat > {}", sq(dir), sq(path));
    let (code, msg) = route_ssh_stream(&remote, bytes, Some(60))?;
    if code != 0 {
        bail!(
            "{path}: {}",
            if msg.trim().is_empty() {
                format!("write failed (exit {code})")
            } else {
                msg.trim().to_string()
            }
        );
    }
    Ok(())
}

fn record_vm_overwrite(path: &str) {
    let old = guest_fetch_cap(path, crate::snapshot::VM_SNAPSHOT_FILE_CAP).ok();
    crate::snapshot::vm_record(path, old);
}

pub fn vm_fetch_capped(path: &str, cap: u64) -> Option<Vec<u8>> {
    if shell_route().is_none() {
        return None;
    }
    guest_fetch_cap(path, cap).ok()
}

pub fn vm_restore(path: &str, data: Option<&[u8]>) {
    let res = match data {
        Some(bytes) => guest_push(path, bytes),
        None => route_ssh_exec(&format!("rm -f -- {}", sq(path)), Some(15)).map(|_| ()),
    };
    if let Err(e) = res {
        eprintln!("hi-derola: undo restore {path}: {e:#}");
    }
}

pub fn guest_read(path: &str, offset: usize, limit: usize) -> Result<String> {
    route_resolve()?;
    let bytes = guest_fetch(path)?;
    crate::tools::format_numbered(path, bytes, offset, limit)
}

pub fn guest_write(path: &str, content: &str) -> Result<String> {
    route_resolve()?;
    record_vm_overwrite(path);
    guest_push(path, content.as_bytes())?;
    Ok(format!("wrote {path} ({} lines)", content.lines().count()))
}

pub fn guest_edit(path: &str, old: &str, new: &str, replace_all: bool) -> Result<String> {
    route_resolve()?;
    let bytes = guest_fetch(path)?;
    let content = String::from_utf8(bytes).map_err(|_| anyhow!("{path}: not valid utf-8"))?;
    let (updated, count) = crate::tools::apply_edit(&content, old, new, replace_all)
        .map_err(|e| anyhow!("edit: {e:#} in {path}"))?;
    crate::snapshot::vm_record(path, Some(content.into_bytes()));
    guest_push(path, updated.as_bytes())?;
    Ok(format!(
        "edited {path} ({} replacement{})",
        count,
        if count == 1 { "" } else { "s" }
    ))
}

pub fn guest_list_files(dir: &str) -> Result<String> {
    route_resolve()?;
    let max = crate::tools::MAX_LIST;
    let remote = format!(
        "find {} -maxdepth 3 -mindepth 1 \\( -type d -printf '%p/\\n' -o -type f -printf '%p\\n' \\) 2>/dev/null \
         | LC_ALL=C sort | head -n {}",
        sq(dir),
        max + 1
    );
    let out = route_ssh_exec(&remote, Some(30))?;
    if out.code != 0 {
        bail!("{dir}: {}", one_line(&out.stderr, out.code));
    }
    let mut entries: Vec<String> = out
        .stdout
        .lines()
        .map(|l| guest_rel(l, dir))
        .filter(|l| !l.is_empty())
        .collect();
    if entries.is_empty() {
        return Ok(format!("{dir}: empty"));
    }
    let truncated = entries.len() > max;
    entries.truncate(max);
    if truncated {
        entries.push(format!("... truncated at {max} entries"));
    }
    Ok(entries.join("\n"))
}

pub fn guest_glob(dir: &str, pattern: &str) -> Result<String> {
    use crate::search::{glob_match, MAX_RESULTS};
    route_resolve()?;
    let pats = crate::search::expand_braces(pattern);
    let remote = format!(
        "find {} -type f -not -path '*/.git/*' 2>/dev/null | head -n 20001",
        sq(dir)
    );
    let out = route_ssh_exec(&remote, Some(60))?;
    if out.code != 0 {
        bail!("{dir}: {}", one_line(&out.stderr, out.code));
    }
    let mut files: Vec<String> = out.stdout.lines().map(|l| guest_rel(l, dir)).collect();
    files.sort();
    let mut matched = Vec::new();
    for f in &files {
        if pats.iter().any(|p| glob_match(p, f)) {
            matched.push(f.clone());
            if matched.len() >= MAX_RESULTS {
                break;
            }
        }
    }
    if matched.is_empty() {
        return Ok(format!("{pattern}: no files found"));
    }
    let mut s = matched.join("\n");
    if matched.len() >= MAX_RESULTS {
        s.push_str(&format!("\n... truncated at {MAX_RESULTS} results"));
    }
    Ok(s)
}

pub fn guest_grep(dir: &str, pattern: &str, include: Option<&str>) -> Result<String> {
    use crate::search::MAX_RESULTS;
    if let Err(e) = regex::Regex::new(pattern) {
        bail!("grep: {e}");
    }
    route_resolve()?;
    let ere = pattern.replace(r"\d", "[0-9]").replace(r"\D", "[^0-9]");
    let mut remote = String::from("set -o pipefail; grep -rInE");
    if let Some(inc) = include {
        for p in crate::search::expand_braces(inc) {
            remote.push_str(&format!(" --include={}", sq(&p)));
        }
    }
    remote.push_str(&format!(
        " -- {} {} 2>/dev/null | head -n 400",
        sq(&ere),
        sq(dir)
    ));
    let out = route_ssh_exec(&remote, Some(60))?;
    if out.code > 1 {
        bail!("grep: {}", one_line(&out.stderr, out.code));
    }
    let mut hits: Vec<(String, usize, String)> = Vec::new();
    let mut more = false;
    for line in out.stdout.lines() {
        let Some(a) = line.find(':') else { continue };
        let rest = &line[a + 1..];
        let Some(b) = rest.find(':') else { continue };
        let Ok(n) = rest[..b].parse::<usize>() else {
            continue;
        };
        if hits.len() >= MAX_RESULTS {
            more = true;
            break;
        }
        hits.push((
            guest_rel(&line[..a], dir),
            n,
            rest[b + 1..].chars().take(200).collect(),
        ));
    }
    if hits.is_empty() {
        return Ok(format!("{pattern}: no matches"));
    }
    let mut s = format!("Found {} matches", hits.len());
    if more {
        s.push_str(" (more matches available)");
    }
    let mut cur = String::new();
    for (path, n, text) in hits {
        if path != cur {
            cur = path.clone();
            s.push_str(&format!("\n{cur}:"));
        }
        s.push_str(&format!("\n  Line {n}: {text}"));
    }
    Ok(s)
}

fn one_line(stderr: &str, code: i32) -> String {
    let msg = stderr.trim();
    if msg.is_empty() {
        format!("command failed (exit {code})")
    } else {
        msg.lines().last().unwrap_or(msg).to_string()
    }
}

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
    text.lines()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join(" | ")
}

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

fn unique_id(base: &str, mut taken: impl FnMut(&str) -> bool) -> String {
    for _ in 0..64 {
        let mut b = [0u8; 2];
        getrandom::fill(&mut b).expect("os rng");
        let cand = format!("{base}-{:02x}{:02x}", b[0], b[1]);
        if !taken(&cand) {
            return cand;
        }
    }
    format!(
        "{base}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    )
}

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
    let r = unsafe { libc::kill(pid as i32, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn pid_is_qemu(pid: u32) -> bool {
    #[cfg(unix)]
    {
        std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .map(|s| s.trim().starts_with("qemu"))
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        let Ok(out) = silent(&mut Command::new("tasklist").args([
            "/FI",
            &format!("PID eq {pid}"),
            "/NH",
            "/FO",
            "CSV",
        ]))
        .output() else {
            return false;
        };
        String::from_utf8_lossy(&out.stdout)
            .to_lowercase()
            .contains("qemu")
    }
}

#[cfg(windows)]
fn pid_alive(pid: u32) -> bool {
    let Ok(out) =
        silent(&mut Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH"]))
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

    #[test]
    fn sq_quotes_for_the_guest_shell() {
        assert_eq!(sq("plain"), "'plain'");
        assert_eq!(sq("a'b"), "'a'\\''b'");
        assert_eq!(sq("$(rm -rf ~)"), "'$(rm -rf ~)'");
        assert_eq!(sq(""), "''");

        let quoted = sq("it's \"ok\"");
        assert!(quoted.starts_with('\'') && quoted.ends_with('\''));
        assert!(quoted.contains(r"'\'"));
    }

    #[test]
    fn guest_rel_strips_dot_slash_and_root() {
        assert_eq!(guest_rel("./src/lib.rs", "."), "src/lib.rs");
        assert_eq!(guest_rel("src/lib.rs", "."), "src/lib.rs");
        assert_eq!(guest_rel("/home/u/src/lib.rs", "/home/u"), "src/lib.rs");
        assert_eq!(guest_rel("/home/u/src/lib.rs", "/home/u/"), "src/lib.rs");

        assert_eq!(guest_rel("/etc/passwd", "/home/u"), "/etc/passwd");
    }

    #[test]
    fn one_line_compact_errors() {
        assert_eq!(one_line("", 2), "command failed (exit 2)");
        assert_eq!(one_line("  \n", 1), "command failed (exit 1)");
        let err = "bash: line 1: /tmp: Is a directory\nbash: error: x";
        assert_eq!(one_line(err, 1), "bash: error: x");
    }

    #[test]
    fn vm_bg_spawn_output_parse() {
        let (pid, log) = parse_vm_bg_spawn("12345\n/root/.hi-derola-bg/x.log\n").unwrap();
        assert_eq!(pid, 12345);
        assert_eq!(log, "/root/.hi-derola-bg/x.log");

        let (pid, log) = parse_vm_bg_spawn(" 42 \n /tmp/l.log \n").unwrap();
        assert_eq!(pid, 42);
        assert_eq!(log, "/tmp/l.log");

        assert!(parse_vm_bg_spawn("").is_err());
        assert!(parse_vm_bg_spawn("abc\n/x.log\n").is_err());
        assert!(parse_vm_bg_spawn("0\n/x.log\n").is_err());
        assert!(parse_vm_bg_spawn("7\n").is_err());
    }

    #[test]
    fn vm_bg_alive_output_parse() {
        assert_eq!(parse_vm_bg_alive("RUNNING\n"), (true, None));
        assert_eq!(parse_vm_bg_alive("DONE\n0\n"), (false, Some(0)));
        assert_eq!(parse_vm_bg_alive("DONE\n127\n"), (false, Some(127)));

        assert_eq!(parse_vm_bg_alive("DONE\n"), (false, None));
    }

    #[test]
    fn guest_read_uses_the_shared_numbered_formatter() {
        let out = crate::tools::format_numbered("t.rs", b"fn a() {}\nfn b() {}\n".to_vec(), 1, 1)
            .unwrap();
        assert!(out.starts_with("t.rs (2 lines)\n"));
        assert!(out.contains("1\u{2192}fn a() {}"));
        assert!(out.contains("... (+1 more lines, use offset 2)"));

        let bin = vec![0u8, 1, 2, 3];
        let out = crate::tools::format_numbered("t.bin", bin, 1, 10).unwrap();
        assert!(out.contains("binary file (4 bytes)"));

        let big = vec![b'x'; (crate::tools::MAX_ATTACH_BYTES + 1) as usize];
        assert!(crate::tools::format_numbered("big", big, 1, 10).is_err());
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hiderola-sbx-{}-{}", tag, std::process::id()));
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
            forwards: Vec::new(),
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
        assert_eq!(
            serde_json::to_string(&ImageKind::Nixos).unwrap(),
            "\"nixos\""
        );
        assert_eq!(
            serde_json::to_string(&ImageKind::Custom).unwrap(),
            "\"custom\""
        );
        let k: ImageKind = serde_json::from_str("\"ubuntu-24.04\"").unwrap();
        assert_eq!(k, ImageKind::Ubuntu2404);

        let k: ImageKind = serde_json::from_str("\"nixos\"").unwrap();
        assert_eq!(k, ImageKind::Nixos);
        assert_eq!(ImageKind::DebianTrixie.url(), Some(DEBIAN_TRIXIE_URL));
        assert_eq!(ImageKind::Ubuntu2404.url(), Some(UBUNTU_2404_URL));
        assert!(ImageKind::DebianTrixie.needs_download());
        assert!(ImageKind::Ubuntu2404.needs_download());
        assert!(!ImageKind::Custom.needs_download());
        assert!(DEBIAN_TRIXIE_URL.starts_with("https://cloud.debian.org/images/cloud/trixie/"));
        assert!(
            UBUNTU_2404_URL.starts_with("https://cloud-images.ubuntu.com/minimal/releases/24.04/")
        );
        assert!(UBUNTU_2404_URL.ends_with("ubuntu-24.04-minimal-cloudimg-amd64.img"));
    }

    #[test]
    fn standard_variants_roundtrip_and_urls() {
        assert_eq!(
            serde_json::to_string(&ImageKind::DebianTrixieStd).unwrap(),
            "\"debian-trixie-std\""
        );
        assert_eq!(
            serde_json::to_string(&ImageKind::Ubuntu2404Std).unwrap(),
            "\"ubuntu-24.04-std\""
        );
        let k: ImageKind = serde_json::from_str("\"debian-trixie-std\"").unwrap();
        assert_eq!(k, ImageKind::DebianTrixieStd);
        let k: ImageKind = serde_json::from_str("\"ubuntu-24.04-std\"").unwrap();
        assert_eq!(k, ImageKind::Ubuntu2404Std);
        assert_eq!(
            ImageKind::DebianTrixieStd.url(),
            Some(DEBIAN_TRIXIE_STD_URL)
        );
        assert_eq!(ImageKind::Ubuntu2404Std.url(), Some(UBUNTU_2404_STD_URL));
        assert!(DEBIAN_TRIXIE_STD_URL.ends_with("debian-13-generic-amd64.qcow2"));
        assert!(UBUNTU_2404_STD_URL.ends_with("ubuntu-24.04-server-cloudimg-amd64.img"));
        assert_eq!(
            ImageKind::DebianTrixie.label(),
            "Debian 13 (trixie) minimal"
        );
        assert_eq!(
            ImageKind::DebianTrixieStd.label(),
            "Debian 13 (trixie) standard"
        );
        assert_eq!(ImageKind::Ubuntu2404.label(), "Ubuntu 24.04 LTS minimal");
        assert_eq!(
            ImageKind::Ubuntu2404Std.label(),
            "Ubuntu 24.04 LTS standard"
        );
        assert!(ImageKind::DebianTrixieStd.needs_download());
        assert!(ImageKind::Ubuntu2404Std.needs_download());
    }

    #[test]
    fn seed_only_for_cloud_init_kinds() {
        assert!(ImageKind::DebianTrixie.wants_seed());
        assert!(ImageKind::Ubuntu2404.wants_seed());
        assert!(ImageKind::DebianTrixieStd.wants_seed());
        assert!(ImageKind::Ubuntu2404Std.wants_seed());

        assert!(!ImageKind::Nixos.wants_seed());
        assert!(!ImageKind::Custom.wants_seed());
    }

    #[test]
    fn shell_route_set_clear_and_detach() {
        test_global_dir(
            std::env::temp_dir().join(format!("hiderola-sbx-test-{}", std::process::id())),
        );
        set_shell_route(None);
        assert!(shell_route().is_none());

        set_shell_route(Some("sbx:vm-a".into()));
        clear_shell_route_if("vm-b");
        assert_eq!(shell_route().as_deref(), Some("sbx:vm-a"));
        assert_eq!(shell_route_sbx().as_deref(), Some("vm-a"));
        assert!(shell_route_host().is_none());
        clear_shell_route_if("vm-a");
        assert!(shell_route().is_none());

        set_shell_route(Some("vm-a".into()));
        assert_eq!(shell_route_sbx().as_deref(), Some("vm-a"));
        set_shell_route(None);

        set_shell_route(Some("host:web-1".into()));
        assert_eq!(shell_route_host().as_deref(), Some("web-1"));
        assert!(shell_route_sbx().is_none());
        clear_shell_route_if("web-1");
        assert_eq!(shell_route_host().as_deref(), Some("web-1"));
        set_shell_route(None);

        let err = sandbox_bash("true", None).unwrap_err().to_string();
        assert!(err.contains("no sandbox or host attached"), "{err}");

        set_shell_route(Some("sbx:ghost-vm".into()));
        let err = sandbox_bash("true", None).unwrap_err().to_string();
        assert!(err.contains("no longer exists"), "{err}");
        assert!(shell_route().is_none());
        set_shell_route(None);
    }

    #[test]
    fn version_and_accel_parsing() {
        let v = parse_version_line(
            "QEMU emulator version 8.2.0 (Debian 1:8.2.0+ds-1)\nCopyright (c) 2003-2023",
        );
        assert!(v.unwrap().contains("8.2.0"));
        assert_eq!(parse_version_line("no version here"), None);
        assert_eq!(parse_version_line(""), None);
        assert_eq!(
            parse_accel_list("Accelerators supported with machine default:\nwhpx\ntcg\n"),
            "whpx"
        );
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

        assert!(s.contains("hostfwd=tcp:127.0.0.1:2222-:22"));
        assert!(!s.contains("hostfwd=tcp::2222"));
        assert!(s.contains("-device virtio-net-pci,netdev=n0"));
        assert!(s.contains("-name hiderola-test-01"));
        assert!(!s.contains("-cpu max"), "hw accel keeps the default cpu");
        assert!(!s.contains("-cdrom"));

        assert!(s.contains("file=/vm/test/seed.img,format=raw,if=virtio"));

        let ubuntu = spec("ub", ImageKind::Ubuntu2404, None);
        let a = build_qemu_args(&ubuntu, dir, "kvm");
        assert!(a
            .join(" ")
            .contains("file=/vm/test/seed.img,format=raw,if=virtio"));

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

        let nix = spec("nix", ImageKind::Nixos, None);
        let a = build_qemu_args(&nix, dir, "kvm");
        assert!(!a.join(" ").contains("seed.img"));
    }

    #[test]
    fn img_args_overlay_vs_plain() {
        let dir = Path::new("/vm/test");
        for kind in [
            ImageKind::DebianTrixie,
            ImageKind::DebianTrixieStd,
            ImageKind::Ubuntu2404,
            ImageKind::Ubuntu2404Std,
            ImageKind::Nixos,
        ] {
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
            calls <= 2
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
                disk_gib: Some(1000),
                ram_mib: Some(100),
                cpus: Some(99),
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

        assert!(mgr.create(&req("Test Box", ImageKind::Custom)).is_err());

        let mut r2 = req("other", ImageKind::Custom);
        r2.iso_path = Some(iso.display().to_string());
        r2.ssh_port = Some(3000);
        assert!(mgr.create(&r2).is_err());

        let mut r3 = req("noiso", ImageKind::Custom);
        r3.iso_path = Some("/definitely/not/here.iso".into());
        assert!(mgr.create(&r3).is_err());

        assert!(mgr.create(&req("   ", ImageKind::Custom)).is_err());

        mgr.delete(&st.spec.id).unwrap();
        assert!(mgr.list().is_empty());
        assert!(!dir.join(&st.spec.id).exists());

        assert!(mgr.delete(&st.spec.id).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cloud_create_smoke() {
        let dir = temp_dir("cloud");
        let mgr = Arc::new(SandboxManager::new(dir.clone()));
        let st = mgr
            .create(&req("deb box", ImageKind::DebianTrixie))
            .unwrap();
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

        assert!(sdir.join("seed.img").is_file());
        assert!(sdir.join("id_ed25519").is_file());
        assert!(sdir.join("id_ed25519.pub").is_file());
        let pub_line = std::fs::read_to_string(sdir.join("id_ed25519.pub")).unwrap();
        assert!(pub_line.starts_with("ssh-ed25519 "));
        assert!(pub_line.contains("hiderola-"));

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

        let mut r = req("dflt", ImageKind::Custom);
        r.iso_path = Some(iso.display().to_string());
        let st = mgr.create(&r).unwrap();
        assert_eq!(st.spec.login, "derola");
        let _ = mgr.delete(&st.spec.id);

        let mut r = req("custom", ImageKind::Custom);
        r.iso_path = Some(iso.display().to_string());
        r.login = Some("  agent-1 ".into());
        let st = mgr.create(&r).unwrap();
        assert_eq!(st.spec.login, "agent-1");
        let _ = mgr.delete(&st.spec.id);

        let mut r = req("blank", ImageKind::Custom);
        r.iso_path = Some(iso.display().to_string());
        r.login = Some("   ".into());
        let st = mgr.create(&r).unwrap();
        assert_eq!(st.spec.login, "derola");
        let _ = mgr.delete(&st.spec.id);

        for bad in [
            "root",
            "Root",
            "9lives",
            "has space",
            "x".repeat(33).as_str(),
        ] {
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

        assert!(mgr.install_agent("missing").is_err());

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

        let mgr2 = Arc::new(SandboxManager::new(dir.clone()));
        let items = mgr2.list();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].state, VmState::Stopped);

        mgr2.persist_pid(&id, Some(4_000_000_000));
        mgr2.reload();
        assert_eq!(mgr2.list()[0].state, VmState::Stopped);

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
        let v: serde_json::Value = serde_json::to_value(&st.clone()).unwrap();
        assert!(v["spec"].is_object());
    }

    #[test]
    fn ssh_info_json_shape_matches_gui() {
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
