# OxiRemote Discovery Worker

Tiny Cloudflare Worker that maps `sha256(permanent_key)` → `tunnelUrl` so the
standalone SPA can resolve an agent's current Quick Tunnel URL from a short-
lived temp key embedded in a QR code. No data relay — all traffic stays
peer-to-peer between SPA and agent.

## Routes

| Method | Path | Body / Query | Auth | Response |
|--------|------|--------------|------|----------|
| POST | `/api/session/create` | `{ apiKey }` | Bearer | `{ ok: true }` |
| POST | `/api/session/update` | `{ apiKey, tunnelUrl }` | Bearer | `{ ok: true }` |
| POST | `/api/temp-key/create` | `{ apiKey, expiryMinutes? }` | Bearer | `{ tempKey, expiresAt }` |
| POST | `/api/code/register` | `{ apiKey, code, expiryMinutes? }` | Bearer | `{ ok: true, expiresAt }` |
| GET  | `/api/session/lookup?k=<tempKey>` | — | — | `{ tunnelUrl }` or 404 |
| OPTIONS | * | — | — | 204 + CORS preflight |

`apiKey` is **always** a 64-char lowercase hex string (the agent's stable
`discovery_id`, not derived from any key). Plaintext API keys never leave the
agent.

Mutating routes (Phase 04 / H1) require `Authorization: Bearer <secret>` where
`<secret>` is whatever was set via `wrangler secret put
AGENT_REGISTRATION_SECRET`. The secret is fail-closed: an unset secret returns
`503 worker not configured`. Lookups stay open (the SPA has no Bearer).
`tunnelUrl` must parse as `https:` (Phase 04 / H2). 32-hex machine temp keys
are single-use (delete-on-read, Phase 04 / H4); pairing codes / OTKs / 16-hex
permanent-key lookup_ids stay re-resolvable.

## Local dev

```bash
bun install
bun run dev          # wrangler dev with miniflare
bun run test         # vitest unit tests
bun run typecheck    # tsc --noEmit
```

## Deploy

One-time setup (per Cloudflare account):

```bash
wrangler login
wrangler kv namespace create DISCOVERY
wrangler kv namespace create DISCOVERY --preview
# paste the two IDs into wrangler.toml
wrangler deploy
```

Smoke test once deployed (substitute your worker URL + the agent's
discovery secret — recover via `oxiremote config discovery-secret`):

```bash
HASH=$(printf 'demo' | shasum -a 256 | cut -d' ' -f1)
W=https://oxiremote-discovery.<account>.workers.dev
S="$(oxiremote config discovery-secret)"   # or paste the value directly
H_AUTH="Authorization: Bearer $S"
H_JSON='content-type: application/json'

curl -sX POST $W/api/session/create -H "$H_AUTH" -H "$H_JSON" -d "{\"apiKey\":\"$HASH\"}"
curl -sX POST $W/api/session/update -H "$H_AUTH" -H "$H_JSON" -d "{\"apiKey\":\"$HASH\",\"tunnelUrl\":\"https://example.trycloudflare.com\"}"
TK=$(curl -sX POST $W/api/temp-key/create -H "$H_AUTH" -H "$H_JSON" -d "{\"apiKey\":\"$HASH\"}" | jq -r .tempKey)
curl -s "$W/api/session/lookup?k=$TK"      # lookup is open
```

## Constraints

- Free tier: 100k KV reads/day, 1k writes/day. Each pair flow uses 3 writes
  (create + update + temp-key) and 1 read (lookup). Plenty of headroom for
  personal use.
- KV TTL = 30 min on every write — no cleanup job needed.
- Rate limit: 20 req/min/IP across mutating + lookup paths, in-memory,
  best-effort.
- CORS allowlist: `oxiremote.app`, `*.pages.dev`, `localhost:5173/4173`.
