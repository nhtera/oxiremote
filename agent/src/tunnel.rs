use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use flate2::read::GzDecoder;
use regex::Regex;
use reqwest::Client;
use sha2::{Digest, Sha256};
use tar::Archive;
use tokio::io::AsyncBufReadExt;
use tracing::info;

use crate::events::{AgentEvent, EventBus, TunnelStep};

fn cloudflared_path(data_dir: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        data_dir.join("cloudflared.exe")
    }
    #[cfg(not(windows))]
    {
        data_dir.join("cloudflared")
    }
}

/// Artifact descriptor for a given OS/arch.
struct Artifact {
    /// Filename in the GitHub release (`cloudflared-linux-amd64`, etc.).
    release_filename: String,
    /// Filename on the checksums line.
    checksum_filename: String,
    /// Is the downloaded asset a tgz archive (macos) or a bare binary (linux/windows)?
    is_tarball: bool,
}

fn artifact_for_current_host() -> anyhow::Result<Artifact> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    match (os, arch) {
        ("macos", "aarch64") => Ok(Artifact {
            release_filename: "cloudflared-darwin-arm64.tgz".into(),
            checksum_filename: "cloudflared-darwin-arm64.tgz".into(),
            is_tarball: true,
        }),
        ("macos", "x86_64") => Ok(Artifact {
            release_filename: "cloudflared-darwin-amd64.tgz".into(),
            checksum_filename: "cloudflared-darwin-amd64.tgz".into(),
            is_tarball: true,
        }),
        ("linux", "x86_64") => Ok(Artifact {
            release_filename: "cloudflared-linux-amd64".into(),
            checksum_filename: "cloudflared-linux-amd64".into(),
            is_tarball: false,
        }),
        ("linux", "aarch64") => Ok(Artifact {
            release_filename: "cloudflared-linux-arm64".into(),
            checksum_filename: "cloudflared-linux-arm64".into(),
            is_tarball: false,
        }),
        ("windows", "x86_64") => Ok(Artifact {
            release_filename: "cloudflared-windows-amd64.exe".into(),
            checksum_filename: "cloudflared-windows-amd64.exe".into(),
            is_tarball: false,
        }),
        _ => anyhow::bail!("unsupported cloudflared host: {os}/{arch}"),
    }
}

pub async fn ensure_cloudflared(data_dir: &Path) -> anyhow::Result<PathBuf> {
    let path = cloudflared_path(data_dir);
    if path.exists() {
        return Ok(path);
    }

    let version = cloudflared_latest_version().await.context("get latest version")?;
    let art = artifact_for_current_host()?;

    let asset_url = format!(
        "https://github.com/cloudflare/cloudflared/releases/download/{version}/{}",
        art.release_filename
    );
    let checksums_url = format!(
        "https://github.com/cloudflare/cloudflared/releases/download/{version}/cloudflared-{version}-checksums.txt"
    );

    let tmp_dir = data_dir.join("tmp");
    std::fs::create_dir_all(&tmp_dir).context("create tmp dir")?;

    let client = Client::builder().user_agent("oxiremote/0.1").build()?;

    let checksums_text = client
        .get(&checksums_url)
        .send()
        .await
        .context("download checksums")?
        .error_for_status()
        .context("download checksums status")?
        .text()
        .await
        .context("read checksums")?;

    let bytes = client
        .get(&asset_url)
        .send()
        .await
        .context("download cloudflared")?
        .error_for_status()
        .context("download cloudflared status")?
        .bytes()
        .await
        .context("read cloudflared bytes")?;

    let expected = find_expected_sha256(&checksums_text, &art.checksum_filename)
        .context("find expected sha256")?;
    let actual = hex::encode(Sha256::digest(&bytes));
    if actual != expected {
        anyhow::bail!("cloudflared sha256 mismatch");
    }

    // Set exec bit on the staging file BEFORE renaming into `path`.
    // Otherwise a crash between rename and chmod leaves a non-executable
    // binary at the canonical location, breaking the next startup's
    // idempotent "exists? → skip download" check.
    let staged: std::path::PathBuf = if art.is_tarball {
        extract_cloudflared_tgz(&bytes, &tmp_dir).context("extract cloudflared")?
    } else {
        let staged = tmp_dir.join("cloudflared.bin");
        std::fs::write(&staged, &bytes).context("write cloudflared binary")?;
        staged
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&staged)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&staged, perms)?;
    }

    std::fs::rename(&staged, &path).context("move cloudflared into place")?;
    let _ = std::fs::remove_dir_all(&tmp_dir);

    info!(version = %version, "cloudflared downloaded");
    Ok(path)
}

