import { devices, expect, test } from '@playwright/test'

// Windows H.264 + audio E2E smoke. Drives the OTK + desktop flow and asserts
// that an Opus inbound-rtp report appears with bytesReceived > 0 within the
// negotiation budget.
//
// Pre-reqs:
//   - Agent running on http://127.0.0.1:8787 with `--features audio`
//   - auto_approve=true, desktop_audio_enabled=true, OXI_VIDEO_PIPELINE=h264
//
// Skip unless OXI_E2E_AUDIO=1.

const ENABLED = process.env.OXI_E2E_AUDIO === '1'

test.use({ ...devices['Desktop Chrome'] })

test.describe('windows h264 + audio', () => {
  test.skip(!ENABLED, 'Set OXI_E2E_AUDIO=1 with a running audio-enabled agent to run.')

  test('opus inbound-rtp receives bytes within 45s', async ({ page }) => {
    test.setTimeout(120_000)

    page.on('pageerror', (err) => console.log('[pageerror]', err.message))
    page.on('console', (msg) => {
      const t = msg.text()
      if (msg.type() === 'error' || /oxi|audio|h264/i.test(t)) console.log(`[console.${msg.type()}]`, t)
    })

    // Hook setRemoteDescription on the prototype so every RTCPeerConnection
    // instance is captured without wrapping the constructor (which can break
    // `new` semantics on some browsers).
    await page.addInitScript(() => {
      const w = window as unknown as { __oxiPCs: RTCPeerConnection[] }
      w.__oxiPCs = []
      const orig = RTCPeerConnection.prototype.setRemoteDescription
      RTCPeerConnection.prototype.setRemoteDescription = function (
        this: RTCPeerConnection,
        ...args: Parameters<typeof orig>
      ) {
        if (!w.__oxiPCs.includes(this)) w.__oxiPCs.push(this)
        return orig.apply(this, args)
      }
    })

    const otk = await page.context().request.post('/api/agent/keys/one-time')
    expect(otk.ok(), await otk.text()).toBeTruthy()
    const { token } = (await otk.json()) as { token: string }

    await page.goto(`/login?k=${token}`)
    await expect.poll(async () => new URL(page.url()).pathname, { timeout: 20_000 }).toMatch(/^\/h\//)
    const [, , hostId] = new URL(page.url()).pathname.split('/')
    expect(hostId).toBeTruthy()

    await page.evaluate((h) => {
      window.history.pushState({}, '', `/h/${h}/desktop`)
      window.dispatchEvent(new PopStateEvent('popstate'))
    }, hostId)

    // Wait for a PC to exist + finish negotiation.
    await page.waitForFunction(
      () => {
        const w = window as unknown as { __oxiPCs?: RTCPeerConnection[] }
        return (w.__oxiPCs?.length ?? 0) > 0
      },
      undefined,
      { timeout: 20_000, polling: 250 },
    )

    // In-page polling loop. Resolves with the first audio inbound-rtp report
    // that shows bytesReceived > 0, or null at deadline. Also reports the
    // negotiated codec so we can confirm Opus.
    const result = await page.evaluate(async () => {
      type Stat = {
        bytesReceived: number
        packetsReceived: number
        codec: string
        ssrc?: number
      }
      const w = window as unknown as { __oxiPCs: RTCPeerConnection[] }
      const deadline = Date.now() + 45_000
      while (Date.now() < deadline) {
        const pc = w.__oxiPCs[w.__oxiPCs.length - 1]
        if (pc && pc.connectionState !== 'closed') {
          const report = await pc.getStats()
          const codecs = new Map<string, string>()
          report.forEach((s) => {
            const r = s as unknown as { id: string; type: string; mimeType?: string }
            if (r.type === 'codec' && r.mimeType) codecs.set(r.id, r.mimeType)
          })
          let found: Stat | null = null
          report.forEach((s) => {
            const r = s as unknown as {
              type: string
              kind?: string
              bytesReceived?: number
              packetsReceived?: number
              codecId?: string
              ssrc?: number
            }
            if (r.type === 'inbound-rtp' && r.kind === 'audio' && (r.bytesReceived ?? 0) > 0) {
              found = {
                bytesReceived: r.bytesReceived ?? 0,
                packetsReceived: r.packetsReceived ?? 0,
                codec: codecs.get(r.codecId ?? '') ?? '',
                ssrc: r.ssrc,
              }
            }
          })
          if (found) return found
        }
        await new Promise((r) => setTimeout(r, 500))
      }
      // Diagnostics on timeout: dump the last shape of inbound-rtp + the
      // negotiated SDP m-lines so we can see why audio isn't flowing.
      const pc = w.__oxiPCs[w.__oxiPCs.length - 1]
      const summary: Record<string, unknown> = {
        connectionState: pc?.connectionState,
        iceConnectionState: pc?.iceConnectionState,
        signalingState: pc?.signalingState,
      }
      if (pc) {
        const report = await pc.getStats()
        const inbounds: unknown[] = []
        const codecs: unknown[] = []
        report.forEach((s) => {
          const r = s as unknown as { type: string }
          if (r.type === 'inbound-rtp') inbounds.push(r)
          if (r.type === 'codec') codecs.push(r)
        })
        summary.inbounds = inbounds
        summary.codecs = codecs
        summary.remoteDescription = pc.remoteDescription?.sdp?.split('\n').filter((l) => l.startsWith('m=') || l.startsWith('a=rtpmap')).slice(0, 20)
      }
      return { _timeout: true, ...summary }
    })

    console.log('result:', JSON.stringify(result, null, 2))
    expect((result as { _timeout?: boolean })._timeout, 'audio inbound-rtp not seen within deadline').toBeFalsy()
    const s = result as { bytesReceived: number; packetsReceived: number; codec: string }
    expect(s.bytesReceived).toBeGreaterThan(0)
    expect(s.packetsReceived).toBeGreaterThan(0)
    expect(s.codec.toLowerCase()).toContain('opus')
  })
})
