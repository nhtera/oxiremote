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

fn cloudflared_path(data_dir: &Path) -> PathBuf {
    data_dir.join("cloudflared")
}

pub async fn ensure_cloudflared(data_dir: &Path) -> anyhow::Result<PathBuf> {
    let path = cloudflared_path(data_dir);
    if path.exists() {
        return Ok(path);
    }

    let version = cloudflared_latest_version().await.context("get latest version")?;
    let arch = std::env::consts::ARCH;
    let os = std::env::consts::OS;
    if os != "macos" {
        anyhow::bail!("auto-download only implemented for macos (got {os})");
    }

    let arch_segment = match arch {
        "aarch64" => "darwin-arm64",
        "x86_64" => "darwin-amd64",
        _ => anyhow::bail!("unsupported arch for cloudflared: {arch}"),
    };

    let tgz_url = format!(
        "https://github.com/cloudflare/cloudflared/releases/download/{version}/cloudflared-{arch_segment}.tgz"
    );
    let checksums_url = format!(
        "https://github.com/cloudflare/cloudflared/releases/download/{version}/cloudflared-{version}-checksums.txt"
    );

    let tmp_dir = data_dir.join("tmp");
    std::fs::create_dir_all(&tmp_dir).context("create tmp dir")?;
    let tgz_path = tmp_dir.join("cloudflared.tgz");

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
        .get(&tgz_url)
        .send()
        .await
        .context("download cloudflared")?
        .error_for_status()
        .context("download cloudflared status")?
        .bytes()
        .await
        .context("read cloudflared bytes")?;

    let expected = find_expected_sha256(&checksums_text, &format!("cloudflared-{arch_segment}.tgz"))
        .context("find expected sha256")?;
    let actual = hex::encode(Sha256::digest(&bytes));
    if actual != expected {
        anyhow::bail!("cloudflared sha256 mismatch");
    }

    std::fs::write(&tgz_path, &bytes).context("write cloudflared archive")?;

    let extracted = extract_cloudflared_tgz(&bytes, &tmp_dir).context("extract cloudflared")?;
    std::fs::rename(&extracted, &path).context("move cloudflared into place")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms)?;
    }

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
) -> anyhow::Result<String> {
    let mut child = tokio::process::Command::new(&cloudflared)
        .args(["tunnel", "--url", &format!("http://{addr}")])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("spawn cloudflared")?;

    let stderr = child.stderr.take().context("no stderr")?;
    let reader = tokio::io::BufReader::new(stderr);
    let mut lines = reader.lines();

    let re = Regex::new(r"https://[a-z0-9\-]+\.trycloudflare\.com").unwrap();

    let url = Arc::new(tokio::sync::Mutex::new(None::<String>));
    let url2 = url.clone();

    tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            info!(target: "cloudflared", "{}", line);
            if let Some(m) = re.find(&line) {
                let mut u = url2.lock().await;
                if u.is_none() {
                    *u = Some(m.as_str().to_string());
                }
            }
        }
    });

    for _ in 0..60 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let u = url.lock().await;
        if let Some(ref tunnel_url) = *u {
            return Ok(tunnel_url.clone());
        }
    }

    anyhow::bail!("timed out waiting for quick tunnel URL")
}
