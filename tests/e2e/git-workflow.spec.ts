import { expect, test } from '@playwright/test'
import { STORAGE_STATE } from './global-setup'

/**
 * Git API smoke — read-only by design.
 *
 * Stage / unstage / commit are intentionally NOT exercised here: the agent
 * runs against the user's actual workspace, and a flaky test that stages
 * a synthetic file would leave artifacts in their working tree on failure.
 * The mutating endpoints have unit-level coverage in `agent/src/git.rs`.
 *
 * What this guards:
 *   - `/api/git/status` returns the documented `[{path, index, working}]`
 *     shape (one-char status fields).
 *   - `/api/git/diff` accepts `staged=0|1` and returns a string body.
 *   - Path-traversal in `?path=` is rejected with 400 before git runs.
 */

type StatusEntry = { path: string; index: string; working: string }

const PAIRING_CODE = process.env.OXI_PAIRING_CODE

test.describe('git read-only API smoke', () => {
  test.skip(!PAIRING_CODE, 'OXI_PAIRING_CODE env var not provided')
  test.use({ storageState: STORAGE_STATE })

  test('status returns array of {path, index, working}', async ({ request }) => {
    const res = await request.get('/api/git/status')
    expect(res.ok(), await res.text()).toBeTruthy()
    const entries = (await res.json()) as StatusEntry[]
    expect(Array.isArray(entries)).toBeTruthy()
    for (const e of entries) {
      expect(typeof e.path).toBe('string')
      expect(e.index.length).toBe(1)
      expect(e.working.length).toBe(1)
    }
  })

  test('diff (working tree) returns text body', async ({ request }) => {
    const res = await request.get('/api/git/diff')
    expect(res.ok(), await res.text()).toBeTruthy()
    // Empty string is a valid response when the tree is clean.
    expect(typeof (await res.text())).toBe('string')
  })

  test('diff (staged) returns text body', async ({ request }) => {
    const res = await request.get('/api/git/diff?staged=1')
    expect(res.ok()).toBeTruthy()
    expect(typeof (await res.text())).toBe('string')
  })

  test('diff with path-traversal payload is rejected with 400', async ({ request }) => {
    const res = await request.get('/api/git/diff?path=../escape')
    expect(res.status()).toBe(400)
  })
})
