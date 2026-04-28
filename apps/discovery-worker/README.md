# OxiRemote Discovery Worker

Tiny Cloudflare Worker that maps `sha256(permanent_key)` → `tunnelUrl` so the
standalone SPA can resolve an agent's current Quick Tunnel URL from a short-
lived temp key embedded in a QR code. No data relay — all traffic stays
peer-to-peer between SPA and agent.

## Routes

| Method | Path | Body / Query | Response |
|--------|------|--------------|----------|
| POST | `/api/session/create` | `{ apiKey }` | `{ ok: true }` |
| POST | `/api/session/update` | `{ apiKey, tunnelUrl, localIp? }` | `{ ok: true }` |
| POST | `/api/temp-key/create` | `{ apiKey, expiryMinutes? }` | `{ tempKey, expiresAt }` |
| GET  | `/api/session/lookup?k=<tempKey>` | — | `{ tunnelUrl, localIp }` or 404 |
| OPTIONS | * | — | 204 + CORS preflight |

`apiKey` is **always** a 64-char lowercase hex string (SHA-256 of the agent's
permanent key). Plaintext keys never leave the agent.

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

Smoke test once deployed (substitute your worker URL):

```bash
HASH=$(printf 'demo' | shasum -a 256 | cut -d' ' -f1)
W=https://oxiremote-discovery.<account>.workers.dev

curl -sX POST $W/api/session/create  -H 'content-type: application/json' -d "{\"apiKey\":\"$HASH\"}"
curl -sX POST $W/api/session/update  -H 'content-type: application/json' -d "{\"apiKey\":\"$HASH\",\"tunnelUrl\":\"https://example.trycloudflare.com\"}"
TK=$(curl -sX POST $W/api/temp-key/create -H 'content-type: application/json' -d "{\"apiKey\":\"$HASH\"}" | jq -r .tempKey)
curl -s "$W/api/session/lookup?k=$TK"
```

## Constraints

- Free tier: 100k KV reads/day, 1k writes/day. Each pair flow uses 3 writes
  (create + update + temp-key) and 1 read (lookup). Plenty of headroom for
  personal use.
- KV TTL = 30 min on every write — no cleanup job needed.
- Rate limit: 20 state-changing req/min/IP, in-memory, best-effort.
- CORS allowlist: `oxiremote.app`, `*.pages.dev`, `localhost:5173/4173`.
