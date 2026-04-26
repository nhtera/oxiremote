import { expect, test } from '@playwright/test'
import { STORAGE_STATE } from './global-setup'

/**
 * Files CRUD round-trip — exercises the full file API surface end-to-end.
 *
 * Path layout:
 *   <workspace>/.oxi-e2e-files/                  (test dir)
 *     ├── sample.txt   → renamed to renamed.txt → deleted
 *     └── (cleanup: dir removed in afterAll)
 *
 * The test pollutes the workspace temporarily; `afterAll` cleans up
 * regardless of pass/fail. If a hard crash leaves artifacts, they sit
 * under `.oxi-e2e-files/` so the user can `rm -rf` them safely.
 */

const PAIRING_CODE = process.env.OXI_PAIRING_CODE
const TEST_DIR = '.oxi-e2e-files'
const TEST_FILE = `${TEST_DIR}/sample.txt`
const RENAMED_FILE = `${TEST_DIR}/renamed.txt`
const CONTENT = 'hello e2e\n'

test.describe('files CRUD round-trip', () => {
  test.skip(!PAIRING_CODE, 'OXI_PAIRING_CODE env var not provided')
  test.use({ storageState: STORAGE_STATE })
  test.describe.configure({ mode: 'serial' })

  test.afterAll(async ({ request }) => {
    // Best-effort recursive delete — `/api/files/delete` is recursive on dirs.
    await request.post('/api/files/delete', { data: { path: TEST_DIR } }).catch(() => {})
  })

  test('create directory returns 201', async ({ request }) => {
    const res = await request.post('/api/files/create', {
      data: { path: TEST_DIR, kind: 'dir' },
    })
    expect(res.status(), await res.text()).toBe(201)
  })

  test('create empty file returns 201', async ({ request }) => {
    const res = await request.post('/api/files/create', {
      data: { path: TEST_FILE, kind: 'file' },
    })
    expect(res.status(), await res.text()).toBe(201)
  })

  test('create on existing path returns 409', async ({ request }) => {
    const res = await request.post('/api/files/create', {
      data: { path: TEST_FILE, kind: 'file' },
    })
    expect(res.status()).toBe(409)
  })

  test('write returns size + modified', async ({ request }) => {
    const res = await request.post('/api/files/write', {
      data: { path: TEST_FILE, content: CONTENT },
    })
    expect(res.ok(), await res.text()).toBeTruthy()
    const body = (await res.json()) as { modified: number; size: number }
    expect(body.size).toBe(CONTENT.length)
    expect(body.modified).toBeGreaterThan(0)
  })

  test('read returns the same content that was written', async ({ request }) => {
    const res = await request.get(`/api/files/read?path=${encodeURIComponent(TEST_FILE)}`)
    expect(res.ok()).toBeTruthy()
    const body = (await res.json()) as { content: string; size: number }
    expect(body.content).toBe(CONTENT)
    expect(body.size).toBe(CONTENT.length)
  })

  test('rename moves file; new path readable, old path is 4xx', async ({ request }) => {
    const ren = await request.post('/api/files/rename', {
      data: { from: TEST_FILE, to: RENAMED_FILE },
    })
    expect(ren.ok(), await ren.text()).toBeTruthy()

    const after = await request.get(`/api/files/read?path=${encodeURIComponent(RENAMED_FILE)}`)
    expect(after.ok()).toBeTruthy()
    const body = (await after.json()) as { content: string }
    expect(body.content).toBe(CONTENT)

    const stale = await request.get(`/api/files/read?path=${encodeURIComponent(TEST_FILE)}`)
    expect(stale.status()).toBeGreaterThanOrEqual(400)
  })

  test('delete removes file', async ({ request }) => {
    const del = await request.post('/api/files/delete', { data: { path: RENAMED_FILE } })
    expect(del.ok(), await del.text()).toBeTruthy()

    const gone = await request.get(`/api/files/read?path=${encodeURIComponent(RENAMED_FILE)}`)
    expect(gone.status()).toBeGreaterThanOrEqual(400)
  })

  test('path traversal escape is rejected', async ({ request }) => {
    const res = await request.post('/api/files/write', {
      data: { path: '../escape.txt', content: 'should never land' },
    })
    expect(res.status()).toBeGreaterThanOrEqual(400)
  })
})
