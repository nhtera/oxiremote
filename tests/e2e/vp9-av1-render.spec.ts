// Phase-04+05+06+07 e2e: end-to-end agent-side validation of VP9 + AV1
// pipelines. Mints a session via the localhost OTK API, hits the
// capabilities endpoint to confirm the agent advertises the codec, then
// opens a WebSocket to `/api/hosts/<id>/desktop/ws?force_pipeline=<codec>`
// from a browser page (real WebRTC + MediaStream needed since the agent
// adds a video transceiver) and waits for the `SignalOut::Pipeline { mode }`
// envelope confirming the codec actually got attached.
//
// This bypasses the React SPA — the goal is the codec wire-protocol +
// libvpx/libaom encoder spin-up, not the SPA login UX (covered by
// h264-windows-render.spec.ts elsewhere).
//
// Skip gates:
//   OXI_E2E_VP9=1 — runs the VP9 spec.
//   OXI_E2E_AV1=1 — runs the AV1 spec.

import { devices, expect, test } from '@playwright/test'

const VP9_ENABLED = process.env.OXI_E2E_VP9 === '1'
const AV1_ENABLED = process.env.OXI_E2E_AV1 === '1'

test.use({ ...devices['Desktop Chrome'] })

interface PairedSession {
  hostId: string
  apiKey: string
  deviceId: string
}

/** Mint an OTK via the localhost API, consume it to obtain a session.
 *  Both endpoints are unauthenticated on loopback so we don't need a
 *  storageState or pairing-code dance. */
async function bootstrapSession(
  page: import('@playwright/test').Page,
): Promise<PairedSession> {
  const otk = await page.context().request.post('/api/agent/keys/one-time')
  expect(otk.ok(), `OTK mint failed: ${await otk.text()}`).toBeTruthy()
  const { token } = (await otk.json()) as { token: string }

  const consume = await page.context().request.post('/api/login/one-time', {
    data: { token },
  })
  expect(
    consume.ok(),
    `OTK consume failed (${consume.status()}): ${await consume.text()}`,
  ).toBeTruthy()
  const body = (await consume.json()) as {
    api_key: string
    host_id: string
    device_id: string
  }
  expect(body.api_key.length, 'api_key returned').toBeGreaterThan(16)
  expect(body.host_id).toBeTruthy()
  expect(body.device_id).toBeTruthy()
  return { hostId: body.host_id, apiKey: body.api_key, deviceId: body.device_id }
}

/** Confirm the agent compiled with the given codec by hitting the
 *  capabilities endpoint over loopback. Establishes that the codec
 *  reached `available_pipelines` before we attempt to force it. */
async function expectCodecAdvertised(
  page: import('@playwright/test').Page,
  session: PairedSession,
  codec: 'vp9' | 'av1',
): Promise<void> {
  const caps = await page.context().request.get(
    `/api/hosts/${session.hostId}/desktop/capabilities`,
    { headers: { Authorization: `Bearer ${session.apiKey}` } },
  )
  expect(caps.ok(), `capabilities GET failed: ${await caps.text()}`).toBeTruthy()
  const capsBody = (await caps.json()) as {
    available_pipelines: string[]
    monitors: { id: number }[]
  }
  expect(capsBody.available_pipelines, `binary built with ${codec}`).toContain(codec)
}

/** Open a WS upgrade against the desktop endpoint, complete the SDP
 *  offer/answer dance using the page's RTCPeerConnection, then wait for
 *  the agent's `SignalOut::Pipeline { mode }` envelope to confirm the
 *  forced codec was attached. */
