# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Repository Shape

Bun workspace at the root pinning `apps/*` (only `apps/web` today). Cargo workspace at `agent/` with members `.` (the `oxiremote` binary) and `crates/desktop` (capture/encode/input crate, gated by the `desktop` cargo feature). The release binary embeds the built SPA via `rust-embed` — `agent/build.rs` panics if `apps/web/dist/` is missing during a release build. **Always run `bun run build:web` before `cargo build --release`** (or use `bun run build:release`, which chains them).

## Common Commands

| Command | What it does |
|---|---|
| `bun dev` | Agent (debug, `cargo run`) + Vite dev server in parallel. SPA at `localhost:5173` with `/api`, `/preview`, `/ws` proxied to `127.0.0.1:8787`. |
| `bun run dev:web` / `bun run dev:agent` | Run just one side. |
| `bun run build:release` | Web assets, then `cargo build --release` → `agent/target/release/oxiremote`. |
| `bun run typecheck:web` / `bun run lint:web` | TypeScript + ESLint on the SPA. |
| `bun run check:bundle` | Asserts initial-route gzipped JS < 250 KB and lazy desktop chunk < 60 KB. Runs after `build:web`. |
| `bun run e2e` | Playwright (iPhone 14 profile). Needs a running agent and `OXI_PAIRING_CODE=<code>`. Some specs are opt-in: `OXI_E2E_OTK=1` for `otk-approval-desktop.spec.ts`. |
| `cargo test --manifest-path agent/Cargo.toml` | Rust unit tests (security middleware, auth, tunnel config, OTK). |
| `cargo clippy --manifest-path agent/Cargo.toml --all-targets` | Repo standard is **0 warnings**. |
| `cargo run --manifest-path agent/Cargo.toml -- <subcommand>` | See subcommands below. |

Single Rust test: `cargo test --manifest-path agent/Cargo.toml <name>` (e.g. `tunnel_guard_blocks_localhost_route`).

## Cargo Features

- `default = ["desktop"]` — JPEG remote-desktop pipeline (xcap + mozjpeg + WebRTC DataChannel). Linux requires X11/xcb dev headers; see `.github/workflows/release.yml` step "Install Linux build deps".
- `h264` — adds VideoToolbox (macOS) or OpenH264 (Linux) and the WebRTC video-track pipeline. **Default JPEG build must stay compile-clean** when `h264` is absent — `pipeline_selection::choose()` falls back to JPEG and `Pipeline::H264` is `#[cfg(feature = "h264")]`.
- Build without remote desktop: `cargo build --no-default-features` (e.g. headless CI).

## Subcommand Dispatch (`agent/src/main.rs`)

Subcommands are matched in `main()` **before** the tokio runtime starts so short-lived commands (`notify`, `update`, `--version`, `tunnel use`) don't pay multi-thread runtime cost. Bare invocation chooses TUI when stdout is a TTY and `OXI_HEADLESS` is unset, else headless. `--auto` / `--headless` / `serve` force headless (used by Codespaces `postStartCommand`). `ui` spawns a detached background agent and opens the browser.

## Architecture: things that need multiple files

### Single binary, two run modes

The agent is one process serving:
- `Public` routes (tunnel-reachable): `/api/*`, SPA, `/h/*`, terminal/desktop WebSockets.
- `Localhost` routes (loopback only): `/agent/*` host dashboard, `/api/agent/*` operator control plane, `/api/notify`, `/api/local-sites`, `/proxy/{port}/*`.

Tunnel-origin is detected by the presence of a non-empty `cf-connecting-ip` header injected by cloudflared. Loopback callers never have it; the four security layers are no-ops for loopback.

### Security middleware chain (outer → inner)

`tunnel_guard` → `rate_limit` → `csrf_guard` → `api_key_guard` → handler. Implemented in `agent/src/security/`. Route classification (`route_scope.rs`) is **path-prefix based** and built once at startup; changing the Localhost set means editing `route_scope.rs`. The `/proxy/{port}/*` route is exempt from `api_key_guard` because the upstream app does its own auth — the proxy is a transport, not an auth boundary.

### Pairing → API key → CSRF flow

