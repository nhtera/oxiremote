import type { NavigateFunction } from 'react-router-dom'
import { setActiveHost } from './api-client'
import { useHostStore } from '../state/host-store'

export type SwitchResult =
  | { ok: true }
  | { ok: false; error: 'session-expired' | 'network' | 'mismatch' }

/**
 * Orchestrates an in-place host switch shared by the topbar dropdown, the
 * saved-hosts panel, and the SW push deep-link handler.
 *
 *   1. Point active host pointer at the new host (so the fetch interceptor
 *      rewrites /api/* to that host's tunnel base).
 *   2. Probe /api/me to verify the stored Bearer key is still accepted.
 *   3. fetchHost(hostId) — validates the response host_id matches.
 *   4. Navigate to /h/<hostId>/workspace.
 *
 * On failure the active host pointer is NOT rolled back. Caller decides
 * whether to surface an error and leave the user on their current page.
 */
export async function switchActiveHost(
  hostId: string,
  navigate: NavigateFunction,
): Promise<SwitchResult> {
  setActiveHost(hostId)
  try {
    const res = await fetch('/api/me')
    if (res.status === 401) return { ok: false, error: 'session-expired' }
    if (!res.ok) return { ok: false, error: 'network' }
  } catch {
    return { ok: false, error: 'network' }
  }
  await useHostStore.getState().fetchHost(hostId)
  const { currentHostId } = useHostStore.getState()
  if (currentHostId !== hostId) return { ok: false, error: 'mismatch' }
  navigate(`/h/${hostId}/workspace`, { replace: true })
  return { ok: true }
}
