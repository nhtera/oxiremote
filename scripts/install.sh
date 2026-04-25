#!/usr/bin/env bash
# OxiRemote installer — downloads the latest GitHub release for the host's
# target triple, verifies the SHA256 against the published manifest, and
# drops the binary into the user's local bin (no sudo).
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/nhtera/oxiremote/main/scripts/install.sh | bash
#
# Override the install directory with OXIREMOTE_INSTALL_DIR=/usr/local/bin.
# Override the version with OXIREMOTE_VERSION=v0.1.0 (default: latest).

set -euo pipefail

REPO="nhtera/oxiremote"
INSTALL_DIR="${OXIREMOTE_INSTALL_DIR:-$HOME/.local/bin}"
VERSION_TAG="${OXIREMOTE_VERSION:-}"

err() { printf "\033[31merror:\033[0m %s\n" "$*" >&2; exit 1; }
info() { printf "\033[36m==>\033[0m %s\n" "$*"; }

# Detect target triple → must match the asset names produced by the
# release workflow. Linux musl + arm Linux are not built in v0.1.
detect_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os/$arch" in
    Darwin/arm64)         echo "aarch64-apple-darwin" ;;
    Darwin/x86_64)        echo "x86_64-apple-darwin" ;;
    Linux/x86_64)         echo "x86_64-unknown-linux-gnu" ;;
    Linux/aarch64)        echo "aarch64-unknown-linux-gnu" ;;
    Linux/arm64)          echo "aarch64-unknown-linux-gnu" ;;
    *) err "no prebuilt binary for $os/$arch — install from source" ;;
  esac
}

resolve_version() {
  if [ -n "$VERSION_TAG" ]; then
    echo "${VERSION_TAG#v}"
    return
  fi
  local api="https://api.github.com/repos/$REPO/releases/latest"
  curl -fsSL "$api" \
    | grep -m1 '"tag_name"' \
    | sed -E 's/.*"tag_name": *"v?([^"]+)".*/\1/'
}

main() {
  command -v curl >/dev/null || err "curl is required"
  command -v tar  >/dev/null || err "tar is required"
  command -v shasum >/dev/null || command -v sha256sum >/dev/null \
    || err "shasum or sha256sum is required"

  local target version asset manifest base_url tmp
  target="$(detect_target)"
  version="$(resolve_version)"
  [ -n "$version" ] || err "could not resolve latest version — has the first release been published?"

  info "Installing oxiremote $version for $target → $INSTALL_DIR"

  asset="oxiremote-${version}-${target}.tar.gz"
  manifest="oxiremote-${version}-sha256.txt"
  base_url="https://github.com/$REPO/releases/download/v${version}"

  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT

  info "Downloading $asset"
  curl -fsSL "$base_url/$asset"    -o "$tmp/$asset"
  curl -fsSL "$base_url/$manifest" -o "$tmp/$manifest"

  info "Verifying checksum"
  local expected actual
  expected="$(awk -v f="$asset" '$2 == f {print $1}' "$tmp/$manifest")"
  [ -n "$expected" ] || err "manifest does not list $asset"
  if command -v shasum >/dev/null; then
    actual="$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')"
  else
    actual="$(sha256sum "$tmp/$asset" | awk '{print $1}')"
  fi
  [ "$expected" = "$actual" ] || err "checksum mismatch (expected $expected, got $actual)"

  info "Extracting"
  ( cd "$tmp" && tar -xzf "$asset" )
  local stem="${asset%.tar.gz}"

  mkdir -p "$INSTALL_DIR"
  install -m 0755 "$tmp/$stem/oxiremote" "$INSTALL_DIR/oxiremote"

  info "Installed to $INSTALL_DIR/oxiremote"
  case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *) printf "\nNOTE: %s is not on your PATH. Add this to your shell rc:\n  export PATH=\"%s:\$PATH\"\n\n" "$INSTALL_DIR" "$INSTALL_DIR" ;;
  esac
  info "Run \`oxiremote\` to start, or \`oxiremote --help\` for the full CLI."
}

main "$@"
