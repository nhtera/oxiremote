//! Session-scoped stay-awake guard for macOS hosts.
//!
//! Holds a `caffeinate -d -i -s -u` child for the lifetime of the guard so the
//! display, system, and user-idle timers all stay reset. Also zeros
//! `com.apple.screensaver idleTime` and restores it on Drop so the screen-saver
//! does not auto-trigger mid-session and does not stay disabled after the
//! session ends.
//!
//! Non-macOS targets compile this module to a zero-cost no-op `StayAwakeGuard`
//! so callers don't need their own `#[cfg]` shim.

#[cfg(target_os = "macos")]
mod imp {
    use std::process::{Child, Command, Stdio};

    use anyhow::{Context, Result};
    use tracing::{info, warn};

    const SCREENSAVER_DOMAIN: &str = "com.apple.screensaver";
    const IDLE_TIME_KEY: &str = "idleTime";

    pub struct StayAwakeGuard {
        caffeinate: Option<Child>,
        prior_idle_time: Option<i64>,
    }

    impl StayAwakeGuard {
        /// Acquire system-wake assertions and suppress screensaver auto-trigger.
        ///
        /// The caffeinate spawn is the only step that's allowed to fail — without
        /// it the guard is meaningless. The defaults read/write are best-effort:
        /// a missing screensaver domain (Sequoia-deprecation safety) degrades
        /// silently to "caffeinate only" rather than failing session bring-up.
        pub fn acquire() -> Result<Self> {
            let caffeinate = Command::new("caffeinate")
                .args(["-d", "-i", "-s", "-u"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .context("spawn caffeinate")?;

            let prior_idle_time = read_idle_time().ok();
            if prior_idle_time.is_some()
                && let Err(e) = write_idle_time(0)
            {
                warn!(error = %e, "stay-awake: could not zero screensaver idleTime");
            }

            info!(
                caffeinate_pid = caffeinate.id(),
                prior_idle_time = ?prior_idle_time,
                "stay-awake guard acquired"
            );
            Ok(Self {
                caffeinate: Some(caffeinate),
                prior_idle_time,
            })
        }
    }

    impl Drop for StayAwakeGuard {
        fn drop(&mut self) {
            if let Some(mut child) = self.caffeinate.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            // Skip the restore when we never had a value to restore — writing
            // back an unknown default would do more harm than good.
            if let Some(prior) = self.prior_idle_time
                && let Err(e) = write_idle_time(prior)
            {
                warn!(error = %e, "stay-awake: could not restore screensaver idleTime");
            }
            info!("stay-awake guard released");
        }
    }

    fn read_idle_time() -> Result<i64> {
        let out = Command::new("defaults")
            .args(["-currentHost", "read", SCREENSAVER_DOMAIN, IDLE_TIME_KEY])
            .output()
            .context("defaults read idleTime")?;
        if !out.status.success() {
            anyhow::bail!("defaults read non-zero exit");
        }
        let s = String::from_utf8_lossy(&out.stdout);
        s.trim().parse::<i64>().context("parse idleTime")
    }

    fn write_idle_time(v: i64) -> Result<()> {
        let s = v.to_string();
        let out = Command::new("defaults")
            .args([
                "-currentHost",
                "write",
                SCREENSAVER_DOMAIN,
                IDLE_TIME_KEY,
                "-int",
                &s,
            ])
            .output()
            .context("defaults write idleTime")?;
        if !out.status.success() {
            anyhow::bail!("defaults write non-zero exit");
        }
        Ok(())
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use anyhow::Result;

    /// Non-macOS no-op so callers can construct + drop without `#[cfg]` themselves.
    pub struct StayAwakeGuard;

    impl StayAwakeGuard {
        pub fn acquire() -> Result<Self> {
            Ok(Self)
        }
    }
}

pub use imp::StayAwakeGuard;

#[cfg(test)]
mod tests {
    use super::StayAwakeGuard;

    /// Sanity test: instantiate and drop without panicking. On non-macOS this
    /// is a no-op; on macOS we shell out to caffeinate + defaults — which is
    /// fine in dev but flaky in container CI. Keep it ignored so `cargo test`
    /// stays green there; run manually with `--ignored` on a real Mac.
    #[test]
    #[cfg_attr(target_os = "macos", ignore = "shells out to caffeinate; manual only")]
    fn acquire_and_drop() {
        let g = StayAwakeGuard::acquire().expect("acquire");
        drop(g);
    }
}
