use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use flate2::read::GzDecoder;
use futures_util::StreamExt;
use regex::Regex;
use reqwest::Client;
use sha2::{Digest, Sha256};
use tar::Archive;
use tokio::io::AsyncBufReadExt;
use tokio::sync::Notify;
use tracing::info;

use crate::events::{AgentEvent, EventBus, TunnelStep};

fn emit_prep(bus: &EventBus, info: &str) {
    bus.send(AgentEvent::TunnelStepChanged {
        step: TunnelStep::Preparing,
        attempt: 1,
        info: Some(info.to_string()),
        reason: None,
    });
}

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

pub async fn ensure_cloudflared(data_dir: &Path, bus: Arc<EventBus>) -> anyhow::Result<PathBuf> {
    emit_prep(&bus, "checking cloudflared");
    let path = cloudflared_path(data_dir);
    if path.exists() {
        emit_prep(&bus, "cloudflared cached");
        return Ok(path);
    }

    emit_prep(&bus, "finding latest release");
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

    let client = Client::builder()
        .user_agent("oxiremote/0.1")
        .timeout(Duration::from_secs(60))
        .connect_timeout(Duration::from_secs(10))
        .build()?;

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

    emit_prep(&bus, "downloading cloudflared");
    let resp = client
        .get(&asset_url)
        .send()
        .await
        .context("download cloudflared")?
        .error_for_status()
        .context("download cloudflared status")?;
    let total = resp.content_length();
    let mut stream = resp.bytes_stream();
    let mut bytes: Vec<u8> = Vec::with_capacity(total.unwrap_or(16_000_000) as usize);
    let mut last_emit = Instant::now();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("read cloudflared chunk")?;
        bytes.extend_from_slice(&chunk);
        if last_emit.elapsed() >= Duration::from_millis(200) {
            let info_text = match total {
                Some(t) if t > 0 => {
                    format!("downloading cloudflared {}%", bytes.len() as u64 * 100 / t)
                }
                _ => format!("downloading cloudflared ({} MB)", bytes.len() / 1_000_000),
            };
            emit_prep(&bus, &info_text);
            last_emit = Instant::now();
        }
    }
    // Final progress frame — the throttle can swallow the last chunk if it
    // arrived <200ms after the previous tick, leaving the UI stuck at e.g.
    // 91% before flipping to "verifying". Always emit a closure frame, even
    // when Content-Length was missing (some CDN edges/mirrors omit it).
    let final_text = match total {
        Some(_) => "downloading cloudflared 100%".into(),
        None => format!("downloaded cloudflared ({} MB)", bytes.len() / 1_000_000),
    };
    emit_prep(&bus, &final_text);

    emit_prep(&bus, "verifying");
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

    emit_prep(&bus, "ready");
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
        .timeout(Duration::from_secs(15))
        .connect_timeout(Duration::from_secs(10))
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
    shutdown: Arc<Notify>,
) -> anyhow::Result<String> {
    // Step 1 — process is about to spawn.
    bus.send(AgentEvent::TunnelStepChanged {
        step: TunnelStep::Preparing,
        attempt: 1,
        info: Some(format!("cloudflared at {}", cloudflared.display())),
        reason: None,
    });

    // Let cloudflared pick its preferred transport (defaults to QUIC with
    // automatic http2 fallback). Forcing `--protocol http2` here regressed the
    // happy path: when Cloudflare's edge intermittently rejects http2
    // registration with "context deadline exceeded", QUIC would have
    // succeeded — and there's no in-process fallback once we pin the flag.
    // kill_on_drop ensures cloudflared dies with the agent (panic, SIGINT,
    // OS shutdown). Without it, every crash leaves an orphaned cloudflared
    // reparented to init, accumulating Cloudflare quick-tunnel slot usage.
    let mut child = tokio::process::Command::new(&cloudflared)
        .args([
            "tunnel",
            "--no-autoupdate",
            "--url",
            &format!("http://{addr}"),
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("spawn cloudflared")?;

    let stderr = child.stderr.take().context("no stderr")?;
    let stdout = child.stdout.take().context("no stdout")?;

    // Step 2 — cloudflared spawned, waiting for tunnel URL on stderr.
    bus.send(AgentEvent::TunnelStepChanged {
        step: TunnelStep::Connecting,
        attempt: 1,
        info: Some(format!("waiting for tunnel URL on http://{addr}")),
        reason: None,
    });

    let url_re = Regex::new(r"https://[a-z0-9\-]+\.trycloudflare\.com").unwrap();
    // Match both modern ("Registered tunnel connection") and older
    // ("Connection registered") cloudflared log formats. Cloudflare's URL
    // banner prints BEFORE registration completes — probing immediately
    // after the URL appears can hit an edge that hasn't received the
    // tunnel-routing rule yet, returning 502/503 for the full probe window.
    // Wait for registration to ensure the tunnel is actually serving.
    let registered_re =
        Regex::new(r"(?i)(Registered tunnel connection|Connection registered)").unwrap();

    let url = Arc::new(tokio::sync::Mutex::new(None::<String>));
    let (registered_tx, registered_rx) = tokio::sync::oneshot::channel::<()>();

    // stderr carries cloudflared's INF/ERR log lines, the URL banner, and
    // the registration log. One scraper task handles all three.
    let url_for_stderr = url.clone();
    let bus_for_url = bus.clone();
    tokio::spawn(async move {
        let reader = tokio::io::BufReader::new(stderr);
        let mut lines = reader.lines();
        let mut url_announced = false;
        let mut tx = Some(registered_tx);
        while let Ok(Some(line)) = lines.next_line().await {
            info!(target: "cloudflared", "{}", line);
            if !url_announced
                && let Some(m) = url_re.find(&line)
            {
                url_announced = true;
                let captured = m.as_str().to_string();
                {
                    let mut u = url_for_stderr.lock().await;
                    *u = Some(captured.clone());
                }
                // Surface the URL on the bus so the operator sees progress
                // while we still wait for the registration confirmation.
                bus_for_url.send(AgentEvent::TunnelStepChanged {
                    step: TunnelStep::Connecting,
                    attempt: 1,
                    info: Some(format!("URL issued: {captured} (waiting for edge)")),
                    reason: None,
                });
            }
            if registered_re.is_match(&line)
                && let Some(t) = tx.take()
            {
                let _ = t.send(());
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

    // Wait up to 60s for registration. The URL banner usually arrives within
    // 1-2s of spawn; registration follows within another 1-5s on healthy
    // networks. Slow networks can push registration to ~30s.
    match tokio::time::timeout(Duration::from_secs(60), registered_rx).await {
        Ok(Ok(())) => {
            let captured = url.lock().await.clone();
            match captured {
                Some(u) => {
                    bus.send(AgentEvent::TunnelStepChanged {
                        step: TunnelStep::Tunneling,
                        attempt: 1,
                        info: Some(u.clone()),
                        reason: None,
                    });

                    // Spawn a task that races child-exit against operator
                    // disconnect. On natural exit: fire TunnelDown (no
                    // auto-restart — quick-tunnel URLs rotate). On disconnect:
                    // SIGTERM cloudflared, reap, emit TunnelDisconnected.
                    let bus_for_wait = bus.clone();
                    let shutdown_for_wait = shutdown.clone();
                    tokio::spawn(async move {
                        tokio::select! {
                            status = child.wait() => {
                                bus_for_wait.send(AgentEvent::TunnelDown {
                                    reason: format!("{status:?}"),
                                    recovery_hint: Some(
                                        "Restart the agent to spin up a fresh tunnel. \
                                         Quick-tunnel URLs rotate per-spawn; share the new one once the agent is back up."
                                            .into(),
                                    ),
                                });
                            }
                            _ = shutdown_for_wait.notified() => {
                                let _ = child.kill().await;
                                let _ = child.wait().await;
                                bus_for_wait.send(AgentEvent::TunnelDisconnected);
                            }
                        }
                    });
                    Ok(u)
                }
                None => {
                    bus.send(AgentEvent::TunnelStepChanged {
                        step: TunnelStep::Failed,
                        attempt: 1,
                        info: None,
                        reason: Some("registered without URL banner".into()),
                    });
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    anyhow::bail!("cloudflared registered without URL banner")
                }
            }
        }
        Ok(Err(_)) => {
            // The stderr task dropped its sender — cloudflared closed stderr,
            // which means the child exited (typically within seconds, not 60).
            // Surfacing "did not register within 60s" here is a lie that
            // wastes the user's time. The real error is in the agent log.
            let _ = child.wait().await;
            bus.send(AgentEvent::TunnelStepChanged {
                step: TunnelStep::Failed,
                attempt: 1,
                info: None,
                reason: Some(
                    "cloudflared exited before registering — see agent log for the cloudflared ERR line".into(),
                ),
            });
            anyhow::bail!("cloudflared exited before quick-tunnel registration");
        }
        Err(_) => {
            bus.send(AgentEvent::TunnelStepChanged {
                step: TunnelStep::Failed,
                attempt: 1,
                info: None,
                reason: Some("cloudflared did not register within 60s".into()),
            });
            // Kill + reap so the unresponsive cloudflared doesn't survive as a
            // zombie. tokio::process::Child does not reap on Drop.
            let _ = child.kill().await;
            let _ = child.wait().await;
            anyhow::bail!("timed out waiting for quick tunnel registration")
        }
    }
}

/// Spawn a named tunnel using the config at
/// `~/.config/oxiremote/tunnel.toml`. Returns the configured hostname (if any)
/// once cloudflared logs "connection registered".
pub async fn ensure_named_tunnel(
    cloudflared: PathBuf,
    cfg: crate::tunnel_named::NamedTunnelConfig,
    bus: Arc<EventBus>,
    shutdown: Arc<Notify>,
) -> anyhow::Result<Option<String>> {
    bus.send(AgentEvent::TunnelStepChanged {
        step: TunnelStep::Preparing,
        attempt: 1,
        info: Some(format!("config: {}", cfg.tunnel_name)),
        reason: None,
    });

    // Same rationale as the quick-tunnel path: let cloudflared default to
    // QUIC with built-in http2 fallback rather than pinning http2.
    let mut args: Vec<String> = vec![
        "tunnel".into(),
        "--no-autoupdate".into(),
        "run".into(),
    ];
    if let Some(cred) = cfg.credentials_file.as_deref() {
        args.push("--credentials-file".into());
        args.push(cred.to_string());
    }
    args.push(cfg.tunnel_name.clone());

    bus.send(AgentEvent::TunnelStepChanged {
        step: TunnelStep::Connecting,
        attempt: 1,
        info: Some("spawning cloudflared".into()),
        reason: None,
    });

    let mut child = match tokio::process::Command::new(&cloudflared)
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("spawn cloudflared named tunnel")
    {
        Ok(c) => c,
        Err(err) => {
            bus.send(AgentEvent::TunnelStepChanged {
                step: TunnelStep::Failed,
                attempt: 1,
                info: None,
                reason: Some(format!("{err:#}")),
            });
            return Err(err);
        }
    };

    let stderr = child.stderr.take().context("no stderr")?;
    let stdout = child.stdout.take().context("no stdout")?;

    // Match both modern ("Registered tunnel connection") and older
    // ("Connection registered") cloudflared log formats. Tightened from
    // `Connection .* registered` (overly broad) to literal "Connection
    // registered" to avoid matching unrelated lines like "Connection pool
    // registered".
    let registered_re =
        Regex::new(r"(?i)(Registered tunnel connection|Connection registered)").unwrap();
    // Block this function until the first "registered" line, then return —
    // ensures the bus event ordering is Preparing → Connecting → Tunneling
    // before main.rs synchronously emits Verifying. Without this gate, the
    // stderr-spawned task could race past `Verifying`, causing the WebUI/TUI
    // step list to rewind.
    let (registered_tx, registered_rx) = tokio::sync::oneshot::channel::<()>();
    let bus_for_stderr = bus.clone();
    tokio::spawn(async move {
        let reader = tokio::io::BufReader::new(stderr);
        let mut lines = reader.lines();
        let mut announced = false;
        let mut tx = Some(registered_tx);
        while let Ok(Some(line)) = lines.next_line().await {
            info!(target: "cloudflared", "{}", line);
            if !announced && registered_re.is_match(&line) {
                announced = true;
                bus_for_stderr.send(AgentEvent::TunnelStepChanged {
                    step: TunnelStep::Tunneling,
                    attempt: 1,
                    info: Some("1 connection registered".into()),
                    reason: None,
                });
                if let Some(t) = tx.take() {
                    let _ = t.send(());
                }
            }
        }
    });
    tokio::spawn(async move {
        let reader = tokio::io::BufReader::new(stdout);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            info!(target: "cloudflared", "{}", line);
        }
    });

    // Wait up to 60s for the first registered connection. If cloudflared
    // can't reach the edge in that window, surface Failed and bail.
    match tokio::time::timeout(Duration::from_secs(60), registered_rx).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) => {
            // Stderr task ended before signaling — child likely crashed
            // before printing its registration banner.
            bus.send(AgentEvent::TunnelStepChanged {
                step: TunnelStep::Failed,
                attempt: 1,
                info: None,
                reason: Some("cloudflared exited before registering".into()),
            });
            let _ = child.wait().await;
            anyhow::bail!("named tunnel: cloudflared exited before registering");
        }
        Err(_) => {
            bus.send(AgentEvent::TunnelStepChanged {
                step: TunnelStep::Failed,
                attempt: 1,
                info: None,
                reason: Some("named tunnel did not register within 60s".into()),
            });
            // Kill + reap so we don't leak a zombie. tokio::process::Child
            // does not reap on Drop; without an explicit wait the SIGKILL'd
            // child stays in Z state until our agent exits.
            let _ = child.kill().await;
            let _ = child.wait().await;
            anyhow::bail!("timed out waiting for named tunnel to register");
        }
    }

    // Mirror the quick-tunnel path: race child-exit against operator
    // disconnect so the same `tunnel/disconnect` endpoint works regardless
    // of tunnel mode.
    let bus_for_wait = bus.clone();
    let shutdown_for_wait = shutdown.clone();
    tokio::spawn(async move {
        tokio::select! {
            status = child.wait() => {
                bus_for_wait.send(AgentEvent::TunnelDown {
                    reason: format!("{status:?}"),
                    recovery_hint: Some(
                        "Check `cloudflared` logs and the named-tunnel credentials in \
                         ~/.config/oxiremote/tunnel.toml, then restart the agent."
                            .into(),
                    ),
                });
            }
            _ = shutdown_for_wait.notified() => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                bus_for_wait.send(AgentEvent::TunnelDisconnected);
            }
        }
    });

    // Suppress the historical `named://<tunnel_name>` placeholder when the
    // operator hasn't configured a public hostname — it surfaced as a fake
    // URL in the dashboard and nobody could open it. Returning Ok(None) lets
    // the caller render a clearer "Named tunnel active (no public hostname)"
    // state instead of dangling a non-clickable string.
    Ok(cfg.hostname)
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
                recovery_hint: None,
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

    /// Mirrors the production wait-task: select on child-exit vs shutdown
    /// notify. Asserts that notifying shutdown emits TunnelDisconnected
    /// (not TunnelDown), and that the long-running child gets reaped.
    #[cfg(unix)]
    #[tokio::test]
    async fn tunnel_disconnect_emits_disconnected_and_kills_child() {
        use crate::events::EventBus;

        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let shutdown = Arc::new(Notify::new());

        // `/bin/sleep 30` stays alive long enough that the wait branch can't
        // win the race when shutdown fires first.
        let mut child = tokio::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("spawn /bin/sleep");

        let bus_clone = bus.clone();
        let shutdown_clone = shutdown.clone();
        tokio::spawn(async move {
            tokio::select! {
                status = child.wait() => {
                    bus_clone.send(AgentEvent::TunnelDown {
                        reason: format!("{status:?}"),
                        recovery_hint: None,
                    });
                }
                _ = shutdown_clone.notified() => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    bus_clone.send(AgentEvent::TunnelDisconnected);
                }
            }
        });

        // Give the spawn a beat to start awaiting before notifying.
        tokio::time::sleep(Duration::from_millis(50)).await;
        shutdown.notify_one();

        let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("timed out waiting for TunnelDisconnected")
            .expect("bus closed unexpectedly");
        assert!(
            matches!(event, AgentEvent::TunnelDisconnected),
            "expected TunnelDisconnected, got {event:?}"
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