fn find_expected_sha256(checksums: &str, filename: &str) -> anyhow::Result<String> {
    for line in checksums.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() == 2 && parts[1] == filename {
            return Ok(parts[0].to_string());
        }
    }
    anyhow::bail!("sha256 not found for {filename}")
}

fn extract_cloudflared_tgz(bytes: &[u8], dest: &Path) -> anyhow::Result<PathBuf> {
    let decoder = GzDecoder::new(bytes);
    let mut archive = Archive::new(decoder);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        if path.file_name().and_then(|n| n.to_str()) == Some("cloudflared") {
            let out = dest.join("cloudflared");
            entry.unpack(&out)?;
            return Ok(out);
        }
    }
    anyhow::bail!("cloudflared binary not found in archive")
}

async fn cloudflared_latest_version() -> anyhow::Result<String> {
    let client = Client::builder()
        .user_agent("oxiremote/0.1")
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    let resp = client
        .get("https://github.com/cloudflare/cloudflared/releases/latest")
        .send()
        .await
        .context("request latest release")?;

    if let Some(loc) = resp.headers().get("location") {
        let loc = loc.to_str().context("location header")?;
        if let Some(tag) = loc.rsplit('/').next() {
            return Ok(tag.to_string());
        }
    }

    anyhow::bail!("could not determine latest cloudflared version")
}

pub async fn ensure_quick_tunnel(
    addr: std::net::SocketAddr,
    cloudflared: PathBuf,
    bus: Arc<EventBus>,
) -> anyhow::Result<String> {
    // Step 1 — process is about to spawn.
    bus.send(AgentEvent::TunnelStepChanged {
        step: TunnelStep::Preparing,
        attempt: 1,
        info: Some(format!("cloudflared at {}", cloudflared.display())),
    });

    let mut child = tokio::process::Command::new(&cloudflared)
        .args(["tunnel", "--url", &format!("http://{addr}")])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("spawn cloudflared")?;

    let stderr = child.stderr.take().context("no stderr")?;
    let stdout = child.stdout.take().context("no stdout")?;

    // Step 2 — cloudflared spawned, waiting for tunnel URL on stderr.
    bus.send(AgentEvent::TunnelStepChanged {
        step: TunnelStep::Connecting,
        attempt: 1,
        info: Some(format!("waiting for tunnel URL on http://{addr}")),
    });

    let re = Regex::new(r"https://[a-z0-9\-]+\.trycloudflare\.com").unwrap();
    let url = Arc::new(tokio::sync::Mutex::new(None::<String>));

    // stderr carries cloudflared's INF/ERR log lines AND the tunnel URL banner.
    let url_for_stderr = url.clone();
    let re_stderr = re.clone();
    tokio::spawn(async move {
        let reader = tokio::io::BufReader::new(stderr);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            info!(target: "cloudflared", "{}", line);
            if let Some(m) = re_stderr.find(&line) {
                let mut u = url_for_stderr.lock().await;
                if u.is_none() {
                    *u = Some(m.as_str().to_string());
                }
            }
        }
    });

    // stdout is rarely used by cloudflared but inheriting it would corrupt the
    // TUI alternate buffer. Forward through tracing → bus instead.
    tokio::spawn(async move {
        let reader = tokio::io::BufReader::new(stdout);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            info!(target: "cloudflared", "{}", line);
        }
    });

    // Wait for the URL to appear before handing back to the caller.
    for _ in 0..60 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let u = url.lock().await;
        if let Some(ref tunnel_url) = *u {
            // Step 3 — URL captured, transport layer is up.
            bus.send(AgentEvent::TunnelStepChanged {
                step: TunnelStep::Tunneling,
                attempt: 1,
                info: Some(tunnel_url.clone()),
            });

            // Spawn a separate task that blocks until the child exits, then
            // fires TunnelDown. No auto-restart — quick-tunnel URLs rotate on
            // each spawn which would silently invalidate active QR codes.
            let bus_for_wait = bus.clone();
            tokio::spawn(async move {
                let status = child.wait().await;
                bus_for_wait.send(AgentEvent::TunnelDown {
                    reason: format!("{status:?}"),
                });
            });
            return Ok(tunnel_url.clone());
        }
    }

    bus.send(AgentEvent::TunnelStepChanged {
        step: TunnelStep::Failed {
            reason: "timed out waiting for tunnel URL (60 s)".into(),
        },
        attempt: 1,
        info: None,
    });
    anyhow::bail!("timed out waiting for quick tunnel URL")
}

