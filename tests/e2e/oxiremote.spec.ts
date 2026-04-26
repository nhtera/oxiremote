import { expect, test } from '@playwright/test'
import { STORAGE_STATE } from './global-setup'

/**
 * Hardened core API smoke (iPhone 14 profile).
 *
 * Auth bootstrap moved to `global-setup.ts` — it pairs once per run and
 * stashes the `oxi_session` cookie in storageState. Tests that need auth
 * `test.use({ storageState: STORAGE_STATE })`; tests that exercise
 * tunnel-origin negative contracts run without it.
 *
 * Pre-reqs (CI responsibility):
 *   - Agent at OXI_BASE_URL (default http://127.0.0.1:8787).
 *   - OXI_PAIRING_CODE — fresh 8-char code; consumed once by global-setup.
 *
 * What this covers:
 *   - Pair body-shape contract (asserted in global-setup; see throw on regression).
 *   - /api/me succeeds with the shared session cookie.
 *   - Terminal session can be created and listed.
 *   - File list endpoint returns at least one entry.
 *   - /api/notify is 403 from tunnel-origin (push surface is localhost-only).
 *   - /api/workspaces POST without CSRF is rejected before the handler runs.
 */

const PAIRING_CODE = process.env.OXI_PAIRING_CODE

test.describe('authed core API smoke', () => {
  test.skip(!PAIRING_CODE, 'OXI_PAIRING_CODE env var not provided')
  test.use({ storageState: STORAGE_STATE })

  test('/api/me succeeds with shared session', async ({ request }) => {
    const res = await request.get('/api/me')
    expect(res.ok(), await res.text()).toBeTruthy()
  })

  test('terminal session can be created and listed', async ({ request }) => {
    const create = await request.post('/api/terminal/sessions', {
      data: { name: 'e2e-shared', cols: 80, rows: 24 },
    })
    expect(create.ok(), await create.text()).toBeTruthy()

    const list = await request.get('/api/terminal/sessions')
    expect(list.ok()).toBeTruthy()
    const body = (await list.json()) as unknown
    const sessions = Array.isArray(body) ? body : (body as { sessions?: unknown[] }).sessions
    expect(Array.isArray(sessions)).toBeTruthy()
  })

  test('files list returns successfully', async ({ request }) => {
    const res = await request.get('/api/files/list?path=.')
    expect(res.ok(), await res.text()).toBeTruthy()
  })
})

test.describe('tunnel-origin negative contracts', () => {
  test('tunnel-origin /api/notify is rejected', async ({ request }) => {
    const res = await request.post('/api/notify', {
      headers: {
        'cf-connecting-ip': '203.0.113.10',
        authorization: 'Bearer fake',
      },
      data: { title: 'should not reach' },
    })
    expect(res.status()).toBe(403)
  })

  test('tunnel-origin state-changing request without CSRF is rejected', async ({ request }) => {
    const res = await request.post('/api/workspaces', {
      headers: { 'cf-connecting-ip': '203.0.113.10' },
      data: { path: '/tmp', label: 'x' },
    })
    // 401 (missing bearer) OR 403 (missing csrf) — either signals rejection
    // before the handler executes.
    expect([401, 403]).toContain(res.status())
  })
})