async function expectPipelineSignal(
  page: import('@playwright/test').Page,
  session: PairedSession,
  codec: 'vp9' | 'av1',
  timeoutMs: number,
  audioOptIn: boolean = false,
  hidpi: boolean = false,
): Promise<{ mode: string; reason: string; hardwareAccel?: boolean }> {
  // The desktop WS endpoint authenticates via WS subprotocol header
  // `oxi-bearer-v1` + an extra `oxi-bearer:<api_key>` subprotocol entry.
  // Run inside the page so we use the browser's WebSocket + RTCPeerConnection
  // (the agent expects an SDP offer before it'll emit Pipeline).
  return page.evaluate<{ mode: string; reason: string; hardwareAccel?: boolean }, {
    deviceId: string
    apiKey: string
    codec: string
    timeoutMs: number
    audioOptIn: boolean
    hidpi: boolean
  }>(
    async ({ deviceId, apiKey, codec, timeoutMs, audioOptIn, hidpi }) => {
      const wsUrl =
        `${location.protocol === 'https:' ? 'wss:' : 'ws:'}//${location.host}` +
        `/ws/desktop/${deviceId}?force_pipeline=${codec}`
      // Subprotocol auth per agent/src/auth.rs: server picks the marker
      // ("oxi-bearer-v1") and reads the api_key from the OTHER offered
      // value. Two distinct subprotocol entries.
      const ws = new WebSocket(wsUrl, ['oxi-bearer-v1', apiKey])
      ws.binaryType = 'arraybuffer'
      const pc = new RTCPeerConnection({
        iceServers: [{ urls: 'stun:stun.l.google.com:19302' }],
      })
      // Recvonly video transceiver — same as use-desktop-video-session.ts.
      pc.addTransceiver('video', { direction: 'recvonly' })

      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          ws.close()
          pc.close()
          reject(new Error(`pipeline signal timeout after ${timeoutMs}ms`))
        }, timeoutMs)

        ws.addEventListener('error', (ev) =>
          reject(new Error(`WS error: ${JSON.stringify(ev)}`)),
        )

        ws.addEventListener('open', async () => {
          // Advertise codecs that include the one we're forcing. The agent
          // fails fast on a forced codec the client doesn't advertise.
          const advertised: string[] = ['h264-baseline-3.1']
          if (codec === 'vp9') advertised.push('vp9')
          if (codec === 'av1') advertised.push('av1')
          ws.send(
            JSON.stringify({
              type: 'capabilitiesClient',
              codecs: advertised,
              webcodecs: false,
              audio: audioOptIn,
              loopback: true,
            }),
          )
          // HiDPI is plumbed via a separate `settings` envelope on the signaling
          // WS (mirrors the SPA's pre-offer push in use-desktop-video-session.ts).
          // The agent uses this to size the SCK output BEFORE encoder init; the
          // 1964×3/4=1473 odd-dim regression manifests only when hidpi=true.
          if (hidpi) {
            ws.send(JSON.stringify({ type: 'settings', hidpi: true }))
          }
          try {
            const offer = await pc.createOffer()
            await pc.setLocalDescription(offer)
            ws.send(JSON.stringify({ type: 'offer', sdp: offer.sdp }))
          } catch (e) {
            reject(new Error(`offer build failed: ${(e as Error).message}`))
          }
        })

        ws.addEventListener('message', async (ev) => {
          if (typeof ev.data !== 'string') return
          let msg: Record<string, unknown>
          try {
            msg = JSON.parse(ev.data) as Record<string, unknown>
          } catch {
            return
          }
          if (msg.type === 'answer' && typeof msg.sdp === 'string') {
            try {
              await pc.setRemoteDescription({ type: 'answer', sdp: msg.sdp })
            } catch (e) {
              reject(new Error(`SRD failed: ${(e as Error).message}`))
            }
          } else if (msg.type === 'pipeline' && typeof msg.mode === 'string') {
            clearTimeout(timer)
            ws.close()
            pc.close()
            resolve({
              mode: msg.mode as string,
              reason: typeof msg.reason === 'string' ? msg.reason : 'unknown',
              hardwareAccel:
                typeof msg.hardware_accel === 'boolean'
                  ? (msg.hardware_accel as boolean)
                  : undefined,
            })
          } else if (msg.type === 'fallbackPending') {
            // Agent's pre-negotiation SDP validation detected the chosen
            // codec is missing from the browser's offer m=video rtpmap.
            // Surface as a pseudo-mode 'fallback' so tests can assert
            // graceful exit instead of the prior `ErrUnsupportedCodec` crash.
            clearTimeout(timer)
            ws.close()
            pc.close()
            resolve({
              mode: 'fallback',
              reason: typeof msg.reason === 'string' ? msg.reason : 'unknown',
            })
          }
        })
      })
    },
    {
      deviceId: session.deviceId,
      apiKey: session.apiKey,
      codec,
      timeoutMs,
      audioOptIn,
      hidpi,
    },
  )
}

test.describe('vp9 pipeline negotiates and emits SignalOut::Pipeline', () => {
  test.skip(!VP9_ENABLED, 'OXI_E2E_VP9=1 required (agent built with vp9 feature)')

  test('forced VP9 → mode=vp9 reason=forced-vp9', async ({ page }) => {
    test.setTimeout(90_000)
    // Land on a same-origin page so the WS URL resolves to the agent.
    await page.goto('/')
    const session = await bootstrapSession(page)
    await expectCodecAdvertised(page, session, 'vp9')

    const sig = await expectPipelineSignal(page, session, 'vp9', 60_000)
    console.log('VP9 signal:', JSON.stringify(sig))
    expect(sig.mode).toBe('vp9')
    expect(sig.reason).toMatch(/^(forced-vp9|auto-vp9)$/)
    expect(sig.hardwareAccel).toBe(false) // libvpx is software-only.
  })
})

