//! remote hosts: ssh targets that are not local qemu sandboxes.
//!
//! one json file per host under `<data>/hi-derola/hosts/<id>.json`, an ssh
//! key pair per host under `<data>/hi-derola/hosts/<id>/`. the agent's bash
//! and file tools reach an attached host through the same shell route the
//! sandboxes use, so once a host answers over ssh everything downstream
//! (guest file tools, background bash) works unchanged.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteHost {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub created: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct HostStatus {
    pub host: RemoteHost,
    /// ready = last check answered, failed = refused/timed out, new = never checked
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<bool>,
}

#[derive(Clone)]
pub struct HostManager {
    dir: PathBuf,
}

impl HostManager {
    pub fn global() -> &'static HostManager {
        static MGR: std::sync::OnceLock<HostManager> = std::sync::OnceLock::new();
        MGR.get_or_init(|| HostManager::at(hosts_base()))
    }

    pub fn at(dir: PathBuf) -> HostManager {
        HostManager { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn host_file(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    fn key_dir(&self, id: &str) -> PathBuf {
        self.dir.join(id)
    }

    fn read_one(&self, id: &str) -> Option<RemoteHost> {
        let raw = std::fs::read_to_string(self.host_file(id)).ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// every stored host with its persisted state (ready/failed + last error)
    pub fn list(&self) -> Vec<HostStatus> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return out;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(&p) else {
                continue;
            };
            let Ok(h) = serde_json::from_str::<RemoteHost>(&raw) else {
                continue;
            };
            let st = read_state(&self.dir, &h.id);
            out.push(HostStatus {
                host: h,
                state: st.state,
                error: st.error,
                checked: st.checked,
                agent: None,
            });
        }
        out.sort_by(|a, b| a.host.name.cmp(&b.host.name));
        out
    }

    pub fn add(&self, name: &str, user: &str, host: &str, port: u16) -> Result<HostStatus> {
        let user = user.trim();
        let host = host.trim();
        if user.is_empty() || host.is_empty() {
            bail!("user and host are required");
        }
        let id = {
            let base = crate::sessions::new_id();
            format!("h{}", &base[1..])
        };
        let name = name.trim();
        let h = RemoteHost {
            id: id.clone(),
            name: if name.is_empty() {
                host.to_string()
            } else {
                name.to_string()
            },
            host: host.to_string(),
            port: port.clamp(1, 65535),
            user: user.to_string(),
            created: now(),
        };
        std::fs::create_dir_all(&self.dir).context("create hosts dir")?;
        let raw = serde_json::to_string_pretty(&h)?;
        let tmp = self.host_file(&id).with_extension("tmp");
        std::fs::write(&tmp, raw)?;
        std::fs::rename(&tmp, self.host_file(&id))?;
        // no agent probe here: the host has no key installed yet, the ssh
        // probe would just block for its full timeout
        Ok(self.status_of_impl(&id, false))
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let p = self.host_file(id);
        if !p.exists() {
            bail!("no host \"{id}\"");
        }
        std::fs::remove_file(p)?;
        let _ = std::fs::remove_dir_all(self.key_dir(id));
        let _ = std::fs::remove_file(self.dir.join(format!("{id}.state")));
        Ok(())
    }

    /// generate the per-host key pair lazily (ssh-keygen, unencrypted
    /// ed25519); add() stays instant even without an ssh client installed
    fn ensure_key(&self, id: &str) -> Result<PathBuf> {
        let dir = self.key_dir(id);
        std::fs::create_dir_all(&dir)?;
        let key = dir.join("id_ed25519");
        if key.exists() {
            return Ok(key);
        }
        let out = std::process::Command::new("ssh-keygen")
            .args(["-t", "ed25519", "-N", "", "-q", "-f"])
            .arg(&key)
            .output()
            .context("run ssh-keygen")?;
        if !out.status.success() {
            bail!(
                "ssh-keygen failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(key)
    }

    /// the public key to install into the host's authorized_keys
    pub fn pubkey(&self, id: &str) -> Result<String> {
        self.ensure_key(id)?;
        std::fs::read_to_string(self.key_dir(id).join("id_ed25519.pub"))
            .map(|s| s.trim().to_string())
            .context("read public key")
    }

    fn target(&self, id: &str) -> Result<crate::sshx::SshTarget> {
        let h = self
            .read_one(id)
            .ok_or_else(|| anyhow::anyhow!("no host \"{id}\""))?;
        let dir = self.key_dir(id);
        std::fs::create_dir_all(&dir)?;
        Ok(crate::sshx::SshTarget::remote(
            &dir, &h.host, h.port, &h.user,
        ))
    }

    fn agent_present(&self, h: &RemoteHost) -> Result<bool> {
        let t = self.target(&h.id)?;
        let Some(bin) = crate::sshx::find_ssh() else {
            bail!("no ssh client on the host");
        };
        let cancel = AtomicBool::new(false);
        let out = crate::sshx::exec(
            &bin,
            &t,
            "test -x $HOME/.cargo/bin/hi-derola && echo yes || echo no",
            Duration::from_secs(10),
            &cancel,
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(out.code == 0 && out.stdout.contains("yes"))
    }

    /// connectivity check: `true` over ssh, state persisted for the ui
    pub fn check(&self, id: &str) -> HostStatus {
        let st = self.do_check(id);
        self.save_state(id, &st);
        self.status_of_impl(id, st.state == "ready")
    }

    fn do_check(&self, id: &str) -> HostStateRec {
        let Some(h) = self.read_one(id) else {
            return HostStateRec {
                state: "failed".into(),
                error: Some(format!("no host \"{id}\"")),
                checked: Some(now()),
            };
        };
        let res = (|| -> Result<bool> {
            let t = self.target(id)?;
            let bin = crate::sshx::find_ssh()
                .ok_or_else(|| anyhow::anyhow!("no ssh client on the host"))?;
            let cancel = AtomicBool::new(false);
            let out = crate::sshx::exec(&bin, &t, "true", Duration::from_secs(10), &cancel)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            if out.code != 0 {
                bail!("{}", out.stderr.trim());
            }
            Ok(true)
        })();
        match res {
            Ok(_) => HostStateRec {
                state: "ready".into(),
                error: None,
                checked: Some(now()),
            },
            Err(e) => HostStateRec {
                state: "failed".into(),
                error: Some(format!("{e:#}")),
                checked: Some(now()),
            },
        }
        .with_host(&h)
    }

    fn save_state(&self, id: &str, st: &HostStateRec) {
        let p = self.dir.join(format!("{id}.state"));
        if let Ok(raw) = serde_json::to_string(st) {
            let _ = std::fs::write(&p, raw);
        }
    }

    /// probe=false skips the (blocking) ssh agent check
    fn status_of_impl(&self, id: &str, probe: bool) -> HostStatus {
        let h = self.read_one(id).unwrap_or(RemoteHost {
            id: id.to_string(),
            name: id.to_string(),
            host: String::new(),
            port: 22,
            user: String::new(),
            created: 0,
        });
        let st = read_state(&self.dir, id);
        let agent = if probe {
            self.agent_present(&h).ok()
        } else {
            None
        };
        HostStatus {
            host: h,
            state: st.state,
            error: st.error,
            checked: st.checked,
            agent,
        }
    }

    /// run one command on the host (gui ssh console + connectivity internals)
    pub fn exec(
        &self,
        id: &str,
        command: &str,
        timeout_secs: Option<u64>,
    ) -> Result<crate::sshx::SshOut> {
        let t = self.target(id)?;
        let bin =
            crate::sshx::find_ssh().ok_or_else(|| anyhow::anyhow!("no ssh client on the host"))?;
        let cancel = AtomicBool::new(false);
        crate::sshx::exec(
            &bin,
            &t,
            command,
            Duration::from_secs(timeout_secs.unwrap_or(15).clamp(1, 600)),
            &cancel,
        )
        .map_err(|e| anyhow::anyhow!("{e}"))
    }

    /// pipe stdin bytes into a remote command (`cat > file` style writes);
    /// returns (exit code, tail of the remote output for error messages)
    pub fn stream(
        &self,
        id: &str,
        command: &str,
        stdin: &[u8],
        timeout_secs: Option<u64>,
    ) -> Result<(i32, String)> {
        let t = self.target(id)?;
        let bin =
            crate::sshx::find_ssh().ok_or_else(|| anyhow::anyhow!("no ssh client on the host"))?;
        let cancel = AtomicBool::new(false);
        let mut tail: Vec<String> = Vec::new();
        let code = crate::sshx::stream(
            &bin,
            &t,
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
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok((code, tail.join("\n")))
    }

    /// open a terminal window into the host (plain shell or the agent tui)
    pub fn open_terminal(&self, id: &str, agent: bool) -> Result<()> {
        let t = self.target(id)?;
        let bin =
            crate::sshx::find_ssh().ok_or_else(|| anyhow::anyhow!("no ssh client on the host"))?;
        let args = crate::sshx::terminal_cmdline(&t, agent);
        crate::sshx::spawn_terminal(&bin, &args).map_err(|e| anyhow::anyhow!("{e}"))
    }

    /// wait for ssh with a deadline (used right after add, when the key is
    /// not installed yet this fails fast with the pubkey hint)
    pub fn wait_ready_secs(&self, id: &str, secs: u64) -> Result<u64, String> {
        let t = self.target(id).map_err(|e| format!("{e:#}"))?;
        let bin = crate::sshx::find_ssh().ok_or_else(|| "no ssh client on the host".to_string())?;
        let cancel = AtomicBool::new(false);
        crate::sshx::wait_ready(
            &bin,
            &t,
            &cancel,
            Instant::now() + Duration::from_secs(secs),
        )
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HostStateRec {
    state: String,
    error: Option<String>,
    checked: Option<u64>,
}

impl HostStateRec {
    fn with_host(self, _h: &RemoteHost) -> HostStateRec {
        self
    }
}

fn read_state(dir: &Path, id: &str) -> HostStateRec {
    let p = dir.join(format!("{id}.state"));
    std::fs::read_to_string(&p)
        .ok()
        .and_then(|raw| serde_json::from_str::<HostStateRec>(&raw).ok())
        .unwrap_or(HostStateRec {
            state: "new".into(),
            error: None,
            checked: None,
        })
}

pub fn hosts_base() -> PathBuf {
    if let Ok(d) = std::env::var("HI_DEROLA_HOSTS_DIR") {
        if !d.trim().is_empty() {
            return PathBuf::from(d);
        }
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("hi-derola")
        .join("hosts")
}

impl RemoteHost {
    pub fn ssh_label(&self) -> String {
        format!("{}@{}:{}", self.user, self.host, self.port)
    }
}

impl HostManager {
    pub fn target_of(&self, id: &str) -> Result<crate::sshx::SshTarget> {
        self.target(id)
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

    fn keygen_available() -> bool {
        std::process::Command::new("ssh-keygen")
            .arg("-h")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
    }

    #[test]
    fn add_list_delete() {
        let base = tmpdir("hiderola-hosts");
        let m = HostManager::at(base.clone());
        let st = m.add("box", "root", "10.1.1.1", 2222).unwrap();
        assert_eq!(st.host.name, "box");
        assert_eq!(st.host.port, 2222);
        assert_eq!(st.state, "new");
        let list = m.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].host.host, "10.1.1.1");
        m.delete(&st.host.id).unwrap();
        assert!(m.list().is_empty());
        assert!(m.delete("nope").is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn pubkey_and_missing_host() {
        if !keygen_available() {
            return;
        }
        let base = tmpdir("hiderola-hosts-pk");
        let m = HostManager::at(base.clone());
        let st = m.add("k", "u", "h", 22).unwrap();
        let pk = m.pubkey(&st.host.id).unwrap();
        assert!(pk.starts_with("ssh-ed25519 "));
        assert!(m.target_of(&st.host.id).is_ok());
        assert!(m.target_of("missing").is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
