//! Self-update — `hi-derola update` / `--version` / `/update`.
//!
//! The release workflow (`.github/workflows/release.yml`) uploads raw
//! binaries named `hi-derola-<target-triple>[.exe]` for every `v*` tag. The
//! updater checks the latest release through the github api, downloads the
//! asset for the running platform and swaps the running binary with a rename
//! dance (renaming a running image is allowed, even on windows), so no
//! archive/zip support is needed.

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use std::cmp::Ordering;
use std::path::PathBuf;

pub const REPO: &str = "mel0k1/hi-derola";

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// semver-ish compare of version strings / release tags ("v" prefix allowed,
/// dot segments compared numerically, missing segments are 0)
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let parse = |v: &str| -> Vec<u64> {
        v.trim()
            .trim_start_matches('v')
            .split('.')
            .map(|seg| {
                let digits: String = seg.chars().take_while(|c| c.is_ascii_digit()).collect();
                digits.parse::<u64>().unwrap_or(0)
            })
            .collect()
    };
    let (a, b) = (parse(a), parse(b));
    let len = a.len().max(b.len());
    for i in 0..len {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x.cmp(&y);
        }
    }
    Ordering::Equal
}

#[derive(Debug, Clone, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub tag: String,
    pub notes: String,
    pub assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    body: String,
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

pub fn parse_release(v: &serde_json::Value) -> Result<Release> {
    let r: GhRelease = serde_json::from_value(v.clone()).context("unexpected release json")?;
    Ok(Release {
        tag: r.tag_name,
        notes: r.body,
        assets: r
            .assets
            .into_iter()
            .map(|a| Asset {
                name: a.name,
                url: a.browser_download_url,
            })
            .collect(),
    })
}

/// which release asset matches the running platform
pub fn asset_for(r: &Release) -> Option<&Asset> {
    asset_for_triple(r, target_triple_name(), std::env::consts::EXE_SUFFIX)
}

fn asset_for_triple<'a>(r: &'a Release, triple: &str, exe: &str) -> Option<&'a Asset> {
    let exact = format!("hi-derola-{triple}{exe}");
    r.assets.iter().find(|a| a.name == exact).or_else(|| {
        // fallback is suffix-aware so a future `hi-derola-<triple>.tar.gz`
        // style asset is never downloaded as a raw binary
        r.assets
            .iter()
            .find(|a| a.name.contains(triple) && a.name.ends_with(exe))
    })
}

/// triple used in release asset names (mirrors the release workflow matrix)
pub fn target_triple_name() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        _ => "unknown",
    }
}

fn client() -> Result<reqwest::Client> {
    let mut headers = reqwest::header::HeaderMap::new();
    // unauthenticated github api allows 60 req/h per ip; any of these tokens
    // bumps the limit to 5000/h
    let token = std::env::var("HI_DEROLA_GITHUB_TOKEN")
        .or_else(|_| std::env::var("GITHUB_TOKEN"))
        .or_else(|_| std::env::var("GH_TOKEN"))
        .ok()
        .filter(|t| !t.trim().is_empty());
    if let Some(t) = token {
        if let Ok(v) = reqwest::header::HeaderValue::from_str(&format!("Bearer {t}")) {
            headers.insert(reqwest::header::AUTHORIZATION, v);
        }
    }
    reqwest::Client::builder()
        .user_agent(format!("hi-derola-selfupdate/{}", current_version()))
        .default_headers(headers)
        .build()
        .context("http client")
}

/// latest github release; Ok(None) when the repo has no releases yet
pub async fn latest() -> Result<Option<Release>> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp = client()?
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .context("github api request")?;
    if resp.status().as_u16() == 404 {
        return Ok(None);
    }
    if resp.status().as_u16() == 403 {
        bail!(
            "github api rate limit exceeded — retry in an hour or set GITHUB_TOKEN to raise the limit"
        );
    }
    let v: serde_json::Value = resp.error_for_status()?.json().await?;
    Ok(Some(parse_release(&v)?))
}

async fn download(url: &str) -> Result<Vec<u8>> {
    let resp = client()?.get(url).send().await?.error_for_status()?;
    let bytes = resp.bytes().await?;
    // a real hi-derola binary is way past this; a truncated/corrupt download
    // should never reach the swap step
    if bytes.len() < 1024 * 1024 {
        bail!(
            "downloaded asset is suspiciously small ({} bytes), aborting",
            bytes.len()
        );
    }
    Ok(bytes.to_vec())
}

/// replace the running binary: write `<exe>.download`, rename the current
/// image to `<exe>.old` (legal while it runs, even on windows), move the new
/// one into place; on failure the old image is restored
pub fn apply(bytes: &[u8]) -> Result<PathBuf> {
    let exe = std::env::current_exe()?
        .canonicalize()
        .unwrap_or_else(|_| std::env::current_exe().expect("current_exe"));
    let new = PathBuf::from(format!("{}.download", exe.display()));
    let old = PathBuf::from(format!("{}.old", exe.display()));
    std::fs::write(&new, bytes).with_context(|| format!("write {}", new.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755));
    }
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).with_context(|| format!("move {} aside", exe.display()))?;
    match std::fs::rename(&new, &exe) {
        Ok(_) => {}
        Err(e) => {
            let _ = std::fs::rename(&old, &exe);
            let _ = std::fs::remove_file(&new);
            return Err(e).context("place the new binary");
        }
    }
    // best effort: on windows the running image keeps the file mapped and the
    // delete fails — the leftover .old disappears on the next update attempt
    let _ = std::fs::remove_file(&old);
    Ok(exe)
}

