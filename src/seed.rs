//! cloud-init NoCloud seed for the sandbox VMs.
//!
//! Cloud images (Debian genericcloud, Ubuntu minimal cloudimg) run cloud-init
//! on first boot. cloud-init picks up a filesystem labeled `cidata`/`CIDATA`
//! and reads `user-data` + `meta-data` from it — so we build a small FAT
//! image (`seed.img`, attached as a second virtio drive) that creates the
//! wizard's login with the sandbox ssh key baked in:
//!
//! - login from the wizard (default `derola`), shell bash, password locked
//! - `ssh_authorized_keys` = the sandbox's generated ed25519 public key
//! - sudo: passwordless when the wizard's root flag is on, none when off
//! - `ssh_pwauth: false` — the VM is reachable by key only
//!
//! The private key (`id_ed25519`) never leaves the sandbox directory; the
//! ssh executor (see `sshx`) uses it to reach the VM on the forwarded port.

use anyhow::{anyhow, bail, Context, Result};
use fatfs::{FormatVolumeOptions, FsOptions};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

/// default in-VM login (wizard may override)
pub const DEFAULT_LOGIN: &str = "derola";
/// size of the generated seed image (two small text files fit easily)
pub const SEED_SIZE: u64 = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshKeypair {
    /// OpenSSH PEM ("-----BEGIN OPENSSH PRIVATE KEY-----")
    pub private_pem: String,
    /// authorized_keys line ("ssh-ed25519 AAAA... comment")
    pub authorized_key: String,
}

/// linux username guard: lowercase start, then [a-z0-9_-], 1..=32 chars,
/// not `root` (cloud-init would fight the distro root account)
pub fn validate_login(login: &str) -> Result<()> {
    let mut chars = login.chars();
    let bad = match chars.next() {
        None => true,
        Some(c) if !c.is_ascii_lowercase() => true,
        Some(_) => {
            login.len() > 32
                || !login
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
                || login == "root"
        }
    };
    if bad {
        bail!(
            "login must be 1-32 chars: a-z, digits, '-' or '_', starting with a letter (not \"root\")"
        );
    }
    Ok(())
}

/// #cloud-config document; `root` toggles passwordless sudo for the login
pub fn user_data(login: &str, authorized_key: &str, root: bool) -> String {
    let sudo = if root {
        "\"ALL=(ALL) NOPASSWD:ALL\""
    } else {
        "false"
    };
    format!(
        "#cloud-config\n\
         hostname: {login}-vm\n\
         manage_etc_hosts: true\n\
         ssh_pwauth: false\n\
         chpasswd:\n\
         \x20 expire: false\n\
         users:\n\
         \x20 - name: {login}\n\
         \x20   shell: /bin/bash\n\
         \x20   lock_passwd: true\n\
         \x20   sudo: {sudo}\n\
         \x20   ssh_authorized_keys:\n\
         \x20     - {authorized_key}\n"
    )
}

/// instance identity; the sandbox id doubles as a stable hostname
pub fn meta_data(id: &str) -> String {
    format!("instance-id: hiderola-{id}\nlocal-hostname: {id}\n")
}

/// generate a fresh ed25519 keypair in OpenSSH format
pub fn generate_keypair(comment: &str) -> Result<SshKeypair> {
    use ssh_key::{Algorithm, LineEnding, PrivateKey};
    let mut key = PrivateKey::random(&mut ssh_key::rand_core::OsRng, Algorithm::Ed25519)
        .context("generate ed25519 ssh keypair")?;
    key.set_comment(comment);
    let mut pubk = key.public_key().clone();
    pubk.set_comment(comment);
    let authorized_key = pubk.to_openssh()?;
    let private_pem = key.to_openssh(LineEnding::LF)?.to_string();
    Ok(SshKeypair {
        private_pem,
        authorized_key,
    })
}

/// build the FAT "CIDATA" volume with user-data + meta-data inside
pub fn build_seed_image(path: &Path, user_data: &str, meta_data: &str) -> Result<()> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .with_context(|| format!("create {}", path.display()))?;
    file.set_len(SEED_SIZE)
        .with_context(|| format!("size {}", path.display()))?;
    let mut file = file;
    fatfs::format_volume(
        &mut file,
        FormatVolumeOptions::new()
            .volume_id(0x4844_5241) // "HDRA"
            .volume_label(*b"CIDATA     "),
    )
    .map_err(|e| anyhow!("format seed volume: {e}"))?;
    let fs_opts = FsOptions::new();
    {
        let fs =
            fatfs::FileSystem::new(file, fs_opts).map_err(|e| anyhow!("open seed volume: {e}"))?;
        let root = fs.root_dir();
        let mut ud = root
            .create_file("user-data")
            .map_err(|e| anyhow!("seed user-data: {e}"))?;
        ud.write_all(user_data.as_bytes())
            .map_err(|e| anyhow!("write user-data: {e}"))?;
        let mut md = root
            .create_file("meta-data")
            .map_err(|e| anyhow!("seed meta-data: {e}"))?;
        md.write_all(meta_data.as_bytes())
            .map_err(|e| anyhow!("write meta-data: {e}"))?;
        // fs (and its buffers) are dropped here, flushing the volume
    }
    Ok(())
}

