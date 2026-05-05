#!/usr/bin/env bash
# Drift guard: fails CI / pre-tag if version strings disagree across
#   - agent/Cargo.toml                 (Rust binary)
#   - agent/crates/desktop/Cargo.toml  (desktop crate, must match)
#   - npm-wrapper/package.json         (npm package + oxiremoteBinaryVersion)
#   - apps/landing/src/version.ts      (landing-page banner)
#
# Run before tagging a release: `bash scripts/release/check-versions.sh`

set -euo pipefail

cd "$(dirname "$0")/../.."

fail() {
  echo "version drift: $*" >&2
  exit 1
}

extract_toml_version() {
  # Reads the FIRST `version = "..."` after `[package]` in the given Cargo.toml.
  awk '
    /^\[package\]/ { in_pkg = 1; next }
    /^\[/ && !/^\[package\]/ { in_pkg = 0 }
    in_pkg && /^version[[:space:]]*=/ {
      match($0, /"[^"]+"/)
      print substr($0, RSTART + 1, RLENGTH - 2)
      exit
    }
  ' "$1"
}

agent_version="$(extract_toml_version agent/Cargo.toml)"
desktop_version="$(extract_toml_version agent/crates/desktop/Cargo.toml)"
npm_version="$(node -p "require('./npm-wrapper/package.json').version")"
npm_bin_version="$(node -p "require('./npm-wrapper/package.json').oxiremoteBinaryVersion || ''")"
landing_version="$(grep -E "^export const VERSION[[:space:]]" apps/landing/src/version.ts | sed -E "s/.*'v?([^']+)'.*/\1/")"

echo "agent/Cargo.toml                  = ${agent_version}"
echo "agent/crates/desktop/Cargo.toml   = ${desktop_version}"
echo "npm-wrapper/package.json version  = ${npm_version}"
echo "  oxiremoteBinaryVersion          = ${npm_bin_version:-<unset>}"
echo "apps/landing/src/version.ts       = ${landing_version}"

[ -n "$agent_version" ] || fail "agent/Cargo.toml has no version"
[ "$agent_version" = "$desktop_version" ] \
  || fail "agent ($agent_version) ≠ desktop crate ($desktop_version)"
[ "$agent_version" = "$npm_version" ] \
  || fail "agent ($agent_version) ≠ npm-wrapper ($npm_version)"
if [ -n "$npm_bin_version" ]; then
  [ "$agent_version" = "$npm_bin_version" ] \
    || fail "agent ($agent_version) ≠ npm oxiremoteBinaryVersion ($npm_bin_version)"
fi
[ "$agent_version" = "$landing_version" ] \
  || fail "agent ($agent_version) ≠ landing ($landing_version)"

echo
echo "Verifying agent/Cargo.lock is in sync…"
cargo metadata --manifest-path agent/Cargo.toml --locked --format-version 1 >/dev/null \
  || fail "Cargo.lock out of sync — run \`cargo build --manifest-path agent/Cargo.toml\` and commit Cargo.lock"

echo "all versions agree on ${agent_version}"
