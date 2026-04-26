import { expect, test } from '@playwright/test'
import { STORAGE_STATE } from './global-setup'

/**
 * Terminal session lifecycle — create → list → rename → close.
 *
 * WebSocket I/O (attach + write input + read output chunks) is intentionally
 * out of scope: the WS contract is well covered by `agent/src/terminal_ws.rs`
 * unit tests, and Playwright's `request` fixture can't carry the session
 * cookie into a `WebSocket` constructor without extra plumbing. The HTTP
 * lifecycle here protects the surface the SPA uses to manage tabs.
 */

type Session = {
  id: string
  name?: string | null
  status: string
}

const PAIRING_CODE = process.env.OXI_PAIRING_CODE

test.describe('terminal session lifecycle', () => {
  test.skip(!PAIRING_CODE, 'OXI_PAIRING_CODE env var not provided')
  test.use({ storageState: STORAGE_STATE })
  test.describe.configure({ mode: 'serial' })

  let sessionId = ''

  test.afterAll(async ({ request }) => {
    if (sessionId) {
      await request.post(`/api/terminal/sessions/${sessionId}/close`).catch(() => {})
    }
  })

  test('create session returns id', async ({ request }) => {
    const res = await request.post('/api/terminal/sessions', {
      data: { name: 'e2e-lifecycle', cols: 80, rows: 24 },
    })
    expect(res.ok(), await res.text()).toBeTruthy()
    const body = (await res.json()) as { id: string }
    expect(typeof body.id).toBe('string')
    expect(body.id.length).toBeGreaterThan(0)
    sessionId = body.id
  })

  test('list shows the new session as running', async ({ request }) => {
    const res = await request.get('/api/terminal/sessions')
    expect(res.ok()).toBeTruthy()
    const body = (await res.json()) as Session[] | { sessions: Session[] }
    const sessions = Array.isArray(body) ? body : body.sessions
    const me = sessions.find((s) => s.id === sessionId)
    expect(me).toBeDefined()
    expect(me!.status).toBe('running')
  })

  test('rename updates name in the list', async ({ request }) => {
    const ren = await request.patch(`/api/terminal/sessions/${sessionId}`, {
      data: { name: 'e2e-renamed' },
    })
    expect(ren.ok(), await ren.text()).toBeTruthy()

    const list = await request.get('/api/terminal/sessions')
    const body = (await list.json()) as Session[] | { sessions: Session[] }
    const sessions = Array.isArray(body) ? body : body.sessions
    const me = sessions.find((s) => s.id === sessionId)
    expect(me?.name).toBe('e2e-renamed')
  })

  test('rename rejects empty / whitespace name with 400', async ({ request }) => {
    const res = await request.patch(`/api/terminal/sessions/${sessionId}`, {
      data: { name: '   ' },
    })
    expect(res.status()).toBe(400)
  })

  test('rename rejects oversize name (>64 chars) with 400', async ({ request }) => {
    const res = await request.patch(`/api/terminal/sessions/${sessionId}`, {
      data: { name: 'x'.repeat(65) },
    })
    expect(res.status()).toBe(400)
  })

  test('close session returns 2xx', async ({ request }) => {
    const res = await request.post(`/api/terminal/sessions/${sessionId}/close`)
    expect(res.ok(), await res.text()).toBeTruthy()
  })

  test('closed session is no longer running', async ({ request }) => {
    const res = await request.get('/api/terminal/sessions')
    const body = (await res.json()) as Session[] | { sessions: Session[] }
    const sessions = Array.isArray(body) ? body : body.sessions
    const me = sessions.find((s) => s.id === sessionId)
    // Either pruned from the list or status flipped off 'running'.
    if (me) expect(me.status).not.toBe('running')
  })
})
