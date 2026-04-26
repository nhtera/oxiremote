import { request, type FullConfig } from '@playwright/test'
import { mkdirSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'

// Shared session bootstrap. Pairing codes are one-time-use, so every spec
// that runs its own `pair → cookie` dance burns a code. We do it once here,
// stash the resulting `oxi_session` cookie in storageState, and downstream
// auth-required tests opt in via `test.use({ storageState: STORAGE_STATE })`.
//
// When OXI_PAIRING_CODE is unset we still write an empty state file so
// `storageState` references a real path; auth specs are expected to
// `test.skip(!process.env.OXI_PAIRING_CODE, ...)` themselves.

export const STORAGE_STATE = join(__dirname, '.auth', 'state.json')

async function globalSetup(config: FullConfig) {
  mkdirSync(dirname(STORAGE_STATE), { recursive: true })

  const PAIRING_CODE = process.env.OXI_PAIRING_CODE
  if (!PAIRING_CODE) {
    writeFileSync(STORAGE_STATE, JSON.stringify({ cookies: [], origins: [] }))
    return
  }

  const baseURL = config.projects[0]?.use?.baseURL ?? 'http://127.0.0.1:8787'
  const ctx = await request.newContext({ baseURL })
  const res = await ctx.post('/api/pairing/exchange', {
    data: { code: PAIRING_CODE, device_label: 'playwright-shared' },
  })
  if (!res.ok()) {
    throw new Error(`global-setup pair failed (${res.status()}): ${await res.text()}`)
  }

  // Body-shape contract — if this regresses, fail-fast before any spec runs.
  const body = (await res.json()) as {
    ok?: boolean
    api_key?: string
    api_key_last4?: string
    device_id?: string
  }
  if (
    body.ok !== true ||
    typeof body.api_key !== 'string' ||
    body.api_key.length <= 16 ||
    !/^[\w-]{4}$/.test(body.api_key_last4 ?? '') ||
    typeof body.device_id !== 'string'
  ) {
    throw new Error(`pair body shape regression: ${JSON.stringify(body)}`)
  }

  await ctx.storageState({ path: STORAGE_STATE })
  await ctx.dispose()
}

export default globalSetup
