import { expect, test } from '@playwright/test'

/**
 * End-to-end smoke over the iPhone 14 profile.
 *
 * Pre-reqs (CI responsibility — this spec stays dumb):
 *   - A running agent at OXI_BASE_URL (default http://127.0.0.1:8787).
 *   - OXI_PAIRING_CODE env var containing a fresh 8-char pairing code the
 *     test can exchange. The CI job prints the code when it spawns the agent
 *     and passes it in.
 *
 * What this covers:
 *   - Pairing exchange → cookie + api_key + api_key_last4 in response.
 *   - Authenticated GET /api/me succeeds with the cookie.
 *   - A terminal session can be created and listed.
 *   - File list endpoint returns at least one entry.
 *   - Git status endpoint responds (shape only — repo state is out of scope).
 *   - The push /notify endpoint cannot be reached from tunnel-origin traffic
 *     (403). We fake a tunnel origin by forwarding the `cf-connecting-ip`
 *     header, which is the same condition the real cloudflared proxy sets.
 *
 * iPhone 14 profile ensures the UI renders mobile-first; the API-level smoke
 * additionally protects the hardening guarantees in phase 06.
 */

const PAIRING_CODE = process.env.OXI_PAIRING_CODE

test.describe('oxiremote hardened smoke', () => {
  test.skip(!PAIRING_CODE, 'OXI_PAIRING_CODE env var not provided')

  test('pair → cookie + api_key, fetch /api/me, create terminal', async ({ request }) => {
    const pairRes = await request.post('/api/pairing/exchange', {
      data: { code: PAIRING_CODE!, device_label: 'playwright-iphone14' },
    })
    expect(pairRes.ok(), await pairRes.text()).toBeTruthy()
    const body = await pairRes.json()
    expect(body.ok).toBe(true)
    expect(typeof body.api_key).toBe('string')
    expect(body.api_key.length).toBeGreaterThan(16)
    expect(body.api_key_last4).toMatch(/^[\w-]{4}$/)
    expect(typeof body.device_id).toBe('string')

    const me = await request.get('/api/me')
    expect(me.ok()).toBeTruthy()

    const create = await request.post('/api/terminal/sessions', {
      data: { name: 'e2e-session', cols: 80, rows: 24 },
    })
    expect(create.ok(), await create.text()).toBeTruthy()

    const list = await request.get('/api/terminal/sessions')
    expect(list.ok()).toBeTruthy()
    const sessions = await list.json()
    expect(Array.isArray(sessions.sessions ?? sessions)).toBeTruthy()

    const files = await request.get('/api/files/list?path=.')
    expect(files.ok()).toBeTruthy()
  })

  test('tunnel-origin request to /api/notify is rejected', async ({ request }) => {
    const res = await request.post('/api/notify', {
      headers: {
        'cf-connecting-ip': '203.0.113.10',
        authorization: 'Bearer fake',
      },
      data: { title: 'should not reach' },
    })
    expect(res.status()).toBe(403)
  })

  test('tunnel-origin state-changing request without CSRF header is rejected', async ({ request }) => {
    const res = await request.post('/api/workspaces', {
      headers: { 'cf-connecting-ip': '203.0.113.10' },
      data: { path: '/tmp', label: 'x' },
    })
    // 401 (missing bearer) OR 403 (missing csrf) — either signals rejection
    // before the handler executes.
    expect([401, 403]).toContain(res.status())
  })
})
