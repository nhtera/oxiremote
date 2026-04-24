import { expect, test } from '@playwright/test'

/**
 * Tunnel-scope 403 pentest. Forges a cloudflared-origin request by injecting
 * the `cf-connecting-ip` header — the exact condition `route_scope::tunnel_guard`
 * uses to mark a request tunnel-originating.
 *
 * `/agent/*` (dashboard SPA) and `/api/agent/*` (dashboard API + SSE + OTK
 * generation) MUST be 403 in this mode. Any change that regresses this would
 * leak the host dashboard or let anyone on the tunnel mint pairing tokens.
 *
 * No pairing code needed — these assertions are pure network contract checks.
 */

const TUNNEL_HEADER = { 'cf-connecting-ip': '203.0.113.77' } as const

test.describe('tunnel-scope 403', () => {
  test('GET /agent returns 403 over tunnel', async ({ request }) => {
    const res = await request.get('/agent', { headers: TUNNEL_HEADER })
    expect(res.status()).toBe(403)
  })

  test('GET /api/agent/events returns 403 over tunnel', async ({ request }) => {
    const res = await request.get('/api/agent/events', { headers: TUNNEL_HEADER })
    expect(res.status()).toBe(403)
  })

  test('GET /api/agent/state returns 403 over tunnel', async ({ request }) => {
    const res = await request.get('/api/agent/state', { headers: TUNNEL_HEADER })
    expect(res.status()).toBe(403)
  })

  test('POST /api/agent/keys/one-time returns 403 over tunnel', async ({ request }) => {
    const res = await request.post('/api/agent/keys/one-time', {
      headers: TUNNEL_HEADER,
    })
    expect(res.status()).toBe(403)
  })

  test('GET /api/agent/approvals/pending returns 403 over tunnel', async ({ request }) => {
    const res = await request.get('/api/agent/approvals/pending', {
      headers: TUNNEL_HEADER,
    })
    expect(res.status()).toBe(403)
  })
})
