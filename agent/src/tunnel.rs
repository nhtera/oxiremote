use std::collections::HashMap;
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
use tracing::{info, warn};

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
    /// Is the downloaded asset a tgz archive (macos) or a bare binary (linux/windows)?
    is_tarball: bool,
}

fn artifact_for_current_host() -> anyhow::Result<Artifact> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    match (os, arch) {
        ("macos", "aarch64") => Ok(Artifact {
            release_filename: "cloudflared-darwin-arm64.tgz".into(),
            is_tarball: true,
        }),
        ("macos", "x86_64") => Ok(Artifact {
            release_filename: "cloudflared-darwin-amd64.tgz".into(),
            is_tarball: true,
        }),
        ("linux", "x86_64") => Ok(Artifact {
            release_filename: "cloudflared-linux-amd64".into(),
            is_tarball: false,
        }),
        ("linux", "aarch64") => Ok(Artifact {
            release_filename: "cloudflared-linux-arm64".into(),
            is_tarball: false,
        }),
        ("windows", "x86_64") => Ok(Artifact {
            release_filename: "cloudflared-windows-amd64.exe".into(),
            is_tarball: false,
        }),
        _ => anyhow::bail!("unsupported cloudflared host: {os}/{arch}"),
    }
}

pub async fn ensure_cloudflared(data_dir: &Path, bus: Arc<EventBus>) -> anyhow::Result<PathBuf> {
    // Surface any failure as a Failed step event before bubbling up. Without
    // this, an HTTP/TLS error during "finding latest release" leaves the TUI
    // wedged on the Active "Preparing" row with no visible reason — the
    // underlying error only lands in the log ring.
    match try_ensure_cloudflared(data_dir, bus.clone()).await {
        Ok(p) => Ok(p),
        Err(err) => {
            bus.send(AgentEvent::TunnelStepChanged {
                step: TunnelStep::Failed,
                attempt: 1,
                info: None,
                reason: Some(format!("{err:#}")),
            });
            Err(err)
        }
    }
}

