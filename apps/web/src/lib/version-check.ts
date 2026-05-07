/**
 * SPA version-check force-reload (Phase 05 / H14).
 *
 * The discovery-mode SPA on Cloudflare Pages can be cached by the browser
 * indefinitely. When the operator updates the agent (e.g. to drop the legacy
 * `oxi-bearer-v1` WS subprotocol), a stale tab would keep emitting api_keys
 * over the WS upgrade until the user manually hard-reloads.
 *
 * Mitigation: on app mount, fetch `/api/version` from the active host. If it
 * doesn't match the version baked into the bundle (`__APP_VERSION__`, set by
 * `vite.config.ts` from `agent/Cargo.toml`), force a cache-busting reload so
 * the browser pulls the new bundle. One round-trip cost on app open.
 *
 * Same-origin only — discovery-mode call goes to the agent through the
 * existing fetch interceptor (`api-client.ts::installFetchInterceptor`), so
 * the cross-origin tunnel rewrite applies automatically.
 */

declare const __APP_VERSION__: string

const RELOAD_GUARD_KEY = 'oxi:version-check:reloaded-at'
// Only force one reload per minute — protects against a misconfigured agent
// returning a perpetually-mismatched version that would loop the SPA.
const RELOAD_COOLDOWN_MS = 60_000

export const APP_VERSION: string =
  typeof __APP_VERSION__ === 'string' ? __APP_VERSION__ : '0.0.0'

interface VersionResponse {
  version: string
}

/** Fetch `/api/version` (Public, unauthenticated). Returns null on any error
 *  so a transient network failure doesn't trigger a reload loop. */
async function fetchAgentVersion(): Promise<string | null> {
  try {
    const res = await fetch('/api/version', { credentials: 'omit' })
    if (!res.ok) return null
    const body = (await res.json()) as Partial<VersionResponse>
    if (typeof body.version !== 'string' || body.version.length === 0) return null
    return body.version
  } catch {
    return null
  }
}

/** Returns true when a force-reload was scheduled. The caller should bail out
 *  of further setup since the page is about to reload. */
export async function checkAndReloadOnVersionMismatch(): Promise<boolean> {
  // Skip in environments where window/sessionStorage is unavailable (SSR,
  // workers, tests). The reload guarantee is best-effort UX hardening, not a
  // security boundary — the agent already enforces auth on every request.
  if (typeof window === 'undefined' || typeof sessionStorage === 'undefined') {
    return false
  }

  const agentVersion = await fetchAgentVersion()
  if (agentVersion === null) return false
  if (agentVersion === APP_VERSION) return false

  const lastReloadAt = Number(sessionStorage.getItem(RELOAD_GUARD_KEY) ?? '0')
  if (Date.now() - lastReloadAt < RELOAD_COOLDOWN_MS) {
    // We just reloaded and the version still mismatches — likely a stale CDN
    // edge or a misconfigured agent. Stop reloading and let the user see the
    // (potentially broken) UI rather than spin forever.
    return false
  }
  sessionStorage.setItem(RELOAD_GUARD_KEY, String(Date.now()))
  // Standard reload — browsers honour Cache-Control on the navigation. The
  // sessionStorage cooldown above prevents an infinite loop when a stale CDN
  // edge or a misconfigured agent keeps the version mismatched.
  window.location.reload()
  return true
}
