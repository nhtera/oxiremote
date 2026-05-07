// Localhost-scoped operational hints surfaced as dashboard banners.
//
// Hints are derived on every request rather than persisted: the source of
// truth is the underlying state (workspaces table, file perms, …). This
// closes the broadcast-race fix from Red Team F14 — a dashboard opened
// after the boot-time `SettingsHint` broadcast still sees the hint via the
// pull endpoint.
//
// SPA model:
//   - GET /api/agent/hints on dashboard mount → render banners
//   - subscribe to AgentEvent::SettingsHint via SSE → re-fetch on any event
//   - localStorage `oxi:hint:<code>:dismissed` per code per device

use std::path::Path;

use rusqlite::Connection;
use serde::Serialize;

use crate::events::HintSeverity;

#[derive(Debug, Clone, Serialize)]
pub struct Hint {
    pub code: String,
    pub severity: HintSeverity,
    pub message: String,
}

/// Stable code for the legacy-install root-workspace banner. Surfaces when
/// a `/` (or `C:\`) workspace row is present on the current host —
/// approved tunnel devices would otherwise gain whole-disk access via
/// `/api/files`. Operator must remove it manually under Settings →
/// Workspaces.
pub const CODE_ROOT_WORKSPACE_PRESENT: &str = "root-workspace-present";

/// Stable code for "could not enforce 0o600 / owner-only ACL on the HMAC
/// signing key". Emitted from `auth::load_or_create_key`'s self-heal path
/// when chmod / ACL apply fails. Severity is `Error` — full-forgery risk
/// if the key is exfiltrated.
pub const CODE_SIGNING_KEY_PERMS_FAILED: &str = "signing-key-perms-failed";

/// Stable code for the one-shot first-boot banner that surfaces the freshly-
/// generated discovery worker secret. The message body carries the full
/// secret on the SSE event (one-shot, only at generation time); the pull
/// endpoint surfaces a non-secret reminder while the secret file exists.
/// Per-device localStorage dismissal handles "I've already configured this".
pub const CODE_DISCOVERY_SECRET_GENERATED: &str = "discovery-secret-generated";

/// Stable code for "discovery secret could not be loaded / generated".
/// Severity is `Error` — cross-origin pairing silently stops working until
/// the operator unbreaks the secret file (e.g. recovers from backup or
/// deletes it to trigger regeneration). M3 from Phase 04 review.
pub const CODE_DISCOVERY_SECRET_FAILED: &str = "discovery-secret-failed";

/// Compute the currently-active hint set for `host_id`. Cheap; runs on
/// every `/api/agent/hints` request.
///
/// `data_dir` is used to derive filesystem-shaped state (e.g. presence of
/// the discovery secret file). Pass `None` to skip those probes.
pub fn active_hints(
    db_path: &Path,
    host_id: &str,
    data_dir: Option<&Path>,
) -> rusqlite::Result<Vec<Hint>> {
    let conn = Connection::open(db_path)?;
    let mut out = Vec::new();
    if crate::workspaces::has_root_workspace(&conn, host_id)? {
        out.push(Hint {
            code: CODE_ROOT_WORKSPACE_PRESENT.into(),
            severity: HintSeverity::Warn,
            message: "A root-filesystem workspace is configured. Devices with file \
                      access can read your entire disk. Review or delete it under \
                      Settings → Workspaces."
                .into(),
        });
    }
    // Discovery secret reminder — surfaces while the file exists. Operator
    // dismisses per-device once they've configured the worker. Plaintext is
    // NOT in this message (only in the boot-time SSE event); recover via
    // `oxiremote config discovery-secret`.
    //
    // Discovery URL set but file unreadable: emit the Error-severity hint so
    // late-joining dashboards see the failure even after the boot-time SSE
    // event fired. The plaintext error reason isn't surfaced here (logs are
    // the place for that); the message just points at the recovery path.
    if let Some(dir) = data_dir {
        let p = dir.join(crate::discovery::DISCOVERY_SECRET_FILENAME);
        if p.exists() {
            // Defensive read: if the file exists but is empty / unreadable,
            // surface the failure hint instead of the success one.
            match std::fs::read_to_string(&p) {
                Ok(s) if !s.trim().is_empty() => {
                    out.push(Hint {
                        code: CODE_DISCOVERY_SECRET_GENERATED.into(),
                        severity: HintSeverity::Info,
                        message: "Discovery worker secret is provisioned on this host. \
                                  Configure the worker via `wrangler secret put \
                                  AGENT_REGISTRATION_SECRET` (recover the value with \
                                  `oxiremote config discovery-secret`)."
                            .into(),
                    });
                }
                _ => {
                    out.push(Hint {
                        code: CODE_DISCOVERY_SECRET_FAILED.into(),
                        severity: HintSeverity::Error,
                        message: "Discovery secret file is unreadable or empty. \
                                  Inspect ~/.oxiremote/discovery_secret or remove it \
                                  to regenerate."
                            .into(),
                    });
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn tempdir(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "oxi-hints-{}-{}-{name}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// Build the minimal schema `active_hints` reads. Only the workspaces
    /// table is touched here; we don't seed any rows, so root-workspace hint
    /// stays absent and only the discovery-secret branches are exercised.
    fn fresh_db(dir: &std::path::Path) -> std::path::PathBuf {
        let p = dir.join("test.sqlite");
        let conn = Connection::open(&p).unwrap();
        conn.execute_batch(
            "CREATE TABLE workspaces (host_id TEXT, path TEXT, name TEXT, created_at INTEGER);",
        )
        .unwrap();
        p
    }

    #[test]
    fn no_data_dir_means_no_discovery_hint() {
        let dir = tempdir("nodata");
        let db = fresh_db(&dir);
        let hints = active_hints(&db, "host", None).unwrap();
        assert!(
            !hints
                .iter()
                .any(|h| h.code == CODE_DISCOVERY_SECRET_GENERATED)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_secret_file_means_no_discovery_hint() {
        let dir = tempdir("missing");
        let db = fresh_db(&dir);
        let hints = active_hints(&db, "host", Some(&dir)).unwrap();
        assert!(hints.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn populated_secret_file_emits_info_hint() {
        let dir = tempdir("ok");
        let db = fresh_db(&dir);
        std::fs::write(
            dir.join(crate::discovery::DISCOVERY_SECRET_FILENAME),
            "a".repeat(64),
        )
        .unwrap();
        let hints = active_hints(&db, "host", Some(&dir)).unwrap();
        let h = hints
            .iter()
            .find(|h| h.code == CODE_DISCOVERY_SECRET_GENERATED)
            .expect("expected discovery-secret-generated hint");
        assert!(matches!(h.severity, HintSeverity::Info));
        assert!(!h.message.contains("aaa"), "must NOT include the secret value");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_secret_file_emits_error_hint() {
        let dir = tempdir("empty");
        let db = fresh_db(&dir);
        std::fs::write(dir.join(crate::discovery::DISCOVERY_SECRET_FILENAME), "").unwrap();
        let hints = active_hints(&db, "host", Some(&dir)).unwrap();
        let h = hints
            .iter()
            .find(|h| h.code == CODE_DISCOVERY_SECRET_FAILED)
            .expect("expected discovery-secret-failed hint");
        assert!(matches!(h.severity, HintSeverity::Error));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
