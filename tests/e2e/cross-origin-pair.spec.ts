import { expect, test } from '@playwright/test'

/**
 * Cross-origin pairing through the discovery worker.
 *
 * Pre-reqs (when enabled):
 *   - Running agent reachable at `baseURL`
 *   - Fresh OTK exposed via `OXI_E2E_OTK_VALUE` (single-use; regenerate per run)
 *   - Tunnel URL exposed via `OXI_TUNNEL_URL` (defaults to baseURL — useful
 *     for purely local CI where the SPA + agent share an origin and the
 *     "cross-origin" form just exercises the discovery code path)
 *
 * The worker `GET /api/session/lookup` is mocked via `page.route()` so the
 * spec runs without deploying the live Cloudflare Worker. A separate
 * deployment-time smoke test should exercise the real worker round-trip.
 *
 * Skip when `OXI_E2E_DISCOVERY=1` is unset — opt-in pattern matches
 * `OXI_E2E_OTK` in `otk-approval-desktop.spec.ts`.
 */

const ENABLED = process.env.OXI_E2E_DISCOVERY === '1'

test.describe('cross-origin pairing via discovery worker', () => {
  test.skip(!ENABLED, 'Set OXI_E2E_DISCOVERY=1 with OXI_E2E_OTK_VALUE + OXI_TUNNEL_URL to enable.')

  test('temp key + otk deep-link completes pairing across origins', async ({ page, baseURL }) => {
    const tunnelUrl = process.env.OXI_TUNNEL_URL ?? baseURL ?? 'http://127.0.0.1:8787'
    const otk = process.env.OXI_E2E_OTK_VALUE
    if (!otk) {
      test.skip(true, 'OXI_E2E_OTK_VALUE is required (generate a fresh OTK before each run).')
      return
    }

    // Mock the worker lookup so the spec is portable across CI environments
    // — the real worker is exercised by a separate deployment smoke test.
    let lookupHits = 0
    await page.route('**/api/session/lookup*', async (route) => {
      lookupHits += 1
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ tunnelUrl, localIp: null }),
      })
    })

    // Inject VITE_DISCOVERY_URL into the SPA before login-page mounts so
    // `isDiscoveryMode()` returns true even when the build was made with the
    // env empty. We do this via a script tag executed before any other JS
    // rather than rebuilding the bundle for every CI run.
    await page.addInitScript(() => {
      ;(window as unknown as { __OXI_E2E_DISCOVERY_URL__: string }).__OXI_E2E_DISCOVERY_URL__ =
        'https://discovery.test.invalid'
    })

    // 32-char lowercase hex shape — must match `isLikelyTempKey` in
    // `apps/web/src/lib/discovery-client.ts`.
    const fakeTempKey = 'a'.repeat(32)
    await page.goto(`/login?k=${fakeTempKey}&otk=${otk}`)

    // Pairing completes → SPA navigates to workspace root. Auto-approve
    // setting on the agent determines whether we land on `/` or
    // `/approval-waiting` — accept either.
    await expect(page).toHaveURL(/\/(approval-waiting)?$/, { timeout: 15_000 })

    // The lookup mock must have been hit at least once — confirms the
    // discovery branch ran rather than falling through to embedded mode.
    expect(lookupHits, 'discovery /api/session/lookup must be invoked').toBeGreaterThanOrEqual(1)

    // localStorage api_key keyed by host_id is the canonical post-pair
    // assertion: discover the key and confirm shape.
    const apiKeyEntry = await page.evaluate(() => {
      for (let i = 0; i < localStorage.length; i++) {
        const k = localStorage.key(i)
        if (k && k.startsWith('oxi_api_key_')) {
          return { key: k, value: localStorage.getItem(k) ?? '' }
        }
      }
      return null
    })
    expect(apiKeyEntry, 'pairing must persist an oxi_api_key_<host_id> entry').not.toBeNull()
    expect(apiKeyEntry!.value.length).toBeGreaterThan(8)
  })
})