1. `POST /api/pairing/exchange` consumes a one-time pairing code, mints an Argon2id-hashed API key, returns `{ ok, device_id, api_key, api_key_last4 }` (key shown once).
2. SPA stores `api_key` in `localStorage` keyed by `host_id`.
3. Tunnel requests carry `Authorization: Bearer <api_key>` + `X-OXI-CSRF: <oxi_csrf cookie>` (state-changing methods only). `api_key_last4` is the pre-filter so verification hashes at most one row.
4. Devices land in approval state `pending` (or `approved` if `settings.auto_approve=true`); `api_key_guard` rejects non-approved devices.

### Discovery worker (optional)

When `OXI_DISCOVERY_URL` is set, the agent posts `{apiKey: discovery_id, tunnelUrl}` to the worker after each `TunnelUrlChanged` and mints a 30-min temp key. The TUI QR then encodes `<discovery_url>/login?k=<tempKey>&otk=<otk>` so a standalone SPA on Cloudflare Pages can resolve the tunnel URL before initiating the pairing exchange — no manual tunnel-URL entry required when the tunnel rotates between sessions. `discovery_id` is a stable 32-byte random hex seeded once into `settings` on first boot, independent of `permanent_key_hash` so a key rotation does not invalidate the discovery mapping. Cross-origin POST `/api/login/one-time` is exempt from CSRF (single-use OTK is itself proof of presence) and now returns `api_key + host_id` so the SPA can Bearer-auth subsequent calls without a follow-up `/api/host` round-trip. Worker source: `apps/discovery-worker/`. Deploy: `wrangler deploy`. Embedded mode (env unset) is preserved exactly.

The worker also serves as a **transparent reverse proxy** at `<discovery_url>/proxy/<discovery_id>/<upstream-path>` (HTTP all-methods + WebSocket pass-through). The SPA routes pair traffic + post-pair `/api/*` + WS through this path whenever `/api/session/lookup` echoes a `discoveryId`, falling back to the direct tunnel URL on 502/504/TypeError. Cached `tunnelBase` stores the proxy URL after pair so existing fetch + WS sites route through the worker without per-site refactor; legacy `*.trycloudflare.com` bases get rewritten opportunistically at SPA boot. The worker resolves upstream via Cloudflare's authoritative resolver (independent of the SPA browser's local OS resolver), which is why this path masks DNS lag after sleep/wake. Strict origin allowlist; 600 req/min/IP on the `proxy:` rate-limit scope.

### Remote desktop pipeline selection

`OXI_VIDEO_PIPELINE=h264|jpeg` (operator preference) AND-merges with client `capabilitiesClient` codec list in `pipeline_selection::choose()` — both must agree on H.264 or the session falls back to JPEG. Wire format and SPA branch logic split across:
- Server: `desktop_ws.rs` (signaling), `desktop_ws_capture.rs` (JPEG capture/send), `video_pipeline.rs` (H.264, gated).
- SPA: `desktop-page.tsx` (mount choice via `caps.preferred_pipeline === 'h264' && supportsH264Video()`), `use-desktop-session.ts` (JPEG state machine), `use-desktop-video-session.ts` (H.264).
- Capability endpoint: `GET /api/hosts/{id}/desktop/capabilities` returns `preferred_pipeline`.

JPEG path uses an unordered DataChannel `"desktop"` for tile frames + `"ctrl"` ordered channel for input, with a 5 s WS-binary fallback race. H.264 path uses `TrackLocalStaticSample` recvonly transceiver; first IDR sends `SignalOut::Pipeline { mode: "h264", avcc_description_b64 }`.

### Event bus

`tokio::sync::broadcast<AgentEvent>` (256 slots) in `events.rs`. Drives the host dashboard SSE (`/api/agent/events`), the TUI, `notifier::spawn_event_notifier` (desktop toasts, 30 s dedup per device), and tray. Slow consumers see `RecvError::Lagged(n)` and skip ahead. SSE is filterable: `?filter=log` returns only `log_entry` frames.

### Database

SQLite at the agent data dir (default `~/.oxiremote/`). **No migration tool** — `db.rs` runs idempotent `CREATE TABLE IF NOT EXISTS` and `ALTER TABLE ... ADD COLUMN` on every startup. Adding a column means appending to that boot sequence, not a numbered migration. Tables: `sessions`, `trusted_devices`, `pairing_codes`, `one_time_keys`, `settings` (key/value, seeded with `auto_approve=false`, `desktop_quality=med`, `tunnel_mode=quick`, `proxy_allowed_ports`), `push_subscriptions`, `previews`.

### Tunnel

