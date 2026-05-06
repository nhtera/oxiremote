// Idempotent mkdir of `.attachments/` inside a workspace. Used by
// paste/drop/desktop-attach flows so screenshots stay tidy instead of
// cluttering the workspace root.
//
// The agent's `/api/files/create` returns 201 on success and 409 if the
// directory already exists; both are treated as success and cached per
// (host_id, ws_id) so we only hit the network once per session.
//
// On any other error we surface false and let the caller fall back to
// uploading into the workspace root — better to clutter than to drop the
// upload entirely.

import { getActiveHost } from './api-client'

export const ATTACHMENTS_DIR = '.attachments'

const cache = new Set<string>()

function cacheKey(wsId: number): string {
  return `${getActiveHost() ?? 'local'}|${wsId}`
}

export async function ensureAttachmentsDir(wsId: number): Promise<boolean> {
  const key = cacheKey(wsId)
  if (cache.has(key)) return true
  try {
    const res = await fetch('/api/files/create', {
      method: 'POST',
      credentials: 'include',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path: ATTACHMENTS_DIR, kind: 'dir', ws_id: wsId }),
    })
    if (res.ok || res.status === 409) {
      cache.add(key)
      return true
    }
    return false
  } catch {
    return false
  }
}
