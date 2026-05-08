// Self-update signature verification (minisign / Ed25519).
//
// Wire-format expectations:
//   - Per-asset signature: standard minisign 4-line `.minisig` block.
//   - Bundled manifest: `oxiremote-<version>-minisig.txt` — concatenated
//     `.minisig` blocks separated by blank lines. The asset name is the
//     `file:<name>` field embedded in each block's `trusted comment:` line.
//
// Trusted-key model (Red Team F13):
//   `TRUSTED_PUBKEYS` is a CONST ARRAY, not a single key. A signature is
//   accepted if ANY key in the array verifies it. Rotation procedure
//   documented in `docs/release-signing.md`.
//
// Soft-launch contract:
//   Signing is enabled in CI by setting the `MINISIGN_SECRET_KEY` +
//   `MINISIGN_PASSPHRASE` repo secrets AND replacing the placeholder in
//   `TRUSTED_PUBKEYS` below. v0.1.25 was the unsigned transition release
//   (the workflow detects the missing secret and skips signing). From
//   v0.1.26 onward the workflow publishes signed manifests and `verify`
//   on the agent side runtime-rejects anything else (Red Team F1). The
//   v0.1.24 → v0.1.25 hop uses the old SHA-only `update.rs`; the v0.1.25
//   → v0.1.26 hop uses the new strict path against v0.1.26's signatures.

use std::collections::HashMap;

use anyhow::{Context, Result, anyhow, bail};
use minisign_verify::{PublicKey, Signature};

/// Trusted minisign public keys. A signed manifest is accepted if **any**
/// key in this array verifies its signature — this is what enables key
/// rotation without stranding existing users:
///
/// 1. Generate K2 offline.
/// 2. Ship release N+1 with `[K1, K2]` embedded — signed by K1 (so the
///    N+0 users with `[K1]` accept it).
/// 3. Once N+1 is widely adopted, ship release N+2 signed by K2 (still
///    embedding `[K1, K2]` so the rotation window stays open).
/// 4. Drop K1 only when N+1 is no longer expected to receive updates.
///
/// **Pre-v0.1.26 action:** generate the production keypair offline
/// (`minisign -G`), replace the placeholder below with the base64 line
/// from `minisign.pub` (third line, no `RWQ`/`RWR`-only header), and
/// stash the secret key in the GitHub repo secrets
/// (`MINISIGN_SECRET_KEY` + `MINISIGN_PASSPHRASE`). The release workflow
/// auto-detects the secret and only then runs the signing pipeline +
/// pubkey-placeholder gate. See `docs/release-signing.md` for custodian
/// + Shamir-2-of-3 backup requirements.
pub const TRUSTED_PUBKEYS: &[&str] = &[
    // K1 — placeholder until the operator generates the production key.
    // The release workflow gates the placeholder check on the
    // `MINISIGN_SECRET_KEY` secret being present, so this placeholder is
    // tolerated for the v0.1.25 transition release. Once the operator
    // provisions the secret, the gate refuses to publish until this line
    // is replaced with the real base64 pubkey from `minisign.pub`
    // (third line, no `RWQ`/`RWR`-only header).
    "REPLACE_WITH_PRODUCTION_PUBKEY_BEFORE_RELEASE",
];

/// Verify `archive_bytes` against `sig_block`. Tries every key in
/// `trusted` until one accepts; returns the first error otherwise.
///
/// `sig_block` is the full 4-line minisign signature text (untrusted +
/// signature + trusted + global signature). Pre-hashed signatures are
/// allowed to keep large-archive verifiers happy.
pub fn verify_with_keys(
    archive_bytes: &[u8],
    sig_block: &str,
    trusted: &[&str],
) -> Result<()> {
    let signature = Signature::decode(sig_block).context("decode minisign signature block")?;

    let mut last_err: Option<minisign_verify::Error> = None;
    for pk_b64 in trusted {
        // Skip the placeholder so a forgotten pre-merge step is loud:
        // verification fails closed instead of silently succeeding.
        if *pk_b64 == "REPLACE_WITH_PRODUCTION_PUBKEY_BEFORE_RELEASE" {
            continue;
        }
        let pk = match PublicKey::from_base64(pk_b64) {
            Ok(pk) => pk,
            Err(err) => {
                last_err = Some(err);
                continue;
            }
        };
        match pk.verify(archive_bytes, &signature, /* allow_legacy = */ true) {
            Ok(()) => return Ok(()),
            Err(err) => last_err = Some(err),
        }
    }
    match last_err {
        Some(err) => Err(anyhow!("no trusted key accepted the signature: {err}")),
        None => bail!(
            "no usable trusted minisign keys configured \
             (TRUSTED_PUBKEYS contains only the placeholder — signing not provisioned)"
        ),
    }
}

