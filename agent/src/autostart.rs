// Autostart management — detect and toggle whether the agent runs at login.
//
// Three OS branches:
//   macOS   — LaunchAgent plist at ~/Library/LaunchAgents/
//   Linux   — systemd user unit via `systemctl --user`
//   Windows — registry HKCU\Software\Microsoft\Windows\CurrentVersion\Run via `reg` CLI
//
// Returns `supported: false` on unknown platforms or when the required
// tooling is absent, so the caller can render a "not supported" notice
// rather than a broken toggle.

use std::path::PathBuf;

#[derive(serde::Serialize, Debug)]
pub struct AutostartStatus {
    pub enabled: bool,
    pub supported: bool,
    pub mechanism: Option<String>,
}

impl AutostartStatus {
    fn unsupported() -> Self {
        Self { enabled: false, supported: false, mechanism: None }
    }
}

/// Detect current autostart state for the running OS.
pub fn detect() -> anyhow::Result<AutostartStatus> {
    #[cfg(target_os = "macos")]
    return detect_macos();

    #[cfg(target_os = "linux")]
    return detect_linux();

    #[cfg(target_os = "windows")]
    return detect_windows();

    #[allow(unreachable_code)]
    Ok(AutostartStatus::unsupported())
}

/// Enable or disable autostart for the running OS.
pub fn set_enabled(enabled: bool) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    return set_macos(enabled);

    #[cfg(target_os = "linux")]
    return set_linux(enabled);

    #[cfg(target_os = "windows")]
    return set_windows(enabled);

    #[allow(unreachable_code)]
    {
        let _ = enabled;
        anyhow::bail!("autostart not supported on this platform")
    }
}

// --- macOS ---

#[cfg(target_os = "macos")]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(target_os = "macos")]
fn plist_path() -> anyhow::Result<PathBuf> {
    let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("HOME not set"))?;
    Ok(PathBuf::from(home)
        .join("Library/LaunchAgents/com.oxiremote.agent.plist"))
}

#[cfg(target_os = "macos")]
fn detect_macos() -> anyhow::Result<AutostartStatus> {
    let path = plist_path()?;
    Ok(AutostartStatus {
        enabled: path.exists(),
        supported: true,
        mechanism: Some("launchd".to_string()),
    })
}

#[cfg(target_os = "macos")]
fn set_macos(enabled: bool) -> anyhow::Result<()> {
    let path = plist_path()?;
    if enabled {
        // Get the path to the running binary.
        let bin = std::env::current_exe()
            .unwrap_or_else(|_| PathBuf::from("oxiremote"));
        let bin_xml = xml_escape(&bin.display().to_string());
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.oxiremote.agent</string>
    <key>ProgramArguments</key>
    <array>
        <string>{bin_xml}</string>
        <string>serve</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
    <key>StandardOutPath</key>
    <string>/tmp/oxiremote.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/oxiremote.log</string>
</dict>
</plist>
"#
        );
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, plist)?;
    } else {
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(())
}

// --- Linux ---

#[cfg(target_os = "linux")]
fn detect_linux() -> anyhow::Result<AutostartStatus> {
    // Check if systemctl is available.
    let which = std::process::Command::new("which")
        .arg("systemctl")
        .output();
    if which.map(|o| !o.status.success()).unwrap_or(true) {
        return Ok(AutostartStatus::unsupported());
    }

    let out = std::process::Command::new("systemctl")
        .args(["--user", "is-enabled", "oxiremote.service"])
        .output();

    let enabled = match out {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            stdout.trim() == "enabled"
        }
        Err(_) => false,
    };

    Ok(AutostartStatus {
        enabled,
        supported: true,
        mechanism: Some("systemd user unit".to_string()),
    })
}

#[cfg(target_os = "linux")]
fn set_linux(enabled: bool) -> anyhow::Result<()> {
    // Write or remove the unit file, then enable/disable it.
    let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("HOME not set"))?;
    let unit_dir = PathBuf::from(&home).join(".config/systemd/user");
    let unit_path = unit_dir.join("oxiremote.service");

    if enabled {
        let bin = std::env::current_exe()
            .unwrap_or_else(|_| PathBuf::from("oxiremote"));
        let unit = format!(
            "[Unit]\nDescription=OxiRemote Agent\nAfter=network.target\n\n[Service]\nExecStart=\"{}\" serve\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n",
            bin.display()
        );
        std::fs::create_dir_all(&unit_dir)?;
        std::fs::write(&unit_path, unit)?;

        let out = std::process::Command::new("systemctl")
            .args(["--user", "enable", "oxiremote.service"])
            .output()
            .map_err(|e| anyhow::anyhow!("systemctl enable failed: {e}"))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!("systemctl enable failed: {stderr}");
        }
    } else {
        let out = std::process::Command::new("systemctl")
            .args(["--user", "disable", "oxiremote.service"])
            .output();
        // Ignore error if service was never enabled.
        let _ = out;
        if unit_path.exists() {
            let _ = std::fs::remove_file(&unit_path);
        }
    }
    Ok(())
}

// --- Windows ---

#[cfg(target_os = "windows")]
const REG_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(target_os = "windows")]
const REG_VALUE: &str = "OxiRemote";

#[cfg(target_os = "windows")]
fn detect_windows() -> anyhow::Result<AutostartStatus> {
    let out = std::process::Command::new("reg")
        .args(["query", REG_KEY, "/v", REG_VALUE])
        .output();

    let enabled = match out {
        Ok(o) => o.status.success(),
        Err(_) => {
            return Ok(AutostartStatus::unsupported());
        }
    };

    Ok(AutostartStatus {
        enabled,
        supported: true,
        mechanism: Some("Windows Startup registry".to_string()),
    })
}

#[cfg(target_os = "windows")]
fn set_windows(enabled: bool) -> anyhow::Result<()> {
    if enabled {
        let bin = std::env::current_exe()
            .unwrap_or_else(|_| PathBuf::from("oxiremote.exe"));
        let value = format!("\"{}\" serve", bin.display());
        let out = std::process::Command::new("reg")
            .args(["add", REG_KEY, "/v", REG_VALUE, "/t", "REG_SZ", "/d", &value, "/f"])
            .output()
            .map_err(|e| anyhow::anyhow!("reg add failed: {e}"))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!("reg add failed: {stderr}");
        }
    } else {
        let out = std::process::Command::new("reg")
            .args(["delete", REG_KEY, "/v", REG_VALUE, "/f"])
            .output();
        // Ignore error if value didn't exist.
        let _ = out;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_does_not_panic() {
        // Smoke test: detect() must return without panicking on the current platform.
        let result = detect();
        // On CI (Linux without systemd) it may return unsupported — that is OK.
        // We only require no panic and a valid AutostartStatus.
        match result {
            Ok(s) => {
                // If unsupported, mechanism must be None.
                if !s.supported {
                    assert!(s.mechanism.is_none());
                    assert!(!s.enabled);
                }
            }
            Err(_) => {
                // An error is acceptable only on platforms where detection itself fails.
            }
        }
    }
}
