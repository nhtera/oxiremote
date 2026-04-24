# OxiRemote

Self-hosted remote-anywhere agent + mobile-friendly web UI.

## Prereqs
- bun
- Rust (cargo)

## Production Build

Build a single binary with embedded web assets:

```bash
bun run build:release
```

The binary is at `agent/target/release/agent`. Run it:

```bash
./agent/target/release/agent
```

On first run, the agent downloads `cloudflared`, creates a Quick Tunnel, and prints a pairing code. Open the tunnel URL on your phone and enter the code.

### Environment variables
- `OXI_SECURE_COOKIES=1` — mark auth cookies as `Secure` (recommended over HTTPS / tunnel)
- `OXI_WORKSPACE=/path/to/project` — set the workspace root (defaults to CWD)

### Notifications (Web Push)

The agent runs a Web Push server. Install the web UI as a PWA on your phone (Add to Home Screen on iOS), enable notifications from the in-app banner, then trigger a push from the shell:

```bash
./agent/target/release/agent notify --title "build done" \
  --body "vite production build OK" \
  --deep-link "/h/<host_id>/terminal/<session_id>"
```

Tapping the notification opens the deep link on the correct host. The CLI reads `~/.oxiremote/notify.token` (chmod 0600, created on first run) to authenticate against the localhost `/api/notify` endpoint.

## Dev

Run both agent + web UI:

```bash
bun dev
```

In dev mode, the agent serves API endpoints and the Vite dev server handles the React UI at `localhost:5173`.

### Agent notes
- On startup, the agent ensures `cloudflared` is available.
  - Auto-downloads the latest release from GitHub and verifies its SHA-256 against Cloudflare’s published checksums.
  - Supported hosts: macOS (arm64/x64), Linux (x64/arm64), Windows (x64).

### Named tunnels (production)

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

Run only web UI:

```bash
bun run dev:web
```

Run only agent:

```bash
bun run dev:agent
```

## Structure
- `agent/` Rust local agent (HTTP server, tunnel, services)
- `apps/web/` React/TS web UI