/// Convenience wrapper using the embedded `TRUSTED_PUBKEYS` array.
pub fn verify(archive_bytes: &[u8], sig_block: &str) -> Result<()> {
    verify_with_keys(archive_bytes, sig_block, TRUSTED_PUBKEYS)
}

/// Parse a concatenated manifest of `.minisig` blocks into a
/// `asset_name -> signature_block` map. Blocks are separated by one or
/// more blank lines; the asset name is the `file:<name>` field of each
/// block's `trusted comment:` line.
///
/// Robust to single-block oddities (Red Team R2/R3): a block missing the
/// `file:` field is **logged and skipped**, NOT propagated as a parse
/// failure — otherwise one legacy/unknown block in a multi-target
/// manifest would brick `oxiremote update` for every user. A caller that
/// can't find its asset gets a focused "not in manifest" error from the
/// lookup site (`update.rs`) instead of a generic parse failure.
pub fn parse_minisig_manifest(text: &str) -> Result<HashMap<String, String>> {
    let mut out: HashMap<String, String> = HashMap::new();

    let flush = |current: &mut Vec<String>, out: &mut HashMap<String, String>| {
        if current.is_empty() {
            return;
        }
        let block_text = current.join("\n");
        if let Some(asset) = extract_filename(&block_text) {
            out.insert(asset, block_text);
        } else {
            tracing::warn!(
                "minisig block missing `file:<name>` in trusted comment — skipping"
            );
        }
        current.clear();
    };

    let mut current: Vec<String> = Vec::with_capacity(4);
    for raw in text.lines() {
        // `lines()` already trims trailing `\r`; use full `trim_end()` to also
        // drop trailing spaces/tabs that would corrupt the embedded base64
        // signature (Red Team R2).
        let line = raw.trim_end();
        if line.is_empty() {
            flush(&mut current, &mut out);
        } else {
            current.push(line.to_string());
        }
    }
    flush(&mut current, &mut out);

    if out.is_empty() {
        bail!("no recognisable minisig blocks parsed (manifest empty or malformed)");
    }
    Ok(out)
}