/// full flow with step logging; returns the final message for the chat
pub async fn run(mut log: impl FnMut(&str)) -> Result<String> {
    log(&format!(
        "checking github releases (current {})...",
        current_version()
    ));
    let rel = match latest().await? {
        Some(r) => r,
        None => return Ok(format!(
            "no github releases yet — install from source: cargo install --git https://github.com/{REPO} hi-derola"
        )),
    };
    match compare_versions(current_version(), &rel.tag) {
        Ordering::Equal | Ordering::Greater => {
            return Ok(format!(
                "already up to date ({}; latest release {})",
                current_version(),
                rel.tag
            ));
        }
        Ordering::Less => {}
    }
    let triple = target_triple_name();
    let asset = asset_for(&rel).ok_or_else(|| {
        anyhow!(
            "release {} has no binary for {triple} — install from source: cargo install --git https://github.com/{REPO} hi-derola",
            rel.tag
        )
    })?;
    log(&format!("downloading {}...", asset.name));
    let bytes = download(&asset.url).await?;
    log(&format!(
        "applying update ({:.1} MiB)...",
        bytes.len() as f64 / (1024.0 * 1024.0)
    ));
    let path = apply(&bytes)?;
    Ok(format!(
        "updated to {} — restart hi-derola to run it ({})",
        rel.tag,
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare() {
        assert_eq!(compare_versions("0.2.0", "v0.2.0"), Ordering::Equal);
        assert_eq!(compare_versions("0.10.0", "0.9.0"), Ordering::Greater);
        assert_eq!(compare_versions("0.2.0", "0.2.1"), Ordering::Less);
        assert_eq!(compare_versions("1.0", "1.0.0"), Ordering::Equal);
        assert_eq!(compare_versions("0.2.0", "0.2.0-rc1"), Ordering::Equal);
        assert_eq!(compare_versions("v1.2.3", "0.9.9"), Ordering::Greater);
    }

    #[test]
    fn parse_release_json() {
        let v: serde_json::Value = serde_json::json!({
            "tag_name": "v0.2.0",
            "body": "changes here",
            "assets": [
                {"name": "hi-derola-x86_64-pc-windows-msvc.exe",
                 "browser_download_url": "https://x/hi-derola-x86_64-pc-windows-msvc.exe"},
                {"name": "hi-derola-x86_64-unknown-linux-gnu",
                 "browser_download_url": "https://x/hi-derola-x86_64-unknown-linux-gnu"}
            ]
        });
        let r = parse_release(&v).unwrap();
        assert_eq!(r.tag, "v0.2.0");
        assert_eq!(r.assets.len(), 2);
        assert_eq!(r.assets[0].name, "hi-derola-x86_64-pc-windows-msvc.exe");
    }

    #[test]
    fn asset_matching() {
        let rel = |names: &[&str]| Release {
            tag: "v1".into(),
            notes: String::new(),
            assets: names
                .iter()
                .map(|n| Asset {
                    name: n.to_string(),
                    url: format!("https://x/{n}"),
                })
                .collect(),
        };
        let win = rel(&["hi-derola-x86_64-pc-windows-msvc.exe"]);
        let lin = rel(&["hi-derola-x86_64-unknown-linux-gnu"]);
        let both = rel(&[
            "hi-derola-x86_64-pc-windows-msvc.exe",
            "hi-derola-x86_64-unknown-linux-gnu",
        ]);
        assert_eq!(
            asset_for_triple(&win, "x86_64-pc-windows-msvc", ".exe")
                .unwrap()
                .name,
            "hi-derola-x86_64-pc-windows-msvc.exe"
        );
        assert_eq!(
            asset_for_triple(&lin, "x86_64-unknown-linux-gnu", "")
                .unwrap()
                .name,
            "hi-derola-x86_64-unknown-linux-gnu"
        );
        // wrong platform never matches
        assert!(asset_for_triple(&lin, "x86_64-pc-windows-msvc", ".exe").is_none());
        assert!(asset_for_triple(&win, "x86_64-unknown-linux-gnu", "").is_none());
        // the suffix must match too: a windows client never takes a tarball
        let tar = rel(&["hi-derola-x86_64-pc-windows-msvc.tar.gz"]);
        assert!(asset_for_triple(&tar, "x86_64-pc-windows-msvc", ".exe").is_none());
        let both_win = rel(&[
            "hi-derola-x86_64-pc-windows-msvc.tar.gz",
            "hi-derola-x86_64-pc-windows-msvc.exe",
        ]);
        assert_eq!(
            asset_for_triple(&both_win, "x86_64-pc-windows-msvc", ".exe")
                .unwrap()
                .name,
            "hi-derola-x86_64-pc-windows-msvc.exe"
        );
        // exact name first, suffix-aware contains() as the fallback (e.g. a
        // version-prefixed asset name)
        let loose = rel(&["hi-derola-v0.2.0-x86_64-pc-windows-msvc.exe"]);
        assert_eq!(
            asset_for_triple(&loose, "x86_64-pc-windows-msvc", ".exe")
                .unwrap()
                .name,
            "hi-derola-v0.2.0-x86_64-pc-windows-msvc.exe"
        );
    }

    #[test]
    fn triple_names_are_known() {
        assert_ne!(target_triple_name(), "unknown");
    }
}
