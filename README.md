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

## Dev

Run both agent + web UI:

```bash
bun dev
```

In dev mode, the agent serves API endpoints and the Vite dev server handles the React UI at `localhost:5173`.

### Agent notes
- On startup, the agent ensures `cloudflared` is available.
  - If not found in the agent data dir, it auto-downloads the latest **macOS** `cloudflared` release from GitHub and verifies its SHA-256 against Cloudflare’s published checksums.
  - Non-macOS platforms currently error if `cloudflared` is missing (auto-download is macOS-only).

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
