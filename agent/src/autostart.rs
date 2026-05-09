// Autostart management — detect and toggle whether the agent runs at login.
//
// Three OS branches:
//   macOS   — LaunchAgent plist at ~/Library/LaunchAgents/
//   Linux   — systemd user unit via `systemctl --user`
//   Windows — Task Scheduler XML (primary); HKCU Run registry (fallback)
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
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
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

/// Run `launchctl bootstrap gui/$UID <plist>` to load the service.
/// Ignores "already loaded" errors (exit code 37).
#[cfg(target_os = "macos")]
fn launchctl_bootstrap(plist_path: &std::path::Path) -> anyhow::Result<()> {
    let uid = unsafe { libc::getuid() };
    let domain = format!("gui/{uid}");
    let out = std::process::Command::new("launchctl")
        .args(["bootstrap", &domain, &plist_path.display().to_string()])
        .output()
        .map_err(|e| anyhow::anyhow!("launchctl bootstrap failed to spawn: {e}"))?;
    // Exit code 37 = "already bootstrapped" — idempotent, treat as success.
    if !out.status.success() {
        let code = out.status.code().unwrap_or(-1);
        if code != 37 {
            let stderr = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!("launchctl bootstrap failed (code {code}): {stderr}");
        }
    }
    Ok(())
}

/// Run `launchctl bootout gui/$UID com.oxiremote.agent` to unload the service.
/// Called by `set_enabled(false)` BEFORE removing the plist file.
/// Ignores "not found" errors — service may not be loaded.
#[cfg(target_os = "macos")]
pub fn launchctl_unload_if_active() {
    let uid = unsafe { libc::getuid() };
    let domain = format!("gui/{uid}");
    let _ = std::process::Command::new("launchctl")
        .args(["bootout", &domain, "com.oxiremote.agent"])
        .output();
}

