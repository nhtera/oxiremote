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
 *   1. Point active host pointer at the new host AND navigate to its
 *      workspace, in lockstep — ordering matters (see below).
 *   2. Probe /api/me to verify the stored Bearer key is still accepted.
 *   3. fetchHost(hostId) — validates the response host_id matches.
 *
 * Why navigate happens BEFORE the async probes: the alternative (fetch →
 * fetchHost-mutates-store → navigate) leaves a render window where the URL
 * still points at the OLD host but `currentHostId` has flipped to the new
 * one. HostRoute reads that as a mismatch and starts its own concurrent
 * switch back to the URL host — two switches race over `setActiveHost` and
 * the user oscillates. Flipping the pointer + URL synchronously means
 * HostRoute only ever sees a settling state (URL == active pointer); its
 * own effect will dedupe through `fetchHostPromise` in the host store.
 *
 * On failure the active host pointer is NOT rolled back. HostRoute renders
 * the "Host not reachable" / "Session expired" panel for the new URL when
 * it observes `currentHostId` failing to converge.
 */
export async function switchActiveHost(
  hostId: string,
  navigate: NavigateFunction,
): Promise<SwitchResult> {
  setActiveHost(hostId)
  navigate(`/h/${hostId}/workspace`, { replace: true })
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
  return { ok: true }
}