/// make sure the sandbox dir holds a consistent keypair + seed image:
/// missing pieces are (re)generated, existing ones are reused so a VM that
/// already booted keeps accepting the same key. Safe to call repeatedly.
pub fn ensure_seed(dir: &Path, id: &str, login: &str, root: bool) -> Result<SshKeypair> {
    validate_login(login)?;
    let key_path = dir.join("id_ed25519");
    let pub_path = dir.join("id_ed25519.pub");
    let seed_path = dir.join("seed.img");

    let keypair = if key_path.is_file() && pub_path.is_file() {
        let private_pem = std::fs::read_to_string(&key_path)
            .with_context(|| format!("read {}", key_path.display()))?;
        let authorized_key = std::fs::read_to_string(&pub_path)
            .with_context(|| format!("read {}", pub_path.display()))?
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        SshKeypair {
            private_pem,
            authorized_key,
        }
    } else {
        let kp = generate_keypair(&format!("hiderola-{id}"))?;
        std::fs::write(&key_path, &kp.private_pem)
            .with_context(|| format!("write {}", key_path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::write(&pub_path, format!("{}\n", kp.authorized_key))
            .with_context(|| format!("write {}", pub_path.display()))?;
        kp
    };

    if !seed_path.is_file() {
        build_seed_image(
            &seed_path,
            &user_data(login, &keypair.authorized_key, root),
            &meta_data(id),
        )?;
    }
    Ok(keypair)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hiderola-seed-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn login_validation() {
        assert!(validate_login("derola").is_ok());
        assert!(validate_login("a").is_ok());
        assert!(validate_login("a-b_c9").is_ok());
        assert!(validate_login("").is_err());
        assert!(validate_login("Root").is_err());
        assert!(validate_login("root").is_err());
        assert!(validate_login("9lives").is_err());
        assert!(validate_login("has space").is_err());
        assert!(validate_login("unicode-ё").is_err());
        assert!(validate_login(&"a".repeat(33)).is_err());
        assert!(validate_login(&"a".repeat(32)).is_ok());
    }

    #[test]
    fn user_data_shape_root_toggle() {
        let ud = user_data("derola", "ssh-ed25519 AAAAtest hiderola-x", true);
        assert!(ud.starts_with("#cloud-config\n"));
        assert!(ud.contains("name: derola\n"));
        assert!(ud.contains("sudo: \"ALL=(ALL) NOPASSWD:ALL\""));
        assert!(ud.contains("- ssh-ed25519 AAAAtest hiderola-x"));
        assert!(ud.contains("ssh_pwauth: false"));
        assert!(ud.contains("lock_passwd: true"));

        let ud = user_data("joe", "ssh-ed25519 K", false);
        assert!(ud.contains("sudo: false"));
        assert!(!ud.contains("NOPASSWD"));
        assert!(ud.contains("hostname: joe-vm\n"));
    }

    #[test]
    fn meta_data_shape() {
        let md = meta_data("test-01");
        assert!(md.contains("instance-id: hiderola-test-01"));
        assert!(md.contains("local-hostname: test-01"));
    }

    #[test]
    fn keypair_roundtrip() {
        let kp = generate_keypair("hiderola-t").unwrap();
        assert!(kp
            .private_pem
            .starts_with("-----BEGIN OPENSSH PRIVATE KEY-----"));
        assert!(kp
            .private_pem
            .ends_with("-----END OPENSSH PRIVATE KEY-----\n"));
        let pubk = ssh_key::PublicKey::from_openssh(&kp.authorized_key).unwrap();
        assert_eq!(pubk.algorithm(), ssh_key::Algorithm::Ed25519);
        assert_eq!(pubk.comment(), "hiderola-t");
        // two keypairs must differ
        let kp2 = generate_keypair("hiderola-t").unwrap();
        assert_ne!(kp.private_pem, kp2.private_pem);
    }

    #[test]
    fn seed_image_is_cidata_volume_with_both_files() {
        let dir = temp_dir("img");
        let img = dir.join("seed.img");
        build_seed_image(&img, "USERDATA-CONTENT", "METADATA-CONTENT").unwrap();
        assert_eq!(img.metadata().unwrap().len(), SEED_SIZE);

        // reopen and read the files back through fatfs
        let file = std::fs::OpenOptions::new().read(true).open(&img).unwrap();
        let fs = fatfs::FileSystem::new(file, FsOptions::new()).unwrap();
        let read = |name: &str| {
            let mut s = String::new();
            std::io::Read::read_to_string(&mut fs.root_dir().open_file(name).unwrap(), &mut s)
                .unwrap();
            s
        };
        assert_eq!(read("user-data"), "USERDATA-CONTENT");
        assert_eq!(read("meta-data"), "METADATA-CONTENT");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_seed_creates_and_reuses() {
        let dir = temp_dir("ensure");
        let id = "sbx-01";
        ensure_seed(&dir, id, "derola", true).unwrap();
        let key = dir.join("id_ed25519");
        let seed = dir.join("seed.img");
        assert!(key.is_file() && dir.join("id_ed25519.pub").is_file() && seed.is_file());
        let pem = std::fs::read_to_string(&key).unwrap();

        // second call reuses the same key and does not rebuild the seed
        let before = std::fs::metadata(&seed).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        ensure_seed(&dir, id, "derola", false).unwrap();
        assert_eq!(std::fs::read_to_string(&key).unwrap(), pem);
        assert_eq!(
            std::fs::metadata(&seed).unwrap().modified().unwrap(),
            before
        );

        // missing key -> regenerated together with a fresh seed
        std::fs::remove_file(&key).unwrap();
        std::fs::remove_file(&seed).unwrap();
        ensure_seed(&dir, id, "derola", false).unwrap();
        assert_ne!(std::fs::read_to_string(&key).unwrap(), pem);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_seed_rejects_bad_login() {
        let dir = temp_dir("badlogin");
        assert!(ensure_seed(&dir, "x", "root", false).is_err());
        assert!(!dir.join("seed.img").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
