// `oxiremote update` — pull the latest GitHub release for our target triple,
// verify SHA256 against the release manifest, and atomic-replace the running
// binary. No long-lived signing key (cargo-dist–style hash chain only) for
// v0.1; ed25519 verification is on the v0.2 hit list.
//
// Wire-format expectations (must stay in sync with `.github/workflows/release.yml`):
// - Asset name: `oxiremote-<version>-<target>.tar.gz` (mac/linux) or `.zip` (windows)
// - Manifest:   `oxiremote-<version>-sha256.txt` containing one line per asset
//                `<hex-sha256>  <asset-filename>`

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const RELEASE_REPO: &str = "nhtera/oxiremote";
const USER_AGENT: &str = concat!("oxiremote-updater/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    assets: Vec<GhAsset>,
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

/// Compile-time `cfg!` map → release target triple. Returning `None` from a
/// platform we don't ship binaries for keeps `oxiremote update` from
/// confidently downloading the wrong artifact.
fn current_target() -> Option<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("aarch64-apple-darwin")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("x86_64-apple-darwin")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu")) {
        Some("x86_64-unknown-linux-gnu")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu")) {
        Some("aarch64-unknown-linux-gnu")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("x86_64-pc-windows-msvc")
    } else {
        None
    }
}

fn asset_extension() -> &'static str {
    if cfg!(target_os = "windows") {
        "zip"
    } else {
        "tar.gz"
    }
}

/// Check whether a newer release is available. Returns the latest version tag
/// (without `v` prefix) if it differs from the compiled-in version, or `None`
/// if already up-to-date or the check fails (network unavailable, etc.).
///
/// Intentionally non-async and synchronous so it can be called from the TUI
/// startup path before the tokio runtime is running on the server thread.
pub fn check_latest() -> Option<String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    rt.block_on(async {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .ok()?;
        let body = client
            .get(format!(
                "https://api.github.com/repos/{RELEASE_REPO}/releases/latest"
            ))
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?
            .text()
            .await
            .ok()?;
        let release: GhRelease = serde_json::from_str(&body).ok()?;
        let latest = release
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&release.tag_name)
            .to_string();
        let current = env!("CARGO_PKG_VERSION");
        if latest != current { Some(latest) } else { None }
    })
}

/// Result of a single `update::run()` invocation. The TUI uses this to break
/// out of the menu loop on success — without it the cached
/// `update_version = Some(latest)` (line in tui/mod.rs) plus the still-old
/// `env!("CARGO_PKG_VERSION")` of the running process keep the menu showing
/// "Update available" forever, and users hit Enter again thinking it failed,
/// re-downloading in a loop.
#[derive(Debug, Clone)]
pub enum UpdateOutcome {
    /// `current_version == latest_version` — no work performed.
    NotNeeded,
    /// New binary written; running process still hosts the old code, so the
    /// caller MUST exit and let the user relaunch to pick up the change.
    Applied(String),
}

pub fn run() -> Result<UpdateOutcome> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;
    rt.block_on(run_async())
}

async fn run_async() -> Result<UpdateOutcome> {
    let target = current_target().ok_or_else(|| {
        anyhow!(
            "no prebuilt release for this target ({}/{}) — install from source",
            std::env::consts::OS,
            std::env::consts::ARCH,
        )
    })?;

    let current_version = env!("CARGO_PKG_VERSION");
    println!("Current version: {current_version}");
    println!("Checking GitHub releases…");

    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("http client")?;

    let release_body = client
        .get(format!(
            "https://api.github.com/repos/{RELEASE_REPO}/releases/latest"
        ))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .context("fetch latest release metadata")?
        .error_for_status()
        .context("github API returned non-2xx — has the first release been published?")?
        .text()
        .await
        .context("read release metadata body")?;
    let release: GhRelease =
        serde_json::from_str(&release_body).context("parse release metadata")?;

    let latest_version = release.tag_name.strip_prefix('v').unwrap_or(&release.tag_name);
    if latest_version == current_version {
        println!("Already on latest ({latest_version}).");
        return Ok(UpdateOutcome::NotNeeded);
    }
    println!("Latest version:  {latest_version}");

    let asset_name = format!("oxiremote-{latest_version}-{target}.{}", asset_extension());
    let manifest_name = format!("oxiremote-{latest_version}-sha256.txt");

    let asset = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .ok_or_else(|| anyhow!("release {latest_version} has no asset named {asset_name}"))?;
    let manifest_asset = release
        .assets
        .iter()
        .find(|a| a.name == manifest_name)
        .ok_or_else(|| anyhow!("release {latest_version} has no manifest {manifest_name}"))?;

    println!("Downloading {asset_name}…");
    let archive_bytes = client
        .get(&asset.browser_download_url)
        .send()
        .await
        .context("download asset")?
        .error_for_status()?
        .bytes()
        .await
        .context("read asset body")?;

    let manifest_text = client
        .get(&manifest_asset.browser_download_url)
        .send()
        .await
        .context("download sha256 manifest")?
        .error_for_status()?
        .text()
        .await
        .context("read manifest body")?;

    let expected_sha = manifest_text
        .lines()
        .find_map(|line| {
            let mut parts = line.split_whitespace();
            let sha = parts.next()?;
            let name = parts.next()?;
            if name == asset_name {
                Some(sha.to_string())
            } else {
                None
            }
        })
        .ok_or_else(|| anyhow!("manifest does not list {asset_name}"))?;

    let actual_sha = hex::encode(Sha256::digest(&archive_bytes));
    if actual_sha != expected_sha {
        bail!(
            "checksum mismatch — refusing to replace binary.\n  expected: {expected_sha}\n  actual:   {actual_sha}"
        );
    }
    println!("Checksum verified.");

    let current_exe = std::env::current_exe().context("current_exe")?;
    let target_dir = current_exe
        .parent()
        .ok_or_else(|| anyhow!("current_exe has no parent"))?
        .to_path_buf();

    let archive_stem = format!("oxiremote-{latest_version}-{target}");
    let extracted = extract_binary(&archive_bytes, &target_dir, &archive_stem)
        .context("extract archive")?;

    atomic_replace(&current_exe, &extracted).context("atomic replace")?;
    println!("Updated to {latest_version}. Restart the agent to pick up the new binary.");
    Ok(UpdateOutcome::Applied(latest_version.to_string()))
}

