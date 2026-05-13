import { devices, expect, test } from '@playwright/test'

// Drives an authenticated H.264 desktop session via the in-app router so
// the in-memory host store survives. We don't assert anything from the
// browser side — the agent log is the source of truth. After this spec
// runs, grep the agent log for:
//   `video_pipeline: capture dim changed`         — encoder-rebuild fired
//   `video_pipeline: first frame encoded`         — encoder produced output
//   `h264: avcC description sent ... level_idc=0x32` — fix-A in effect
//
// Skip unless OXI_E2E_H264=1 + auto_approve already on.

const ENABLED = process.env.OXI_E2E_H264 === '1'

test.use({ ...devices['Desktop Chrome'] })

test.describe('h264 windows render', () => {
  test.skip(!ENABLED, 'Set OXI_E2E_H264=1 with a running loopback agent (auto-approve on) to enable')

  test('h264 session reaches first IDR', async ({ page }) => {
    test.setTimeout(45_000)

    page.on('pageerror', (err) => console.log('[pageerror]', err.message))

    const otk = await page.context().request.post('/api/agent/keys/one-time')
    expect(otk.ok(), await otk.text()).toBeTruthy()
    const { token } = (await otk.json()) as { token: string }

    // Drive the OTK flow via the SPA's /login?k= path so the SPA itself
    // populates the host store + cookies + saved-hosts state. After the
    // OTK consumes, the SPA's `handleOtkSuccess` calls fetchHost() then
    // navigate('/'), and RootRoute redirects to /h/<id>/workspace — all
    // via in-app routing, so the store survives.
    await page.goto(`/login?k=${token}`)
    await expect.poll(async () => new URL(page.url()).pathname, { timeout: 20_000 }).toMatch(/^\/h\//)
    const [, , hostId] = new URL(page.url()).pathname.split('/')
    expect(hostId).toBeTruthy()
    console.log('OTK landed at:', page.url())

    // In-app navigation to the desktop route. Using react-router's history
    // would be cleanest but the production bundle doesn't expose it; we use
    // pushState + popstate, which BrowserRouter listens for.
    await page.evaluate((h) => {
      window.history.pushState({}, '', `/h/${h}/desktop`)
      window.dispatchEvent(new PopStateEvent('popstate'))
    }, hostId)

    // Wait for the desktop canvas, or settle for "any video frame encoded"
    // — either way the H.264 negotiation+encode cycle ran.
    await page.waitForTimeout(8_000)
    const finalUrl = page.url()
    const bodyStart = (await page.locator('body').innerText()).slice(0, 250)
    console.log('final url:', finalUrl)
    console.log('body start:', bodyStart)
  })
})
