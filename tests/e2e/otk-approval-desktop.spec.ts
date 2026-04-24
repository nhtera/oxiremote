import { expect, test } from '@playwright/test'

/**
 * OTK → approval → workspace → desktop first-frame end-to-end.
 *
 * Pre-reqs:
 *   - A running agent reachable at `baseURL` (via `bun dev` or the release
 *     binary; see `playwright.config.ts`). The agent must be addressable on
 *     the loopback interface so `/api/agent/*` calls succeed.
 *
 * Skip when `OXI_E2E_OTK=1` is unset — localhost-only endpoints can't be
 * driven from an arbitrary environment, so this spec is opt-in for
 * developers who are running the agent locally.
 *
 * Canvas first-frame assertion: waits for the `<canvas>` element on the
 * desktop page to have at least one non-zero pixel. This is a Playwright-
 * safe proxy for "the DataChannel / WS fallback delivered a frame" — we
 * can't inspect DataChannels directly from the browser context.
 */

const ENABLED = process.env.OXI_E2E_OTK === '1'

test.describe('otk → approval → desktop first frame', () => {
  test.skip(!ENABLED, 'Set OXI_E2E_OTK=1 with a running agent to enable this flow.')

  test('full pairing + desktop first-frame round-trip', async ({ page, request }) => {
    // 1) Generate a one-time key (localhost-only)
    const otkRes = await request.post('/api/agent/keys/one-time')
    expect(otkRes.ok(), await otkRes.text()).toBeTruthy()
    const { token } = (await otkRes.json()) as { token: string }
    expect(token.length).toBeGreaterThan(8)

    // 2) Open /login?k=<token> on the iPhone profile
    await page.goto(`/login?k=${token}`)

    // 3) Waiting-for-approval screen
    await expect(page.getByText(/waiting for approval/i)).toBeVisible({ timeout: 10_000 })

    // 4) Find the pending device_id and approve it
    const pendingRes = await request.get('/api/agent/approvals/pending')
    expect(pendingRes.ok()).toBeTruthy()
    const pending = (await pendingRes.json()) as Array<{ device_id: string }>
    expect(pending.length).toBeGreaterThan(0)
    const deviceId = pending[0].device_id

    const approveRes = await request.post(`/api/agent/approvals/${deviceId}/approve`)
    expect(approveRes.ok()).toBeTruthy()

    // 5) Client polls → redirects to a host-scoped URL
    await page.waitForURL(/\/h\//, { timeout: 10_000 })

    // 6) Navigate to the desktop page
    const url = new URL(page.url())
    const [, , hostId] = url.pathname.split('/') // `/h/{hostId}/...`
    await page.goto(`/h/${hostId}/desktop`)

    // 7) Wait for canvas + non-zero pixel = first frame received.
    await page.waitForSelector('canvas', { state: 'visible', timeout: 8_000 })
    await expect
      .poll(
        async () =>
          page.evaluate(() => {
            const c = document.querySelector('canvas') as HTMLCanvasElement | null
            if (!c || c.width === 0 || c.height === 0) return false
            const ctx = c.getContext('2d')
            if (!ctx) return false
            const { data } = ctx.getImageData(0, 0, 1, 1)
            return data[0] + data[1] + data[2] > 0
          }),
        { timeout: 8_000, intervals: [500, 500, 1000, 1000, 2000] },
      )
      .toBe(true)
  })
})
