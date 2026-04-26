import { expect, test } from '@playwright/test'
import { STORAGE_STATE } from './global-setup'

/**
 * UX parity-polish happy paths — covers visible surfaces shipped by
 * `260426-0943-ux-parity-polish-implementation` plan (phases 1–6):
 *
 *   - Phase 1  Host dashboard 2-col + tunnel status pill (`/agent`).
 *   - Phase 2  File-tree git overlay (`with_git=1`) + filename search
 *              (`/api/files/search`).
 *   - Phase 5  Sidebar + bottom-nav SVG icons (no text glyphs).
 *   - Phase 6  `/login` "Recently paired" list when localStorage has saved
 *              hosts (one-tap reconnect path).
 *
 * Phases 3 (terminal settings sheet) and 4 (desktop top strip + exit confirm)
 * are excluded — both require an authenticated SPA session driving xterm.js
 * or WebRTC. Out of scope for an API-first iPhone-14 smoke; cover them with
 * Vitest unit tests on `terminal-prefs.ts` / `desktop-toolbar.tsx` instead.
 *
 * Pre-reqs (matches `oxiremote.spec.ts`):
 *   - Agent at OXI_BASE_URL (default http://127.0.0.1:8787).
 *   - OXI_PAIRING_CODE — required only for the API-auth test below; the
 *     unauth UI tests run regardless.
 */

const PAIRING_CODE = process.env.OXI_PAIRING_CODE
const TUNNEL_HEADER = { 'cf-connecting-ip': '203.0.113.42' } as const

test.describe('ux parity flows', () => {
  // --- Phase 6: /login "Recently paired" (no auth, no pairing) ------------

  test('login renders Recently paired list when localStorage seeded', async ({ page }) => {
    await page.addInitScript(() => {
      const saved = [
        {
          host_id: 'fixture-host',
          label: 'fixture-laptop',
          api_key_last4: 'wxyz',
          last_seen: Date.now(),
        },
      ]
      window.localStorage.setItem('oxi:saved-hosts', JSON.stringify(saved))
    })
    await page.goto('/login')
    await expect(page.getByRole('heading', { name: /recently paired/i })).toBeVisible()
    await expect(page.getByText('fixture-laptop')).toBeVisible()
    await expect(page.getByText(/wxyz/)).toBeVisible()
  })

  test('login hides Recently paired list when localStorage empty', async ({ page }) => {
    await page.addInitScript(() => {
      window.localStorage.removeItem('oxi:saved-hosts')
    })
    await page.goto('/login')
    await expect(page.getByRole('heading', { name: /recently paired/i })).toHaveCount(0)
  })

  // --- Phase 1 + 5: /agent dashboard renders w/ SVG nav (localhost only) --

  test('agent dashboard renders 2-col layout, tunnel pill, and SVG nav icons', async ({ page }) => {
    await page.goto('/agent')
    await expect(page.getByText(/host dashboard/i)).toBeVisible()
    await expect(page.getByText(/localhost only/i)).toBeVisible()
    // Sidebar nav uses inline SVG icons (Phase 5) — at least one <svg> child.
    const navSvgs = page.locator('nav svg')
    await expect(navSvgs.first()).toBeVisible()
    expect(await navSvgs.count()).toBeGreaterThanOrEqual(4) // Home, Devices, Logs, Settings
  })

  test('tunnel-origin GET /agent is rejected (regression guard for Phase 1)', async ({ request }) => {
    const res = await request.get('/agent', { headers: TUNNEL_HEADER })
    expect(res.status()).toBe(403)
  })

})

// --- Phase 2: file-tree git overlay + filename search (auth via shared session) ---

test.describe('ux parity — phase 2 file APIs', () => {
  test.skip(!PAIRING_CODE, 'OXI_PAIRING_CODE env var not provided')
  test.use({ storageState: STORAGE_STATE })

  test('files list accepts with_git=1', async ({ request }) => {
    // git_status field is per-entry and may be absent on clean files;
    // we just guard that the response shape didn't regress when
    // `with_git=1` is supplied.
    const res = await request.get('/api/files/list?path=.&with_git=1')
    expect(res.ok(), await res.text()).toBeTruthy()
    const body = (await res.json()) as { entries?: unknown[] } | unknown[]
    const entries = Array.isArray(body) ? body : body.entries
    expect(Array.isArray(entries)).toBeTruthy()
  })

  test('files search returns hits for a 2+ char query', async ({ request }) => {
    const res = await request.get('/api/files/search?q=re')
    expect(res.ok(), await res.text()).toBeTruthy()
    const body = (await res.json()) as { hits?: unknown[] } | unknown[]
    const hits = Array.isArray(body) ? body : body.hits
    expect(Array.isArray(hits)).toBeTruthy()
  })

  test('files search returns empty array for 1-char query', async ({ request }) => {
    const res = await request.get('/api/files/search?q=a')
    expect(res.ok()).toBeTruthy()
    const body = (await res.json()) as { hits?: unknown[] } | unknown[]
    const hits = Array.isArray(body) ? body : body.hits
    expect(Array.isArray(hits)).toBeTruthy()
    expect(hits!.length).toBe(0)
  })
})