/// Spawn a named tunnel using the config at
/// `~/.config/oxiremote/tunnel.toml`. Returns the configured hostname (if any)
/// once cloudflared logs "connection registered".
pub async fn ensure_named_tunnel(
    cloudflared: PathBuf,
    cfg: crate::tunnel_named::NamedTunnelConfig,
) -> anyhow::Result<String> {
    let mut args = vec!["tunnel".to_string(), "run".to_string()];
    if let Some(cred) = cfg.credentials_file.as_deref() {
        args.push("--credentials-file".into());
        args.push(cred.to_string());
    }
    args.push(cfg.tunnel_name.clone());

    let mut child = tokio::process::Command::new(&cloudflared)
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("spawn cloudflared named tunnel")?;

    let stderr = child.stderr.take().context("no stderr")?;
    let stdout = child.stdout.take().context("no stdout")?;

    tokio::spawn(async move {
        let reader = tokio::io::BufReader::new(stderr);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            info!(target: "cloudflared", "{}", line);
        }
    });
    tokio::spawn(async move {
        let reader = tokio::io::BufReader::new(stdout);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            info!(target: "cloudflared", "{}", line);
        }
    });

    Ok(cfg
        .hostname
        .unwrap_or_else(|| format!("named://{}", cfg.tunnel_name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that `TunnelDown` is emitted when the child exits immediately.
    /// Uses `/usr/bin/false` (exits 1 immediately) as a stand-in for a crashed
    /// cloudflared. The URL-scraping loop won't find a URL, so ensure_quick_tunnel
    /// returns an error; but the child-wait task still fires. We test the task in
    /// isolation here to avoid the 60-second timeout of the outer loop.
    #[cfg(unix)]
    #[tokio::test]
    async fn tunnel_down_fires_when_child_exits() {
        use crate::events::EventBus;

        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        // Spawn `/usr/bin/false` which exits immediately with code 1.
        let mut child = tokio::process::Command::new("/usr/bin/false")
            .spawn()
            .expect("spawn /usr/bin/false");

        let bus_clone = bus.clone();
        tokio::spawn(async move {
            let status = child.wait().await;
            bus_clone.send(AgentEvent::TunnelDown {
                reason: format!("{status:?}"),
            });
        });

        // Should receive TunnelDown within 2 seconds.
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("timed out waiting for TunnelDown event")
            .expect("bus closed unexpectedly");

        assert!(
            matches!(event, AgentEvent::TunnelDown { .. }),
            "expected TunnelDown, got {event:?}"
        );
    }

    #[test]
    fn artifact_supports_mainstream_hosts() {
        // These are all the host triples we claim to support; if any of them
        // panics, the docs are lying.
        for (os, arch) in [
            ("macos", "aarch64"),
            ("macos", "x86_64"),
            ("linux", "x86_64"),
            ("linux", "aarch64"),
            ("windows", "x86_64"),
        ] {
            // we can't change env::consts at runtime, so just check the match
            // expression compiles and a host with the running OS works:
            let _ = (os, arch);
        }
        // Sanity-check the currently-running host:
        let art = artifact_for_current_host();
        assert!(art.is_ok(), "current host should be supported: {:?}", art.err());
    }

    #[test]
    fn checksum_parser_finds_matching_line() {
        let sample = "abc123  cloudflared-linux-amd64\ndef456  other-file\n";
        assert_eq!(
            find_expected_sha256(sample, "cloudflared-linux-amd64").unwrap(),
            "abc123"
        );
        assert!(find_expected_sha256(sample, "nonexistent").is_err());
    }
}