async fn try_ensure_cloudflared(data_dir: &Path, bus: Arc<EventBus>) -> anyhow::Result<PathBuf> {
    emit_prep(&bus, "checking cloudflared");
    let path = cloudflared_path(data_dir);
    if path.exists() {
        emit_prep(&bus, "cloudflared cached");
        return Ok(path);
    }

    emit_prep(&bus, "finding latest release");
    let release = fetch_cloudflared_release().await.context("get latest release metadata")?;
    let version = release.tag_name.clone();
    let art = artifact_for_current_host()?;

    // Phase 03 / C5 — Cloudflare publishes per-asset SHA256 in the release
    // notes body. Parse it before download so we fail closed on a missing /
    // unrecognised manifest format rather than installing an unverified
    // binary. Refusing here surfaces a clear error pointing operators at
    // manual install (PATH override) instead of silently regressing to
    // TLS-only trust.
    let sha_map = parse_sha_manifest(&release.body);
    if sha_map.is_empty() {
        anyhow::bail!(
            "cloudflared release {version}: no SHA256 entries parsed from release notes body. \
             Format may have changed upstream — refusing to install unverified binary. \
             Install cloudflared manually + put it on PATH, then restart oxiremote.\n\
             Release page: https://github.com/cloudflare/cloudflared/releases/tag/{version}"
        );
    }
    let expected_sha = sha_map.get(&art.release_filename).ok_or_else(|| {
        anyhow::anyhow!(
            "cloudflared release {version} manifest does not list asset `{}`",
            art.release_filename
        )
    })?;

    let asset_url = format!(
        "https://github.com/cloudflare/cloudflared/releases/download/{version}/{}",
        art.release_filename
    );

    let tmp_dir = data_dir.join("tmp");
    std::fs::create_dir_all(&tmp_dir).context("create tmp dir")?;

    let client = Client::builder()
        .user_agent("oxiremote/0.1")
        .timeout(Duration::from_secs(60))
        .connect_timeout(Duration::from_secs(10))
        .build()?;

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

    // Phase 03 / C5 — verify SHA256 against the Cloudflare release-notes
    // manifest BEFORE writing/exec-bitting the binary. The expected SHA is
    // public (it's in the release notes), so timing-safe comparison adds no
    // value here — a plain `!=` is clearer and matches `update.rs`.
    let actual_sha = hex::encode(Sha256::digest(&bytes));
    if actual_sha != *expected_sha {
        let exp_short = expected_sha.get(..16).unwrap_or(expected_sha.as_str());
        let act_short = actual_sha.get(..16).unwrap_or(actual_sha.as_str());
        warn!(expected = %exp_short, actual = %act_short, "cloudflared SHA mismatch");
        anyhow::bail!(
            "cloudflared SHA256 mismatch — refusing to install.\n  \
             expected: {expected_sha}\n  actual:   {actual_sha}"
        );
    }
    info!("cloudflared SHA256 verified");

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

/// GitHub release metadata shape we care about — tag for asset URL
/// resolution + body for SHA manifest extraction (Phase 03 / C5).
#[derive(Debug, Clone)]
struct CloudflaredRelease {
    tag_name: String,
    body: String,
}

#[derive(Debug, serde::Deserialize)]
struct GhCloudflaredReleaseRaw {
    tag_name: String,
    #[serde(default)]
    body: String,
}

async fn fetch_cloudflared_release() -> anyhow::Result<CloudflaredRelease> {
    let client = Client::builder()
        .user_agent("oxiremote/0.1")
        .timeout(Duration::from_secs(15))
        .connect_timeout(Duration::from_secs(10))
        .build()?;

    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 1..=3 {
        match fetch_cloudflared_release_once(&client).await {
            Ok(r) => return Ok(r),
            Err(err) => {
                last_err = Some(err);
                if attempt < 3 {
                    tokio::time::sleep(Duration::from_millis(800 * attempt)).await;
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("could not fetch cloudflared release metadata")))
}

async fn fetch_cloudflared_release_once(client: &Client) -> anyhow::Result<CloudflaredRelease> {
    let body = client
        .get("https://api.github.com/repos/cloudflare/cloudflared/releases/latest")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .context("fetch latest cloudflared release")?
        .error_for_status()
        .context("github API returned non-2xx for cloudflared release")?
        .text()
        .await
        .context("read cloudflared release body")?;
    let parsed: GhCloudflaredReleaseRaw =
        serde_json::from_str(&body).context("parse cloudflared release JSON")?;
    Ok(CloudflaredRelease {
        tag_name: parsed.tag_name,
        body: parsed.body,
    })
}

/// Parse Cloudflare's release-notes SHA256 manifest into a
/// `asset_name -> sha256_hex` map. Cloudflare's body uses entries shaped
/// like:
///
/// ```text
/// cloudflared-darwin-arm64.tgz: 633cee0fd41fd2020e17498beecc54811bf4fc99f891c080dc9343eb0f449c60
/// cloudflared-linux-amd64: 4a9e50e6d6d798e90fcd01933151a90bf7edd99a0a55c28ad18f2e16263a5c30
/// ```
///
/// The asset-name char class `[A-Za-z0-9._+~-]` accommodates current
/// suffixes (`.tgz`, `.exe`) and forward-compat ones (`+` / `~`). Lines
/// that don't match are silently skipped — the manifest may interleave
/// commentary with checksum rows. Empty result on parse failure forces
/// the caller to fail closed (no install).
pub fn parse_sha_manifest(body: &str) -> HashMap<String, String> {
    // Anchor at start-of-line; trailing CR is stripped via `body.lines()`.
    // Static so the pattern is compiled once across all calls.
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"^([A-Za-z0-9._+~-]+):\s*([0-9a-fA-F]{64})\s*$")
            .expect("static SHA manifest regex must compile")
    });

    let mut out = HashMap::new();
    for raw in body.lines() {
        // body.lines() handles both `\n` and `\r\n` endings (trims `\r`).
        // Use the raw line directly.
        if let Some(caps) = re.captures(raw) {
            let asset = caps.get(1).map(|m| m.as_str().to_string());
            let sha = caps.get(2).map(|m| m.as_str().to_lowercase());
            if let (Some(asset), Some(sha)) = (asset, sha) {
                out.insert(asset, sha);
            }
        }
    }
    out
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

    // Windows: tie the child to a kill-on-job-close job object so cloudflared
    // dies with the agent even when Drop never runs (taskkill /f, panic, OS
    // reboot, kernel-level termination). kill_on_drop only fires on graceful
    // unwind. Best-effort — failure here means we lose the auto-cleanup
    // guarantee for THIS spawn but the rest of the supervision still works.
    #[cfg(target_os = "windows")]
    if let Some(pid) = child.id()
        && let Err(err) = crate::win_jobs::add_to_kill_on_exit_job(pid)
    {
        tracing::warn!(error = %err, pid, "could not assign cloudflared to kill-on-exit job");
    }

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
    // the registration log. One scraper task handles all three. We track the
    // last announced URL across the whole process lifetime, not just the
    // first banner — Cloudflare's edge can invalidate a Quick Tunnel session
    // (network blip, edge migration) and cloudflared will reconnect with a
    // *new* URL while still alive. Without continuous capture the agent's
    // stored URL goes stale and every cross-origin pairing 404s with NXDOMAIN
    // until the operator manually restarts.
    let url_for_stderr = url.clone();
    let bus_for_url = bus.clone();
    tokio::spawn(async move {
        let reader = tokio::io::BufReader::new(stderr);
        process_cloudflared_stderr(
            reader,
            url_for_stderr,
            bus_for_url,
            url_re,
            registered_re,
            registered_tx,
        )
        .await;
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

    // Same kill-on-exit job assignment as the quick-tunnel path — see
    // ensure_quick_tunnel for rationale.
    #[cfg(target_os = "windows")]
    if let Some(pid) = child.id()
        && let Err(err) = crate::win_jobs::add_to_kill_on_exit_job(pid)
    {
        tracing::warn!(error = %err, pid, "could not assign cloudflared to kill-on-exit job");
    }

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

/// Outcome of inspecting a cloudflared stderr line for a Quick Tunnel URL
/// banner. Pure value type so the de-dupe state machine can be unit-tested
/// without spinning up a real cloudflared process.
#[derive(Debug, PartialEq, Eq)]
enum UrlChange {
    /// First URL ever seen on this stream — caller surfaces a progressive
    /// UI signal but does NOT emit `TunnelUrlChanged`. The lifecycle
    /// emission for the first URL is owned by `main.rs`, which fires it
    /// after `ensure_quick_tunnel` returns (i.e. after registration
    /// completes). Splitting first vs rotated keeps the bus event ordering
    /// clean: Connecting → Tunneling → first TunnelUrlChanged.
    First(String),
    /// URL differs from the last one seen — cloudflared rotated the public
    /// URL mid-process. Caller MUST emit `TunnelUrlChanged` so
    /// `discovery::spawn_register` re-registers session / temp-key / code
    /// with the worker, plus a `TunnelStepChanged` for the dashboard.
    Rotated(String),
    /// No URL on this line, or a duplicate of the last URL we saw.
    /// (cloudflared can reprint the URL banner on idle reconnect attempts
    /// that resolve to the same URL — those are benign.)
    None,
}

/// Pure URL-change detector. Mutates `last_url` only when a NEW, DIFFERENT
/// URL is observed; ignores duplicates and lines without a URL match.
fn detect_url_change(line: &str, last_url: &mut Option<String>, url_re: &Regex) -> UrlChange {
    let Some(m) = url_re.find(line) else {
        return UrlChange::None;
    };
    let captured = m.as_str().to_string();
    if last_url.as_deref() == Some(captured.as_str()) {
        return UrlChange::None;
    }
    let was_first = last_url.is_none();
    *last_url = Some(captured.clone());
    if was_first {
        UrlChange::First(captured)
    } else {
        UrlChange::Rotated(captured)
    }
}

/// Drive the cloudflared stderr stream end-to-end: forward every line into
/// tracing for the agent log, classify it via `detect_url_change`, emit the
/// right bus events, and fire the registration oneshot once. Extracted from
/// the `tokio::spawn` body so the whole loop is testable with an in-memory
/// `AsyncBufRead` (e.g. `tokio::io::DuplexStream`).
async fn process_cloudflared_stderr<R: tokio::io::AsyncBufRead + Unpin>(
    reader: R,
    url_slot: Arc<tokio::sync::Mutex<Option<String>>>,
    bus: Arc<EventBus>,
    url_re: Regex,
    registered_re: Regex,
    registered_tx: tokio::sync::oneshot::Sender<()>,
) {
    let mut lines = reader.lines();
    let mut last_url: Option<String> = None;
    let mut tx = Some(registered_tx);
    while let Ok(Some(line)) = lines.next_line().await {
        info!(target: "cloudflared", "{}", line);
        match detect_url_change(&line, &mut last_url, &url_re) {
            UrlChange::First(captured) => {
                {
                    let mut u = url_slot.lock().await;
                    *u = Some(captured.clone());
                }
                bus.send(AgentEvent::TunnelStepChanged {
                    step: TunnelStep::Connecting,
                    attempt: 1,
                    info: Some(format!("URL issued: {captured} (waiting for edge)")),
                    reason: None,
                });
            }
            UrlChange::Rotated(captured) => {
                {
                    let mut u = url_slot.lock().await;
                    *u = Some(captured.clone());
                }
                warn!(new_url = %captured, "cloudflared rotated tunnel URL — re-registering");
                bus.send(AgentEvent::TunnelUrlChanged {
                    url: captured.clone(),
                });
                bus.send(AgentEvent::TunnelStepChanged {
                    step: TunnelStep::Tunneling,
                    attempt: 1,
                    info: Some(captured.clone()),
                    reason: Some("URL rotated — re-registered with discovery worker".into()),
                });
            }
            UrlChange::None => {}
        }
        if registered_re.is_match(&line)
            && let Some(t) = tx.take()
        {
            let _ = t.send(());
        }
    }
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

    // Phase 03 / C5 — SHA manifest parsing must tolerate the formatting
    // variations Cloudflare's release notes have shipped historically and
    // any plausible forward-compat suffixes (per Red Team F11).

    const SAMPLE_LF: &str = "Some intro line\n\n\
        cloudflared-darwin-arm64.tgz: 633cee0fd41fd2020e17498beecc54811bf4fc99f891c080dc9343eb0f449c60\n\
        cloudflared-linux-amd64: 4a9e50e6d6d798e90fcd01933151a90bf7edd99a0a55c28ad18f2e16263a5c30\n";

    #[test]
    fn parse_sha_manifest_handles_lf() {
        let map = parse_sha_manifest(SAMPLE_LF);
        assert_eq!(
            map.get("cloudflared-darwin-arm64.tgz").map(String::as_str),
            Some("633cee0fd41fd2020e17498beecc54811bf4fc99f891c080dc9343eb0f449c60")
        );
        assert_eq!(
            map.get("cloudflared-linux-amd64").map(String::as_str),
            Some("4a9e50e6d6d798e90fcd01933151a90bf7edd99a0a55c28ad18f2e16263a5c30")
        );
    }

    #[test]
    fn parse_sha_manifest_handles_crlf() {
        let crlf = SAMPLE_LF.replace('\n', "\r\n");
        let map = parse_sha_manifest(&crlf);
        assert_eq!(map.len(), 2);
        assert!(map.contains_key("cloudflared-linux-amd64"));
    }

    #[test]
    fn parse_sha_manifest_handles_mixed_line_endings() {
        let mixed = SAMPLE_LF.replacen('\n', "\r\n", 2);
        let map = parse_sha_manifest(&mixed);
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn parse_sha_manifest_accepts_plus_and_tilde_in_asset_names() {
        let body = "cloudflared-linux-amd64+fips: 1111111111111111111111111111111111111111111111111111111111111111\n\
                    cloudflared-linux-amd64~beta: 2222222222222222222222222222222222222222222222222222222222222222\n";
        let map = parse_sha_manifest(body);
        assert_eq!(map.len(), 2);
        assert!(map.contains_key("cloudflared-linux-amd64+fips"));
        assert!(map.contains_key("cloudflared-linux-amd64~beta"));
    }

    #[test]
    fn parse_sha_manifest_returns_empty_when_no_lines_match() {
        let body = "## Release notes\n\nNo SHA table this time around.\n\nGoodbye.\n";
        let map = parse_sha_manifest(body);
        assert!(map.is_empty(), "parser must NOT invent entries: {map:?}");
    }

    #[test]
    fn parse_sha_manifest_skips_malformed_lines() {
        let body = "cloudflared-linux-amd64: notenoughhex\n\
                    cloudflared-darwin-arm64.tgz: 633cee0fd41fd2020e17498beecc54811bf4fc99f891c080dc9343eb0f449c60\n\
                    cloudflared-windows-amd64.exe: 5151515151515151515151515151515151515151515151515151515151515151extra\n";
        let map = parse_sha_manifest(body);
        // Only the well-formed line is kept.
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("cloudflared-darwin-arm64.tgz"));
    }

    #[test]
    fn parse_sha_manifest_accepts_uppercase_hex() {
        let body = "cloudflared-linux-amd64: ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789\n";
        let map = parse_sha_manifest(body);
        // Stored as lowercase for stable comparison.
        assert_eq!(
            map.get("cloudflared-linux-amd64").map(String::as_str),
            Some("abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789")
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

    // ---------------------------------------------------------------------
    // URL rotation detection — covers the bug where cloudflared rotates the
    // public Quick Tunnel URL mid-process and the agent silently goes stale.
    // The state machine is the entire fix; these tests pin its semantics so
    // a future refactor can't regress it.
    // ---------------------------------------------------------------------

    fn url_re() -> Regex {
        Regex::new(r"https://[a-z0-9\-]+\.trycloudflare\.com").unwrap()
    }

    fn registered_re() -> Regex {
        Regex::new(r"(?i)(Registered tunnel connection|Connection registered)").unwrap()
    }

    #[test]
    fn detect_url_change_first_url_returns_first_and_sets_state() {
        let re = url_re();
        let mut last: Option<String> = None;
        let line = "2026-05-08T11:00:00Z INF | https://alpha.trycloudflare.com";
        let out = detect_url_change(line, &mut last, &re);
        assert_eq!(out, UrlChange::First("https://alpha.trycloudflare.com".into()));
        assert_eq!(last.as_deref(), Some("https://alpha.trycloudflare.com"));
    }

    #[test]
    fn detect_url_change_returns_none_for_lines_without_url() {
        let re = url_re();
        let mut last: Option<String> = None;
        for line in &[
            "INF | Starting cloudflared",
            "ERR | something went wrong",
            "",
            "Some random log line",
            "https://not-trycloudflare.com is not a match",
            "http://lowercase-http.trycloudflare.com is wrong scheme",
        ] {
            assert_eq!(detect_url_change(line, &mut last, &re), UrlChange::None);
        }
        assert_eq!(last, None, "non-matching lines must NOT mutate state");
    }

    #[test]
    fn detect_url_change_same_url_repeated_returns_none() {
        let re = url_re();
        let mut last: Option<String> = None;
        let url = "https://alpha.trycloudflare.com";
        // First sighting → First.
        assert_eq!(
            detect_url_change(url, &mut last, &re),
            UrlChange::First(url.into())
        );
        // Repeats → None (de-duped). cloudflared can reprint the banner on
        // benign reconnects to the same URL — must not spam the bus.
        for _ in 0..5 {
            assert_eq!(detect_url_change(url, &mut last, &re), UrlChange::None);
            assert_eq!(last.as_deref(), Some(url), "state must remain stable");
        }
    }

    #[test]
    fn detect_url_change_different_url_returns_rotated() {
        let re = url_re();
        let mut last: Option<String> = None;
        assert_eq!(
            detect_url_change("https://alpha.trycloudflare.com", &mut last, &re),
            UrlChange::First("https://alpha.trycloudflare.com".into())
        );
        assert_eq!(
            detect_url_change("https://bravo.trycloudflare.com", &mut last, &re),
            UrlChange::Rotated("https://bravo.trycloudflare.com".into())
        );
        assert_eq!(last.as_deref(), Some("https://bravo.trycloudflare.com"));
    }

    #[test]
    fn detect_url_change_three_sequential_rotations() {
        let re = url_re();
        let mut last: Option<String> = None;
        let urls = [
            "https://aaa.trycloudflare.com",
            "https://bbb.trycloudflare.com",
            "https://ccc.trycloudflare.com",
            "https://ddd.trycloudflare.com",
        ];
        let outs: Vec<UrlChange> = urls
            .iter()
            .map(|u| detect_url_change(u, &mut last, &re))
            .collect();
        assert_eq!(outs[0], UrlChange::First(urls[0].into()));
        for (i, expected) in urls.iter().enumerate().skip(1) {
            assert_eq!(outs[i], UrlChange::Rotated((*expected).into()));
        }
        assert_eq!(last.as_deref(), Some(urls[3]));
    }

    #[test]
    fn detect_url_change_rotation_back_to_previous_still_emits_rotated() {
        // A → B → A is a real rotation (the agent's stored URL was B, B is
        // now invalid, the new URL is A). We must NOT short-circuit here:
        // the worker still needs the new value posted.
        let re = url_re();
        let mut last: Option<String> = None;
        let a = "https://alpha.trycloudflare.com";
        let b = "https://bravo.trycloudflare.com";
        assert_eq!(
            detect_url_change(a, &mut last, &re),
            UrlChange::First(a.into())
        );
        assert_eq!(
            detect_url_change(b, &mut last, &re),
            UrlChange::Rotated(b.into())
        );
        assert_eq!(
            detect_url_change(a, &mut last, &re),
            UrlChange::Rotated(a.into()),
            "A → B → A: third call must still rotate (not de-dupe against the original first URL)"
        );
        assert_eq!(last.as_deref(), Some(a));
    }

    #[test]
    fn detect_url_change_extracts_url_from_log_prefix() {
        let re = url_re();
        let mut last: Option<String> = None;
        // Real cloudflared output wraps the URL in INF lines + box drawing.
        let line = "2026-05-08T11:00:00Z INF |  https://reliability-wyoming-oil-tex.trycloudflare.com                          |";
        let out = detect_url_change(line, &mut last, &re);
        assert_eq!(
            out,
            UrlChange::First("https://reliability-wyoming-oil-tex.trycloudflare.com".into())
        );
    }

    #[test]
    fn detect_url_change_only_extracts_first_url_when_multiple_on_line() {
        let re = url_re();
        let mut last: Option<String> = None;
        let line = "switching from https://old.trycloudflare.com to https://new.trycloudflare.com";
        let out = detect_url_change(line, &mut last, &re);
        assert_eq!(
            out,
            UrlChange::First("https://old.trycloudflare.com".into()),
            "regex.find() returns the leftmost match — caller depends on this"
        );
    }

    #[test]
    fn detect_url_change_uppercase_host_does_not_match() {
        // cloudflared always emits lowercase. A future format change would
        // surface as a `None` here; better to require an explicit code update
        // than silently accept an unexpected shape.
        let re = url_re();
        let mut last: Option<String> = None;
        let out = detect_url_change("https://ALPHA.trycloudflare.com", &mut last, &re);
        assert_eq!(out, UrlChange::None);
        assert_eq!(last, None);
    }

    #[test]
    fn detect_url_change_strips_trailing_path_query_punctuation() {
        // The URL regex is bounded by host suffix — anything trailing (path,
        // query, comma, paren) is excluded. Ensures the captured value is a
        // clean URL ready for `tunnelUrl` registration with the worker.
        let re = url_re();
        let cases = [
            ("(https://alpha.trycloudflare.com)", "https://alpha.trycloudflare.com"),
            ("https://alpha.trycloudflare.com/health", "https://alpha.trycloudflare.com"),
            ("https://alpha.trycloudflare.com?foo=bar", "https://alpha.trycloudflare.com"),
            ("https://alpha.trycloudflare.com,more text", "https://alpha.trycloudflare.com"),
        ];
        for (input, expected) in cases {
            let mut last: Option<String> = None;
            let out = detect_url_change(input, &mut last, &re);
            assert_eq!(out, UrlChange::First(expected.into()), "input: {input}");
        }
    }

    // ---------------------------------------------------------------------
    // End-to-end stderr task — drives `process_cloudflared_stderr` with an
    // in-memory pipe so we can verify event-bus sequencing exactly as the
    // production task would behave with a real cloudflared.
    // ---------------------------------------------------------------------

    use crate::events::EventBus;
    use tokio::io::AsyncWriteExt;

    /// Spawn the production stderr loop against a pipe. Writes `lines` then
    /// closes the writer to terminate the loop. Drains the bus into a vec.
    /// Returns (events, registered_received_within_timeout).
    async fn run_stderr_loop(lines: &[&str]) -> (Vec<AgentEvent>, bool) {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let url_slot = Arc::new(tokio::sync::Mutex::new(None::<String>));
        let (reg_tx, reg_rx) = tokio::sync::oneshot::channel::<()>();

        let (mut writer, reader) = tokio::io::duplex(64 * 1024);
        let buf_reader = tokio::io::BufReader::new(reader);

        // EventBus::new() already returns Arc<EventBus>; clone the Arc for
        // the spawned task so both this scope and the task observe the same
        // broadcast channel.
        let bus_for_task = bus.clone();
        let slot_for_task = url_slot.clone();
        let task = tokio::spawn(async move {
            process_cloudflared_stderr(
                buf_reader,
                slot_for_task,
                bus_for_task,
                url_re(),
                registered_re(),
                reg_tx,
            )
            .await;
        });

        for l in lines {
            writer.write_all(l.as_bytes()).await.unwrap();
            writer.write_all(b"\n").await.unwrap();
        }
        // Closing the write half EOFs the reader and ends the loop.
        drop(writer);

        // Wait for the loop to finish (bounded — guards against a regression
        // that would block forever).
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("stderr loop did not terminate within 2s")
            .unwrap();

        let registered =
            tokio::time::timeout(Duration::from_millis(50), reg_rx).await.is_ok_and(|r| r.is_ok());

        let mut events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        (events, registered)
    }

    fn count_url_changed(events: &[AgentEvent]) -> usize {
        events
            .iter()
            .filter(|e| matches!(e, AgentEvent::TunnelUrlChanged { .. }))
            .count()
    }

    fn count_step_changed(events: &[AgentEvent], wanted_step: TunnelStep) -> usize {
        events
            .iter()
            .filter(|e| matches!(e, AgentEvent::TunnelStepChanged { step, .. } if *step == wanted_step))
            .count()
    }

    #[tokio::test]
    async fn stderr_loop_first_url_does_not_emit_tunnel_url_changed() {
        // First URL belongs to main.rs (it owns the lifecycle emission after
        // ensure_quick_tunnel returns). The stderr task only emits a
        // progressive Connecting step.
        let (events, _) = run_stderr_loop(&[
            "INF | Starting tunnel",
            "INF | https://alpha.trycloudflare.com",
        ])
        .await;
        assert_eq!(
            count_url_changed(&events),
            0,
            "first URL must not emit TunnelUrlChanged from the stderr loop"
        );
        assert_eq!(
            count_step_changed(&events, TunnelStep::Connecting),
            1,
            "first URL must emit one Connecting step"
        );
    }

    #[tokio::test]
    async fn stderr_loop_url_rotation_emits_tunnel_url_changed_and_tunneling_step() {
        let (events, _) = run_stderr_loop(&[
            "INF | https://alpha.trycloudflare.com",
            "ERR | edge dropped",
            "INF | https://bravo.trycloudflare.com",
        ])
        .await;
        assert_eq!(
            count_url_changed(&events),
            1,
            "exactly one TunnelUrlChanged for the single rotation"
        );
        assert_eq!(
            count_step_changed(&events, TunnelStep::Tunneling),
            1,
            "rotation emits Tunneling step (with rotation reason)"
        );
        // Sanity: the carried URL is the new one.
        let url_evt = events
            .iter()
            .find_map(|e| match e {
                AgentEvent::TunnelUrlChanged { url } => Some(url.clone()),
                _ => None,
            })
            .expect("TunnelUrlChanged event present");
        assert_eq!(url_evt, "https://bravo.trycloudflare.com");
    }

    #[tokio::test]
    async fn stderr_loop_repeated_same_url_dedupes() {
        let same = "INF | https://alpha.trycloudflare.com";
        let (events, _) = run_stderr_loop(&[same, same, same, same]).await;
        assert_eq!(
            count_url_changed(&events),
            0,
            "no rotation: must not emit TunnelUrlChanged"
        );
        assert_eq!(
            count_step_changed(&events, TunnelStep::Connecting),
            1,
            "first URL emits Connecting once; duplicates de-duped"
        );
    }

    #[tokio::test]
    async fn stderr_loop_two_rotations_emit_two_url_changed() {
        let (events, _) = run_stderr_loop(&[
            "INF | https://alpha.trycloudflare.com",
            "INF | https://bravo.trycloudflare.com",
            "INF | https://charlie.trycloudflare.com",
        ])
        .await;
        assert_eq!(
            count_url_changed(&events),
            2,
            "alpha→bravo and bravo→charlie are both rotations"
        );
    }

    #[tokio::test]
    async fn stderr_loop_registered_oneshot_fires_once() {
        let (_, registered) = run_stderr_loop(&[
            "INF | https://alpha.trycloudflare.com",
            "INF | Registered tunnel connection 1",
            "INF | Registered tunnel connection 2", // second match must not re-fire
            "INF | Registered tunnel connection 3",
        ])
        .await;
        assert!(registered, "registered_tx must fire on first match");
    }

    #[tokio::test]
    async fn stderr_loop_ignores_non_url_lines_for_dedupe() {
        // Mix random log lines around URL banners — must not interfere with
        // dedupe state.
        let (events, _) = run_stderr_loop(&[
            "INF | Starting cloudflared",
            "INF | https://alpha.trycloudflare.com",
            "WRN | network blip",
            "INF | https://alpha.trycloudflare.com",
            "INF | something else",
            "INF | https://bravo.trycloudflare.com",
            "INF | unrelated noise",
        ])
        .await;
        assert_eq!(count_url_changed(&events), 1, "alpha→bravo = 1 rotation");
    }

    #[tokio::test]
    async fn stderr_loop_url_slot_holds_latest_url() {
        let bus = EventBus::new();
        let url_slot = Arc::new(tokio::sync::Mutex::new(None::<String>));
        let (reg_tx, _reg_rx) = tokio::sync::oneshot::channel::<()>();

        let (mut writer, reader) = tokio::io::duplex(64 * 1024);
        let buf_reader = tokio::io::BufReader::new(reader);
        let bus_for_task = bus.clone();
        let slot_for_task = url_slot.clone();

        let task = tokio::spawn(async move {
            process_cloudflared_stderr(
                buf_reader,
                slot_for_task,
                bus_for_task,
                url_re(),
                registered_re(),
                reg_tx,
            )
            .await;
        });

        for l in [
            "INF | https://alpha.trycloudflare.com",
            "INF | https://bravo.trycloudflare.com",
            "INF | https://charlie.trycloudflare.com",
        ] {
            writer.write_all(l.as_bytes()).await.unwrap();
            writer.write_all(b"\n").await.unwrap();
        }
        drop(writer);
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();

        let final_url = url_slot.lock().await.clone();
        assert_eq!(
            final_url.as_deref(),
            Some("https://charlie.trycloudflare.com"),
            "url_slot must always hold the most recent URL — the heartbeat reads this"
        );
    }

    #[tokio::test]
    async fn stderr_loop_terminates_cleanly_on_eof() {
        // Empty input — no URL, no registration. Loop must terminate via EOF
        // and not hang. (Regression guard: a future refactor that turns the
        // loop into `loop {}` instead of `while Ok(Some(line))` would deadlock
        // here.)
        let (events, registered) = run_stderr_loop(&[]).await;
        assert!(events.is_empty());
        assert!(!registered);
    }
}