Quick Tunnel (default) spawns `cloudflared --url localhost:<port>`; URL is captured **once** from stderr and never rotates mid-process — `AgentEvent::TunnelUrlChanged` is one-shot. Named tunnel kicks in when `~/.config/oxiremote/tunnel.toml` exists (see `tunnel_named.rs`). cloudflared is auto-downloaded with SHA256 verification if not on PATH.

The agent self-heals zombie cloudflared processes via two complementary probes: `heartbeat.rs` detects sleep/wake skew and probes both loopback (`/api/health`) and the public tunnel URL; `edge_health_monitor.rs` issues a HEAD to the public URL every 30 s and triggers `force_respawn` after 3 consecutive failures (30 → 60 → 120 s backoff, 60 s respawn throttle). Both signal the supervisor via `force_respawn: Arc<Notify>`. `AgentEvent::EdgeUnhealthy { url, consecutive_failures }` is emitted for dashboard/tray surfaces. Reusable probe helpers live in `health_check.rs`.

### Self-update

`oxiremote update` (`update.rs`) fetches the latest GitHub release, downloads the matching target triple archive, verifies SHA256 against `oxiremote-<version>-sha256.txt`, and atomic-replaces the running binary. Restart required. The release workflow (`.github/workflows/release.yml`) builds per-target on native runners and concatenates per-asset `.sha256` files into the manifest — its filename must stay in sync with `update.rs`.

## Conventions

- **File naming**: kebab-case for TS/JS files. Rust uses snake_case (Rust convention).
- **File size**: keep individual files under ~200 LOC where reasonable; split into focused modules. The `agent/src/` directory is intentionally flat-by-concern (one file per service: `terminal_*.rs`, `desktop_*.rs`, `push*.rs`).
- **No new "enhanced" sibling files** — edit existing files in place.
- **No emojis** in code, commits, or docs unless explicitly requested.
- **Docs live in `./docs/`**; plans live in `./plans/{date}-{slug}/`. Both directories are `.gitignore`d intentionally — they're working artifacts, not shipped.
- **Bundle-size budget is enforced in CI**: 250 KB gz initial, 60 KB gz per lazy chunk. Adding heavyweight deps to the SPA fails `bun run check:bundle`.

## Environment Variables

| Var | Purpose |
|---|---|
| `OXI_SECURE_COOKIES=1` | Mark auth cookies `Secure` (required over HTTPS / tunnel). |
| `OXI_WORKSPACE=/path` | Workspace root exposed by the file browser (defaults to CWD). |
| `OXI_HEADLESS=1` | Force headless server even with a TTY. |
| `OXI_VIDEO_PIPELINE=h264\|jpeg` | Operator preference for remote desktop pipeline. |
| `OXI_DISCOVERY_URL` | Cloudflare discovery-worker base URL (e.g. `https://oxiremote-discovery.<account>.workers.dev`). When set, the agent registers `discovery_id → tunnelUrl` after every `TunnelUrlChanged` and the QR encodes a cross-origin form. Unset = embedded-SPA mode (no behaviour change). |
| `OXI_GIT_BASH_PATH` | Windows only. Absolute path to `bash.exe` from Git for Windows when the standard install paths are non-default (e.g. portable Git, scoop). When unset, terminal sessions probe `C:\Program Files\Git\bin\bash.exe` then fall back to `powershell.exe`. |
| `OXIREMOTE_INSTALL_DIR` | Override install target dir for `scripts/install.sh`. |
| `OXIREMOTE_BINARY_URL` | npm-wrapper download base URL (corp proxies / mirrors). |
| `OXIREMOTE_VERSION` | Pin a specific release tag in the install script (`oxiremote update` always tracks `latest`). |

## Macros and gotchas

- `agent/build.rs` injects Swift runtime rpaths on macOS for `screencapturekit`. Order matters: `/usr/lib/swift` must come first to avoid duplicate-class loads.
- The desktop crate uses `rayon` for parallel per-tile encode; capture→send channel capacity is intentionally **2** (drop-newest backpressure via `try_send`).
- `desktop_service::DesktopService` enforces single-viewer per `device_id` — a second WS connection evicts the previous session.
- `webrtc-rs` quirk (saved in memory): on the answerer side, use `add_transceiver_from_track` **before** `set_remote_description`; `add_track()` creates an orphan transceiver.
- `notify` CLI authenticates to `localhost/api/notify` via `~/.oxiremote/notify.token` (chmod 0600). The route is Localhost-scoped — tunnel callers get 403.
