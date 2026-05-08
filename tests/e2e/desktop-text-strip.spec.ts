import { expect, test } from '@playwright/test'
import { STORAGE_STATE } from './global-setup'

/**
 * Pinned text strip → `{t:'text', s}` ctrl-frame round-trip.
 *
 * Verifies that typing punctuation-heavy input into the mobile composer
 * dispatches a single literal-text frame on the ctrl DataChannel — the
 * regression target is the old per-char keycode synthesis where punctuation
 * silently dropped via `dom_code_to_key`.
 *
 * Pre-reqs (CI responsibility):
 *   - Agent at OXI_BASE_URL with desktop pipeline available (Screen Recording
 *     permission granted on macOS host).
 *   - OXI_PAIRING_CODE consumed by global-setup → shared session cookie.
 *
 * Skip when OXI_E2E_DESKTOP_TEXT=1 is unset — the spec needs a live host with
 * a running capture pipeline, same opt-in pattern as otk-approval-desktop.
 */

const ENABLED = process.env.OXI_E2E_DESKTOP_TEXT === '1'

test.describe('desktop pinned text strip', () => {
  test.skip(
    !ENABLED,
    'Set OXI_E2E_DESKTOP_TEXT=1 with a running agent + screen permission to enable',
  )
  test.use({ storageState: STORAGE_STATE })

  test('Send dispatches a single {t:"text"} ctrl frame with the literal payload', async ({
    page,
    request,
  }) => {
    // Capture every ctrl-channel frame the SPA emits. The frames flow over
    // an ordered DataChannel (`ctrl`) once WebRTC is up, with a WS-binary
    // fallback if the DC never opens. Patch both `send` paths before any
    // page script runs so we don't miss the DC race window.
    await page.exposeFunction('__captureFrame', (text: string) => {
      capturedFrames.push(text)
    })
    const capturedFrames: string[] = []

    await page.addInitScript(() => {
      const tap = (data: unknown) => {
        if (typeof data !== 'string') return
        if (data.includes('"t":"text"')) {
          ;(window as unknown as { __captureFrame: (s: string) => void })
            .__captureFrame(data)
        }
      }
      const wsOrig = WebSocket.prototype.send
      WebSocket.prototype.send = function (data) {
        tap(data)
        return wsOrig.call(this, data as Parameters<typeof wsOrig>[0])
      }
      const dcOrig = RTCDataChannel.prototype.send
      RTCDataChannel.prototype.send = function (data) {
        tap(data)
        return dcOrig.call(this, data as Parameters<typeof dcOrig>[0])
      }
    })

    // Resolve our device_id so the URL matches the session cookie's bound id.
    const meRes = await request.get('/api/me')
    expect(meRes.ok(), await meRes.text()).toBeTruthy()
    const { device_id: deviceId } = (await meRes.json()) as { device_id: string }

    await page.goto(`/h/${deviceId}/desktop`)

    const strip = page.getByRole('group', { name: 'Send text to remote' })
    await expect(strip).toBeVisible({ timeout: 10_000 })

    const input = strip.getByLabel('Text to send to remote machine')
    await input.tap()
    const payload = `Hello!@#$%^&*()_+-=[]{}|;:'",.<>/?`
    await page.keyboard.type(payload)
    await page.keyboard.press('Enter')

    // Field clears on send so the operator can chain another value.
    await expect(input).toHaveValue('')

    // Exactly one text frame, with the literal payload intact.
    await expect.poll(() => capturedFrames.length, { timeout: 5_000 }).toBeGreaterThan(0)
    const parsed = capturedFrames.map((f) => JSON.parse(f) as { t: string; s: string })
    const textFrames = parsed.filter((f) => f.t === 'text')
    expect(textFrames.length).toBe(1)
    expect(textFrames[0].s).toBe(payload)
  })
})