#[cfg(target_os = "macos")]
fn set_macos(enabled: bool) -> anyhow::Result<()> {
    let path = plist_path()?;
    if enabled {
        let bin = std::env::current_exe()
            .unwrap_or_else(|_| PathBuf::from("oxiremote"));
        let bin_xml = xml_escape(&bin.display().to_string());
        // KeepAlive dict with Crashed=true: launchd respawns only on abnormal exit.
        // NetworkState=true is intentionally omitted — it triggers on VPN/WiFi
        // handoffs causing dozens of spurious respawns on mobile laptops.
        // ThrottleInterval=10 caps rapid crash-loop frequency.
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
    <dict>
        <key>Crashed</key>
        <true/>
    </dict>
    <key>ProcessType</key>
    <string>Background</string>
    <key>ThrottleInterval</key>
    <integer>10</integer>
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
        // Load immediately so the user doesn't have to log out/in.
        launchctl_bootstrap(&path)?;
    } else {
        // Bootout before removing plist — launchd needs the label resolved first.
        launchctl_unload_if_active();
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(())
}

// --- Linux ---

#[cfg(target_os = "linux")]
fn detect_linux() -> anyhow::Result<AutostartStatus> {
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
    let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("HOME not set"))?;
    let unit_dir = PathBuf::from(&home).join(".config/systemd/user");
    let unit_path = unit_dir.join("oxiremote.service");

    if enabled {
        let bin = std::env::current_exe()
            .unwrap_or_else(|_| PathBuf::from("oxiremote"));
        // StartLimitIntervalSec + StartLimitBurst cap rapid crash loops to 5
        // restarts per 5 minutes before systemd gives up.
        let unit = format!(
            "[Unit]\nDescription=OxiRemote Agent\nAfter=network.target\n\n\
             [Service]\nExecStart=\"{}\" serve\nRestart=on-failure\nRestartSec=5s\n\
             StartLimitIntervalSec=300\nStartLimitBurst=5\n\n\
             [Install]\nWantedBy=default.target\n",
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
const TASK_NAME: &str = "OxiRemote";

/// Escape a string for safe inclusion in an XML text node or attribute value.
/// Required before embedding file-system paths in Task Scheduler XML — paths
/// such as `C:\Dev & Tools\<bad>\oxiremote.exe` contain characters that break
/// XML structure without escaping (red-team Finding 5).
#[cfg(target_os = "windows")]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Build Task Scheduler XML for the OxiRemote service.
///
/// Triggers: LogonTrigger (run at user login), EventTrigger (Kernel-Power 107,
/// resume from sleep), SessionStateChangeTrigger (SessionUnlock, laptop lid open).
/// Settings: RestartOnFailure 3×60s, no battery restriction, hidden, single instance.
#[cfg(target_os = "windows")]
fn build_task_xml(exe_path: &str, user_name: &str) -> String {
    let cmd = xml_escape(exe_path);
    let user = xml_escape(user_name);
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.4" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>OxiRemote background agent — self-hosted remote access</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
    <EventTrigger>
      <Enabled>true</Enabled>
      <Subscription>&lt;QueryList&gt;&lt;Query Id="0" Path="System"&gt;&lt;Select Path="System"&gt;*[System[Provider[@Name='Microsoft-Windows-Kernel-Power'] and EventID=107]]&lt;/Select&gt;&lt;/Query&gt;&lt;/QueryList&gt;</Subscription>
    </EventTrigger>
    <SessionStateChangeTrigger>
      <Enabled>true</Enabled>
      <StateChange>SessionUnlock</StateChange>
      <UserId>{user}</UserId>
    </SessionStateChangeTrigger>
  </Triggers>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <AllowStartIfOnBatteries>true</AllowStartIfOnBatteries>
    <Hidden>true</Hidden>
    <RestartOnFailure>
      <Interval>PT1M</Interval>
      <Count>3</Count>
    </RestartOnFailure>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{cmd}</Command>
      <Arguments>serve</Arguments>
    </Exec>
  </Actions>
</Task>
"#
    )
}

/// Resolve the current Windows username via the USERNAME env var.
/// Falls back to "SYSTEM" which schtasks accepts for machine-scoped tasks,
/// though in practice USERNAME is always set in an interactive session.
#[cfg(target_os = "windows")]
fn current_username() -> String {
    std::env::var("USERNAME").unwrap_or_else(|_| "SYSTEM".to_string())
}

#[cfg(target_os = "windows")]
fn detect_windows() -> anyhow::Result<AutostartStatus> {
    // Check Task Scheduler first (preferred mechanism).
    let scht = std::process::Command::new("schtasks")
        .args(["/Query", "/TN", TASK_NAME, "/FO", "LIST"])
        .stderr(std::process::Stdio::null())
        .output();

    if let Ok(o) = scht {
        if o.status.success() {
            return Ok(AutostartStatus {
                enabled: true,
                supported: true,
                mechanism: Some("Task Scheduler".to_string()),
            });
        }
    }

    // Fall back to registry check.
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
        mechanism: Some(if enabled {
            "HKCU Run".to_string()
        } else {
            "Windows Startup registry".to_string()
        }),
    })
}