test.describe('av1 pipeline negotiates and emits SignalOut::Pipeline', () => {
  test.skip(!AV1_ENABLED, 'OXI_E2E_AV1=1 required (agent built with av1 feature)')

  test('forced AV1 → mode=av1 reason=forced-av1', async ({ page }) => {
    test.setTimeout(120_000)
    await page.goto('/')
    const session = await bootstrapSession(page)
    await expectCodecAdvertised(page, session, 'av1')

    // libaom cpu_used=11 cold-start is slower than libvpx — give it 90 s.
    const sig = await expectPipelineSignal(page, session, 'av1', 90_000)
    console.log('AV1 signal:', JSON.stringify(sig))
    expect(sig.mode).toBe('av1')
    expect(sig.reason).toMatch(/^(forced-av1|auto-av1)$/)
    expect(sig.hardwareAccel).toBe(false) // libaom is software-only.
  })
})

// HiDPI regression (260515): on a 2× retina monitor at Med tier, resize_dims
// used to return 2268×1473 — odd height — and libvpx/libaom rejected init
// with `dims must be even`. The agent emitted Pipeline (because force-reason
// arrives before encoder build) but no frames ever flowed, leaving the SPA
// canvas black. This test forces hidpi=true on session-start and asserts
// the pipeline signal still arrives, which can only happen if the encoder
// built successfully (i.e. resize_dims clamped to even).
test.describe('HiDPI session yields even encoder dims (regression 260515)', () => {
  test.skip(
    !VP9_ENABLED && !AV1_ENABLED,
    'OXI_E2E_VP9=1 or OXI_E2E_AV1=1 required',
  )

  test('VP9 + hidpi:true → pipeline signal arrives', async ({ page }) => {
    test.skip(!VP9_ENABLED, 'OXI_E2E_VP9=1 required')
    test.setTimeout(90_000)
    await page.goto('/')
    const session = await bootstrapSession(page)
    await expectCodecAdvertised(page, session, 'vp9')
    const sig = await expectPipelineSignal(page, session, 'vp9', 60_000, false, true)
    console.log('VP9+hidpi signal:', JSON.stringify(sig))
    expect(sig.mode).toBe('vp9')
  })

  test('AV1 + hidpi:true → pipeline signal arrives', async ({ page }) => {
    test.skip(!AV1_ENABLED, 'OXI_E2E_AV1=1 required')
    test.setTimeout(120_000)
    await page.goto('/')
    const session = await bootstrapSession(page)
    await expectCodecAdvertised(page, session, 'av1')
    const sig = await expectPipelineSignal(page, session, 'av1', 90_000, false, true)
    console.log('AV1+hidpi signal:', JSON.stringify(sig))
    expect(sig.mode).toBe('av1')
  })
})

// Audio-parity smoke: opt the client into audio on a VP9 session and prove
// the agent (a) accepts the audio capability and (b) reaches the pipeline
// signal without erroring out. The audio path used to be H.264-only; this
// test fails if the audio gate widening regresses. Skip with
// OXI_E2E_AUDIO_VP9=1 because it requires the operator audio setting on the
// agent to be enabled (the launch script does this) — left opt-in so a
// no-audio agent doesn't false-fail.
const AUDIO_VP9_ENABLED = process.env.OXI_E2E_AUDIO_VP9 === '1'
test.describe('audio gate opens on VP9 (parity with H.264)', () => {
  test.skip(
    !AUDIO_VP9_ENABLED,
    'OXI_E2E_AUDIO_VP9=1 required + agent built with audio feature + operator audio setting on',
  )

  test('VP9 + audio:true client cap → pipeline signal still arrives', async ({
    page,
  }) => {
    test.setTimeout(90_000)
    await page.goto('/')
    const session = await bootstrapSession(page)
    await expectCodecAdvertised(page, session, 'vp9')

    // audioOptIn=true: agent's audio gate AND-merges (operator setting +
    // probe + this client flag); if widening regressed the agent would
    // either error the WS or never emit the pipeline signal.
    const sig = await expectPipelineSignal(page, session, 'vp9', 60_000, true)
    console.log('VP9+audio signal:', JSON.stringify(sig))
    expect(sig.mode).toBe('vp9')
  })
})
