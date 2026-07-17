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

- `default = ["desktop", "h264"]` — **as of phase-01 of `260511-0201-remote-desktop-pipeline-enhance`, H.264 is the default video pipeline.** `pipeline_selection::OperatorPref::Auto` resolves to H.264 when the client supports it (iPad Safari ≥17, Chrome, modern Android) and to JPEG otherwise. Linux requires X11/xcb dev headers + `libopenh264-dev`; see `.github/workflows/release.yml` "Install Linux build deps".
- `desktop` (no `h264`) — JPEG-only build (xcap + mozjpeg + WebRTC DataChannel). The compile-clean `--no-default-features --features desktop` invariant is enforced — `Pipeline::H264` stays `#[cfg(feature = "h264")]`, and `choose()` returns `Pipeline::Jpeg` with reason `auto-jpeg-no-feature`.
- `audio` feature on `crates/desktop` subcrate — **not in the global cargo default**, but **release artifacts for macOS + Windows targets are built with `--features audio`** (see `.github/workflows/release.yml` matrix `cargo_features`). Linux stays default-feature (no audio backend). Enable on a local build with `cargo build --features audio` (agent crate forwards to `desktop/audio`). Pulls `audiopus` (vendored libopus) + `rubato` resampler unconditionally; cpal is gated to the Windows target so macOS / Linux builds don't drag Core Audio / ALSA backends they don't use. **Phase-02a (macOS) is operational:** SCK delivers audio + video on the same SCStream, planar Float32 → interleaved i16 → 20 ms Opus frames → BUNDLE'd RTP track. **Phase-02b (Windows) software-complete:** `desktop::audio::wasapi_loopback` opens `cpal::default_host().default_output_device()` and runs an input stream on it — cpal-Windows auto-selects WASAPI shared-mode loopback (rustdesk-proven path). cpal's `Stream` is `!Send` on Windows so the stream lives on a dedicated holder thread; the `AudioCapture` handle holds only Send-safe channels. Audio capture is independent of the video pipeline on Windows (no SCK-style coupling). Default-OFF privacy posture: capture only starts when operator setting (`desktop_audio_enabled`) AND client opt-in (`capabilitiesClient.audio`) AND build probe (`desktop::audio::probe_supported`) all agree. Pipeline owns the single `AgentEvent::AudioStopped` emit point with stable wire reasons (`session_closed`, `user_toggle_off`, `sck_error` on macOS, `wasapi_error` on Windows); the caller picks the right capture-error reason via `AudioPipelineConfig::capture_error_reason`. Operator can revoke audio mid-session via `POST /api/agent/settings/audio` and the 2 s settings poll inside `run_h264_session` tears down audio without touching video. Linux deferred indefinitely.
- Build without remote desktop: `cargo build --no-default-features` (e.g. headless CI).
- Per-session override: the SPA can append `?force_pipeline=jpeg|h264|vp9|av1|auto` to the WS upgrade. The agent parses + validates and uses it for **that session only** without touching the env-var preference. Used by the SPA to honor `FallbackPending`, which the agent emits from two watchdogs: 5 s post-Connected no-IDR, and 15 s PC-never-Connected (`ice-timeout-15s` — UDP-blocked networks where STUN-only ICE can't complete; the signaling WS rides the tunnel and still delivers the fallback). The SPA additionally self-falls-back to JPEG on `pc.connectionState === 'failed'` or 10 s with an attached track but no decoded frame (black-stream guard); `streaming` status is only reported once a frame has actually been presented.
- Congestion control: the ABR controller (`desktop_abr.rs`) is the **single writer** to the encoder bitrate watch channel. The RTCP reader emits REMB/loss/RTT as observations only. Recovery keeps cutting each tick while loss persists (REMB-bounded), Comfort re-entry keeps the recovered bitrate (no ceiling jump), Probe climbs back +10 %/tick, floor `ABR_FLOOR_KBPS` = 250 kbps — this is what makes H.264/VP9/AV1 usable on links slower than the tier bitrate instead of black-screening.

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

When `OXI_DISCOVERY_URL` is set, the agent posts `{apiKey: discovery_id, tunnelUrl}` to the worker after each `TunnelStepChanged{Ready}` (not on raw `TunnelUrlChanged` — prevents registering a known-bad URL) and mints a 30-min temp key. The TUI QR then encodes `<discovery_url>/login?k=<tempKey>&otk=<otk>` so a standalone SPA on Cloudflare Pages can resolve the tunnel URL before initiating the pairing exchange — no manual tunnel-URL entry required when the tunnel rotates between sessions. `discovery_id` is a stable 32-byte random hex seeded once into `settings` on first boot, independent of `permanent_key_hash` so a key rotation does not invalidate the discovery mapping. Cross-origin POST `/api/login/one-time` is exempt from CSRF (single-use OTK is itself proof of presence) and now returns `api_key + host_id` so the SPA can Bearer-auth subsequent calls without a follow-up `/api/host` round-trip. Worker source: `apps/discovery-worker/`. Deploy: `wrangler deploy`. Embedded mode (env unset) is preserved exactly.

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

The dashboard `TunnelStatusPill` and `TunnelStatusCard` render a tri-state: green Reachable / amber Verifying / red Tunnel-unhealthy + reason. Outcomes map through `ProbeOutcome` (`Ok | Timeout | DohNxdomain | HttpError(u16) | Network(String)`) to `TunnelStep::Ready | Degraded | Failed`. A dismissible `NamedTunnelBanner` on the agent dashboard nudges quick-tunnel users toward named tunnels when discovery is configured.

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
| `OXI_VIDEO_PIPELINE=auto\|h264\|vp9\|av1\|jpeg` | Operator preference for remote-desktop pipeline. **Default `auto`** resolves AV1 > VP9 > H.264 > JPEG based on compiled features + client capability. `h264` / `vp9` / `av1` fail-closed on incapable clients (no silent fallback); `jpeg` forces JPEG. Unknown values fall back to `auto`. |
| `OXI_DISCOVERY_URL` | Cloudflare discovery-worker base URL (e.g. `https://oxiremote-discovery.<account>.workers.dev`). When set, the agent registers `discovery_id → tunnelUrl` after every `TunnelUrlChanged` and the QR encodes a cross-origin form. Unset = embedded-SPA mode (no behaviour change). |
| `OXI_STUN_URL` | Override the default STUN server (`stun:stun.l.google.com:19302`) used by both the agent's PeerConnection and the SPA (advertised via `GET /api/hosts/{id}/desktop/capabilities` → `ice_servers`). |
| `OXI_TURN_URL` | Optional TURN relay (e.g. `turns:turn.example.com:443?transport=tcp`). When set, both the agent and the SPA add it to their ICE servers, so remote-desktop media works on UDP-blocked / symmetric-NAT networks (CRD-style relay). Unset = STUN-only (previous behavior). |
| `OXI_TURN_USERNAME` / `OXI_TURN_PASSWORD` | Long-term credentials for `OXI_TURN_URL`. Served only on the Bearer+cookie-authed capabilities endpoint. |
| `OXI_GIT_BASH_PATH` | Windows only. Absolute path to `bash.exe` from Git for Windows when the standard install paths are non-default (e.g. portable Git, scoop). When unset, terminal sessions probe `C:\Program Files\Git\bin\bash.exe` then fall back to `powershell.exe`. |
| `OXIREMOTE_INSTALL_DIR` | Override install target dir for `scripts/install.sh`. |
| `OXIREMOTE_BINARY_URL` | npm-wrapper download base URL (corp proxies / mirrors). |
| `OXIREMOTE_VERSION` | Pin a specific release tag in the install script (`oxiremote update` always tracks `latest`). |
| `OXI_ABR_PROBE_INTERVAL_S` | **Advanced/debug.** Seconds in Comfort before the H.264 ABR controller probes for headroom. Default `5`, min `1`. |
| `OXI_ABR_RECOVERY_CUT_PCT` | **Advanced/debug.** Bitrate cut % applied on entry to Recovery zone. Default `30`, clamped `5-90`. |
| `OXI_ABR_PROBE_STEP_PCT` | **Advanced/debug.** Bitrate bump % per Probe tick. Default `10`, clamped `1-50`. |
| `OXI_ABR_HYSTERESIS_TICKS` | **Advanced/debug.** Consecutive 1 Hz ticks of agreement required for a non-Recovery zone transition. Default `2`, clamped `1-10`. (Recovery always skips hysteresis.) |
| `OXI_ABR_ANTI_THRASH_S` | **Advanced/debug.** Minimum seconds between any two zone transitions (Recovery exempt). Default `4`, min `1`. |

## Macros and gotchas

- `agent/build.rs` injects Swift runtime rpaths on macOS for `screencapturekit`. Order matters: `/usr/lib/swift` must come first to avoid duplicate-class loads.
- The desktop crate uses `rayon` for parallel per-tile encode; capture→send channel capacity is intentionally **2** (drop-newest backpressure via `try_send`).
- `desktop_service::DesktopService` enforces single-viewer per `device_id` — a second WS connection evicts the previous session.
- `webrtc-rs` quirk (saved in memory): on the answerer side, use `add_transceiver_from_track` **before** `set_remote_description`; `add_track()` creates an orphan transceiver.
- `notify` CLI authenticates to `localhost/api/notify` via `~/.oxiremote/notify.token` (chmod 0600). The route is Localhost-scoped — tunnel callers get 403.
