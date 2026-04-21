# OxiRemote

Self-hosted remote-anywhere agent + mobile-friendly web UI.

## Prereqs
- bun
- Rust (cargo)
- Node (for web tooling; via bun)

## Dev
Run both agent + web UI:

```bash
bun dev
```

### Agent notes
- On startup, the agent ensures `cloudflared` is available.
  - If not found in the agent data dir, it auto-downloads the latest **macOS** `cloudflared` release from GitHub and verifies its SHA-256 against Cloudflare’s published checksums.
  - Non-macOS platforms currently error if `cloudflared` is missing (auto-download is macOS-only).
- `OXI_SECURE_COOKIES`: set to `1` or `true` to mark auth cookies as `Secure` (recommended when serving over HTTPS / Cloudflare Tunnel).


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