#[cfg(target_os = "windows")]
fn set_windows(enabled: bool) -> anyhow::Result<()> {
    if enabled {
        let bin = std::env::current_exe()
            .unwrap_or_else(|_| PathBuf::from("oxiremote.exe"));
        let exe_path = bin.display().to_string();
        let user = current_username();
        let xml = build_task_xml(&exe_path, &user);

        // Write XML to a temp file for schtasks to consume.
        // schtasks /XML requires UTF-16 LE with BOM (0xFF 0xFE prefix).
        let tmp_path = std::env::temp_dir().join("oxiremote_task.xml");
        let mut utf16_bytes: Vec<u8> = vec![0xFF, 0xFE]; // UTF-16 LE BOM
        utf16_bytes.extend(xml.encode_utf16().flat_map(|c| c.to_le_bytes()));
        std::fs::write(&tmp_path, utf16_bytes)
            .map_err(|e| anyhow::anyhow!("write task XML to temp: {e}"))?;

        let scht_out = std::process::Command::new("schtasks")
            .args([
                "/Create",
                "/TN", TASK_NAME,
                "/XML", &tmp_path.display().to_string(),
                "/F",
            ])
            .output();
        let _ = std::fs::remove_file(&tmp_path);

        match scht_out {
            Ok(o) if o.status.success() => {
                // Task Scheduler registration succeeded.
                return Ok(());
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                tracing::warn!("schtasks create failed ({}): {} — falling back to HKCU Run", o.status, stderr.trim());
            }
            Err(e) => {
                tracing::warn!("schtasks not available: {e} — falling back to HKCU Run");
            }
        }

        // Fallback: HKCU Run registry entry (no restart-on-crash, but always available).
        let value = format!("\"{}\" serve", exe_path);
        let out = std::process::Command::new("reg")
            .args(["add", REG_KEY, "/v", REG_VALUE, "/t", "REG_SZ", "/d", &value, "/f"])
            .output()
            .map_err(|e| anyhow::anyhow!("reg add failed: {e}"))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!("reg add failed: {stderr}");
        }
    } else {
        // Attempt Task Scheduler removal first.
        let _ = std::process::Command::new("schtasks")
            .args(["/Delete", "/TN", TASK_NAME, "/F"])
            .output();
        // Also clean up any HKCU Run fallback entry.
        let _ = std::process::Command::new("reg")
            .args(["delete", REG_KEY, "/v", REG_VALUE, "/f"])
            .output();
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

    /// Verify that special XML characters in exe_path are properly escaped and
    /// that the generated XML parses without error. Also asserts that all three
    /// required triggers appear in the output.
    #[cfg(target_os = "windows")]
    #[test]
    fn task_xml_escapes_special_chars_and_contains_required_triggers() {
        let exe_path = r"C:\Dev & Tools\<bad>\oxiremote.exe";
        let xml = build_task_xml(exe_path, "testuser");

        // Must parse cleanly.
        let mut reader = quick_xml::Reader::from_str(&xml);
        reader.config_mut().trim_text(true);
        let mut buf = Vec::new();
        let mut command_value = String::new();
        let mut in_command = false;
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(quick_xml::events::Event::Start(e)) => {
                    if e.name().as_ref() == b"Command" {
                        in_command = true;
                    }
                }
                Ok(quick_xml::events::Event::Text(e)) => {
                    if in_command {
                        command_value = e.unescape().expect("unescape Command text").to_string();
                        in_command = false;
                    }
                }
                Ok(quick_xml::events::Event::Eof) => break,
                Err(e) => panic!("XML parse error at position {}: {e:?}", reader.buffer_position()),
                _ => {}
            }
            buf.clear();
        }

        assert_eq!(
            command_value, exe_path,
            "parsed <Command> must equal original path after XML round-trip"
        );

        // All three required triggers must appear in the raw XML.
        assert!(xml.contains("LogonTrigger"), "LogonTrigger missing");
        assert!(xml.contains("EventTrigger"), "EventTrigger missing");
        assert!(xml.contains("SessionStateChangeTrigger"), "SessionStateChangeTrigger missing");
    }

    /// On non-Windows, verify xml_escape handles the special characters correctly.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_xml_escape_handles_special_chars() {
        let input = r#"C:/path & <data> "quoted" 'apos'"#;
        let escaped = xml_escape(input);
        assert!(escaped.contains("&amp;"));
        assert!(escaped.contains("&lt;"));
        assert!(escaped.contains("&gt;"));
        assert!(escaped.contains("&quot;"));
        assert!(escaped.contains("&apos;"));
        assert!(!escaped.contains('&') || escaped.contains("&amp;") || escaped.contains("&lt;") || escaped.contains("&gt;") || escaped.contains("&quot;") || escaped.contains("&apos;"));
    }
}
