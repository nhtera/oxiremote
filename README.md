# OxiRemote

Self-hosted remote-anywhere agent + mobile-friendly web UI. Run a single binary on your dev machine, expose it through a Cloudflare Quick Tunnel, and reach your terminals, files, dev-server previews, and remote desktop from any browser.

## Install

> Until the first GitHub release ships, the install one-liner and `npm install -g oxiremote` will fail at the download step — they expect tagged release artifacts at `https://github.com/nhtera/oxiremote/releases`. Build from source (below) until then.

### macOS / Linux

```bash
curl -fsSL https://raw.githubusercontent.com/nhtera/oxiremote/main/scripts/install.sh | bash
```

Drops `oxiremote` into `$HOME/.local/bin`. Override with `OXIREMOTE_INSTALL_DIR=/usr/local/bin`. The script verifies SHA256 against the release manifest before installing.

### npm (any platform)

```bash
npm install -g oxiremote
```

The npm package is a [thin wrapper](./npm-wrapper) — installing it downloads the matching prebuilt binary on `postinstall`. Same SHA256 verification.

### Windows

Either install via npm (above) or download `oxiremote-<version>-x86_64-pc-windows-msvc.zip` from the [Releases page](https://github.com/nhtera/oxiremote/releases), extract, and put `oxiremote.exe` on your PATH.

### GitHub Codespaces

Drop the included `.devcontainer/devcontainer.json` into your repository (or copy it). On boot, Codespaces installs OxiRemote and starts it in headless mode (`oxiremote --auto`); the QR code prints to the Codespace console. Forwarded port `8787` exposes the dashboard locally; the Cloudflare tunnel URL is the address you scan from your phone.

### From source

```bash
bun install
bun run build:release
./agent/target/release/oxiremote
```

Requires `bun` and a Rust toolchain.

## Self-update

```bash
oxiremote update
```

Fetches the latest GitHub release for your target triple, verifies SHA256 against the published manifest, atomic-replaces the running binary. Restart the agent to pick up the new version. Set `OXIREMOTE_VERSION=v0.2.3` (in `scripts/install.sh`) to pin a specific release; `oxiremote update` always tracks `latest`.

## First run

On first launch the agent downloads `cloudflared`, opens a Quick Tunnel, and prints a pairing code in the TUI. Open the tunnel URL on your phone, scan the QR, enter the pairing code (or scan a deep-link QR with an active one-time key for one-tap pairing).

## Environment variables

- `OXI_SECURE_COOKIES=1` — mark auth cookies as `Secure` (recommended over HTTPS / tunnel)
- `OXI_WORKSPACE=/path/to/project` — set the workspace root (defaults to CWD)
- `OXI_HEADLESS=1` — force headless server mode even when a TTY is attached
- `OXIREMOTE_INSTALL_DIR=...` — install script target directory
- `OXIREMOTE_BINARY_URL=...` — npm wrapper download base URL (corp proxies / mirrors)

## Notifications (Web Push)

The agent runs a Web Push server. Install the web UI as a PWA on your phone (Add to Home Screen on iOS), enable notifications from the in-app banner, then trigger a push from the shell:

```bash
oxiremote notify --title "build done" \
  --body "vite production build OK" \
  --deep-link "/h/<host_id>/terminal/<session_id>"
```

Tapping the notification opens the deep link on the correct host. The CLI reads `~/.oxiremote/notify.token` (chmod 0600, created on first run) to authenticate against the localhost `/api/notify` endpoint.

## Named tunnels (production)

For a stable hostname, configure a Cloudflare Named Tunnel:

```bash
# 1. Write the config scaffold
oxiremote tunnel use my-tunnel-name

# 2. Create the tunnel and route DNS (standard cloudflared commands)
cloudflared tunnel login
cloudflared tunnel create my-tunnel-name
cloudflared tunnel route dns my-tunnel-name oxi.example.com
```

When `~/.config/oxiremote/tunnel.toml` is present, the agent skips Quick Tunnel and runs your named tunnel. Edit the file to point at a specific `credentials_file` if cloudflared can't auto-discover it.

## Dev

Run both agent + web UI:

```bash
bun dev
```

In dev mode, the agent serves API endpoints and the Vite dev server handles the React UI at `localhost:5173`.

Run only one side:

```bash
bun run dev:web    # web UI only
bun run dev:agent  # agent only
```

## Releases (maintainers)

```bash
git tag v0.1.0
git push --tags
```

The `release` GitHub Action builds the binary on each target's native runner, generates a single SHA256 manifest, and uploads everything to the matching GitHub Release. Re-running with `workflow_dispatch` rebuilds without re-tagging.

## Structure

- `agent/` — Rust local agent (HTTP server, tunnel, services, tray, TUI)
- `apps/web/` — React/TS web UI (Vite)
- `npm-wrapper/` — thin npm package that downloads the prebuilt binary
- `scripts/install.sh` — curl-pipe-sh installer
- `.devcontainer/` — Codespaces config
- `.github/workflows/release.yml` — multi-platform release builder