/// Pulls the `oxiremote` (or `oxiremote.exe`) binary out of the archive into
/// `<target_dir>/oxiremote.new`. Returns the new-binary path.
///
/// `archive_stem` is the top-level directory inside the archive
/// (`oxiremote-<version>-<target>`) — release artifacts nest the binary one
/// level deep, so we strip that prefix during extraction.
fn extract_binary(archive: &[u8], target_dir: &Path, archive_stem: &str) -> Result<PathBuf> {
    let bin_basename = if cfg!(target_os = "windows") {
        "oxiremote.exe"
    } else {
        "oxiremote"
    };
    let dest = target_dir.join(format!("{bin_basename}.new"));

    if cfg!(target_os = "windows") {
        // ZIP path — windows asset. We don't depend on the `zip` crate
        // elsewhere; shell out to `tar` (Win10+ ships bsdtar). Cleaner
        // than a new dep.
        //
        // The previous version extracted with `-C target_dir
        // --strip-components=1`, which dropped `oxiremote.exe` directly
        // beside the running binary. Windows holds an exclusive lock on
        // the live exe → tar fails with "Can't unlink already-existing
        // object" before the .new-rename + atomic_replace dance can run.
        // Extract into a scratch subdir instead so tar never touches the
        // live exe, then move the binary out and clean up the scratch.
        let tmp = target_dir.join("oxiremote-update-archive.zip");
        let scratch = target_dir.join("oxiremote-update-scratch");
        fs::write(&tmp, archive).context("write archive temp")?;
        let _ = fs::remove_dir_all(&scratch); // stale leftover from a prior crash
        fs::create_dir_all(&scratch).context("create scratch dir")?;

        let nested = format!("{archive_stem}/{bin_basename}");
        let status = std::process::Command::new("tar")
            .args(["-xf"])
            .arg(&tmp)
            .args(["-C"])
            .arg(&scratch)
            .arg("--strip-components=1")
            .arg(&nested)
            .status()
            .context("spawn tar")?;
        let _ = fs::remove_file(&tmp);
        if !status.success() {
            let _ = fs::remove_dir_all(&scratch);
            bail!("tar exited non-zero while extracting {bin_basename}");
        }

        if let Err(e) = fs::rename(scratch.join(bin_basename), &dest) {
            let _ = fs::remove_dir_all(&scratch);
            return Err(e).context("rename extracted binary to .new");
        }
        let _ = fs::remove_dir_all(&scratch);
    } else {
        // tar.gz path.
        let decoder = flate2::read::GzDecoder::new(archive);
        let mut tar = tar::Archive::new(decoder);
        let mut found = false;
        for entry in tar.entries().context("read tar entries")? {
            let mut entry = entry.context("tar entry")?;
            let path = entry.path().context("tar entry path")?.into_owned();
            // Tarballs may be flat or nest under a top-level dir; match by basename.
            if path.file_name().and_then(|s| s.to_str()) == Some(bin_basename) {
                let mut buf = Vec::with_capacity(20 * 1024 * 1024);
                entry.read_to_end(&mut buf).context("read tar entry body")?;
                fs::write(&dest, &buf).context("write extracted binary")?;
                found = true;
                break;
            }
        }
        if !found {
            bail!("archive did not contain {bin_basename}");
        }
        // Set exec bit so the rename produces an executable replacement.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&dest)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&dest, perms).context("chmod extracted binary")?;
        }
    }
    Ok(dest)
}

/// On Unix `rename` replaces atomically. On Windows, the running .exe is
/// locked, so we move the live binary aside (`.bak`) before installing the
/// new one — old binary is cleaned up on next launch.
fn atomic_replace(target: &Path, new_binary: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        fs::rename(new_binary, target).context("rename .new over current_exe")?;
    }
    #[cfg(windows)]
    {
        let bak = target.with_extension("exe.bak");
        let _ = fs::remove_file(&bak); // stale leftover from prior update
        fs::rename(target, &bak).context("move running exe aside (.bak)")?;
        if let Err(e) = fs::rename(new_binary, target) {
            // Best-effort rollback so we don't leave the user with no exe.
            let _ = fs::rename(&bak, target);
            return Err(e).context("rename .new over current_exe");
        }
        // The old `.bak` cannot be removed while the process holds the lock;
        // it'll be cleaned on next start by `cleanup_stale_bak()`.
    }
    Ok(())
}

/// Best-effort sweep of `<exe>.exe.bak` left behind by a Windows self-update
/// (the running process locks the file until exit). Called from `main`
/// during normal startup. No-op on Unix.
pub fn cleanup_stale_bak() {
    #[cfg(windows)]
    {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::fs::remove_file(exe.with_extension("exe.bak"));
        }
    }
}