/// Extract the `file:<name>` value from the `trusted comment:` line of a
/// minisign signature block. minisign always emits the prefix in lowercase
/// (`trusted comment:`); we don't accept other casings to keep parsing
/// unambiguous. The trusted-comment payload is a tab- or space-separated
/// list of `key:value` fields.
fn extract_filename(block: &str) -> Option<String> {
    let payload = block
        .lines()
        .find_map(|l| l.trim_start().strip_prefix("trusted comment:"))?;
    for field in payload.split(['\t', ' ']) {
        if let Some(name) = field.strip_prefix("file:") {
            let name = name.trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real golden vector from the `minisign-verify` crate README — proves
    // we wired the API correctly. Pubkey + signature both verify against
    // the literal four bytes `b"test"`.
    const GOLDEN_PUBKEY: &str = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
    const GOLDEN_SIG: &str = "untrusted comment: signature from minisign secret key
RWQf6LRCGA9i59SLOFxz6NxvASXDJeRtuZykwQepbDEGt87ig1BNpWaVWuNrm73YiIiJbq71Wi+dP9eKL8OC351vwIasSSbXxwA=
trusted comment: timestamp:1555779966\tfile:test
QtKMXWyYcwdpZAlPF7tE2ENJkRd1ujvKjlj1m9RtHTBnZPa5WKU5uWRs5GoP5M/VqE81QFuMKI5k/SfNQUaOAA==";

    #[test]
    fn golden_vector_verifies_against_known_pubkey() {
        verify_with_keys(b"test", GOLDEN_SIG, &[GOLDEN_PUBKEY])
            .expect("golden vector must verify");
    }

    #[test]
    fn tampered_data_fails_verification() {
        let err =
            verify_with_keys(b"tampered", GOLDEN_SIG, &[GOLDEN_PUBKEY]).expect_err("must fail");
        let s = format!("{err}");
        assert!(s.contains("no trusted key"), "{s}");
    }

    #[test]
    fn untrusted_key_does_not_accept_valid_sig() {
        // Different valid pubkey shape but wrong key — must reject.
        // A second-key array `[bogus, golden]` should still succeed
        // because verification is "any key in the trusted list".
        let bogus = "RWS+abcdEFGHijklMNOPqrstUVWXyz0123456789012345678901234567";
        // Wrong-only → fail.
        verify_with_keys(b"test", GOLDEN_SIG, &[bogus]).expect_err("wrong key alone must fail");
        // Wrong + right → succeed (rotation semantics).
        verify_with_keys(b"test", GOLDEN_SIG, &[bogus, GOLDEN_PUBKEY])
            .expect("either-of-array must accept golden");
    }

    #[test]
    fn placeholder_pubkey_is_skipped() {
        // The default TRUSTED_PUBKEYS in this branch is the placeholder.
        // Calling `verify` with valid input must still fail closed because
        // no real key is provisioned.
        let err = verify(b"test", GOLDEN_SIG).expect_err("placeholder must fail closed");
        let s = format!("{err}");
        assert!(s.contains("placeholder") || s.contains("not provisioned") || s.contains("no usable"), "{s}");
    }

    #[test]
    fn parse_manifest_extracts_per_file_blocks() {
        let manifest = format!("{GOLDEN_SIG}\n\n{GOLDEN_SIG}\n");
        let map = parse_minisig_manifest(&manifest).expect("parse");
        // Both blocks have file:test in the trusted comment; the second
        // overwrites the first since they share an asset name. We only
        // assert one entry resolves correctly — the duplicate-name case
        // is operator error, not a parser concern.
        assert_eq!(map.len(), 1);
        let block = map.get("test").expect("file:test entry");
        // Round-trip: parsed block re-verifies.
        verify_with_keys(b"test", block, &[GOLDEN_PUBKEY]).expect("parsed block must verify");
    }

    #[test]
    fn parse_manifest_handles_crlf_line_endings() {
        let manifest = GOLDEN_SIG.replace('\n', "\r\n");
        let map = parse_minisig_manifest(&manifest).expect("parse CRLF");
        assert!(map.contains_key("test"));
        let block = map.get("test").unwrap();
        verify_with_keys(b"test", block, &[GOLDEN_PUBKEY])
            .expect("CRLF-stripped block must verify");
    }

    #[test]
    fn parse_manifest_skips_block_missing_filename_but_keeps_others() {
        // Red Team R3 — one bad block must NOT poison the whole manifest.
        // Bad block: trusted comment without `file:<name>`.
        // Good block: GOLDEN_SIG with file:test.
        let bad = "untrusted comment: signature from minisign secret key
RWQf6LRCGA9i59SLOFxz6NxvASXDJeRtuZykwQepbDEGt87ig1BNpWaVWuNrm73YiIiJbq71Wi+dP9eKL8OC351vwIasSSbXxwA=
trusted comment: timestamp:1555779966
QtKMXWyYcwdpZAlPF7tE2ENJkRd1ujvKjlj1m9RtHTBnZPa5WKU5uWRs5GoP5M/VqE81QFuMKI5k/SfNQUaOAA==";
        let combined = format!("{bad}\n\n{GOLDEN_SIG}\n");
        let map = parse_minisig_manifest(&combined).expect("must keep good block");
        assert_eq!(map.len(), 1, "bad block silently skipped, good block retained");
        assert!(map.contains_key("test"));
    }

    #[test]
    fn parse_manifest_fails_when_all_blocks_invalid() {
        // If literally nothing parses, that's a hard failure — caller would
        // otherwise hit a misleading "not in manifest" error per asset.
        let only_bad = "## Release notes\n\nNothing minisign-shaped here.\n";
        let err = parse_minisig_manifest(only_bad).expect_err("empty result must error");
        assert!(format!("{err}").contains("no recognisable") || format!("{err}").contains("empty"));
    }

    #[test]
    fn parse_manifest_strips_trailing_whitespace() {
        // Red Team R2 — a CRLF block whose payload lines have trailing spaces
        // would otherwise inject those spaces into the base64, breaking
        // Signature::decode. trim_end() catches both `\r` and ` `.
        let with_trailing = GOLDEN_SIG
            .lines()
            .map(|l| format!("{l}  "))
            .collect::<Vec<_>>()
            .join("\n");
        let map = parse_minisig_manifest(&with_trailing).expect("trim_end should rescue");
        let block = map.get("test").expect("file:test entry");
        verify_with_keys(b"test", block, &[GOLDEN_PUBKEY])
            .expect("trimmed block must verify");
    }

    #[test]
    fn extract_filename_works_with_tab_or_space_separators() {
        let with_tab = "trusted comment: timestamp:1\tfile:asset.tar.gz";
        let with_space = "trusted comment: timestamp:1 file:asset.tar.gz";
        // Build minimal blocks; only the trusted comment matters.
        let block_tab = format!("untrusted comment: x\nABC=\n{with_tab}\nABC==");
        let block_space = format!("untrusted comment: x\nABC=\n{with_space}\nABC==");
        assert_eq!(extract_filename(&block_tab).as_deref(), Some("asset.tar.gz"));
        assert_eq!(extract_filename(&block_space).as_deref(), Some("asset.tar.gz"));
    }
}
