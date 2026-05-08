import type { NavigateFunction } from 'react-router-dom'
import { useHostStore } from '../state/host-store'
import {
  loadApiKey,
  makeRemoteClient,
  storeApiKey,
  storeDiscoveryLookupId,
  storeTunnelBase,
} from './api-client'
import {
  isDiscoveryMode,
  isLikelyOtk,
  isLikelyPairingCode,
  lookupAny,
  lookupPermanentKey,
  lookupSession,
} from './discovery-client'
import { recordSavedHost } from './saved-hosts'
import { isPermanentKey, sanitizeAccessKey } from '../components/login/access-key-form'

// Pair-flow helpers extracted from login-page.tsx. All functions are pure
// async — error / loading state lives in the caller. Discovery-mode flows
// hit the cross-origin tunnel directly (the SPA on Pages can't use
// same-origin /api/*); same-origin flows ride existing cookies.

interface PairCtx {
  deviceLabel: string
  navigate: NavigateFunction
}

// Same-origin OTK success: cookie auth, no api_key in body.
export async function handleOtkSuccess(res: Response, ctx: PairCtx): Promise<void> {
  if (res.status === 202) {
    const body = await res.json()
    ctx.navigate('/approval-waiting', {
      state: {
        session_id: body.session_id,
        device_label: ctx.deviceLabel.trim() || undefined,
      },
    })
    return
  }
  if (ctx.deviceLabel.trim()) {
    window.localStorage.setItem('oxi:device-label', ctx.deviceLabel.trim())
  }
  await useHostStore.getState().fetchHost()
  const hostState = useHostStore.getState()
  if (hostState.currentHostId) {
    const existingKey = loadApiKey(hostState.currentHostId) ?? ''
    recordSavedHost({
      host_id: hostState.currentHostId,
      label: hostState.label ?? ctx.deviceLabel.trim() ?? hostState.currentHostId.slice(0, 8),
      api_key_last4: existingKey.slice(-4),
    })
    // Persist the current page origin as this host's tunnel base so future
    // cross-origin switches from another host's SPA can route here.
    if (typeof window !== 'undefined') {
      storeTunnelBase(hostState.currentHostId, window.location.origin)
    }
  }
  ctx.navigate('/')
}

// Same-origin pairing-code success: api_key + device_id returned. Persists
// the key so subsequent tunnel requests can Bearer-auth.
//
// Pending-state subtlety: while approval_status === 'pending' the server
// rejects Bearer auth (verify_api_key SQL filters non-approved devices),
// so /api/host returns 401 → fetchHost has no host_id → we cannot key
// localStorage by hostId yet. Stash in sessionStorage under device_id;
// ApprovalWaitingPage promotes on approval observation.
export async function handlePairingSuccess(res: Response, ctx: PairCtx): Promise<void> {
  const body = await res.json().catch(() => ({}))

  if (ctx.deviceLabel.trim()) {
    window.localStorage.setItem('oxi:device-label', ctx.deviceLabel.trim())
  }
  try {
    await useHostStore.getState().fetchHost()
    const hostState = useHostStore.getState()
    const hostId = hostState.currentHostId
    if (body.api_key && hostId) {
      storeApiKey(hostId, body.api_key)
      recordSavedHost({
        host_id: hostId,
        label: hostState.label ?? ctx.deviceLabel.trim() ?? hostId.slice(0, 8),
        api_key_last4: String(body.api_key).slice(-4),
      })
      // Persist the current page origin as this host's tunnel base so future
      // cross-origin switches from another host's SPA can route here.
      if (typeof window !== 'undefined') {
        storeTunnelBase(hostId, window.location.origin)
      }
    } else if (body.api_key && body.device_id) {
      try {
        window.sessionStorage.setItem(
          `oxi_pending_api_key_${body.device_id}`,
          body.api_key,
        )
      } catch { /* storage quota / private mode — degrade gracefully */ }
    }
  } catch {
    await useHostStore.getState().fetchHost()
  }

  if (body.approval_status === 'pending') {
    ctx.navigate('/approval-waiting', {
      state: {
        device_id: body.device_id,
        device_label: ctx.deviceLabel.trim() || undefined,
      },
    })
    return
  }

  ctx.navigate('/')
}

// Cross-origin pairing through the discovery worker (QR deep-link path).
// QR encodes <discoveryUrl>/login?k=<tempKey>&otk=<otkToken>; tempKey
// resolves to the agent's tunnel URL via the worker, then the OTK is
// exchanged directly with the agent's /api/login/one-time endpoint.
// No cookies (different origin); CSRF is exempt for this endpoint.
export async function submitDiscoveryPair(
  tempKey: string,
  otkToken: string,
  ctx: PairCtx,
): Promise<void> {
  const session = await lookupSession(tempKey)
  if (!session) {
    throw new Error('Discovery key expired or unknown — generate a fresh QR.')
  }
  const res = await fetch(`${session.tunnelUrl}/api/login/one-time`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    credentials: 'omit',
    body: JSON.stringify({ token: otkToken.toLowerCase() }),
  })
  if (res.status !== 200 && res.status !== 202) {
    throw new Error('That key is invalid or expired.')
  }
  const body = (await res.json().catch(() => ({}))) as DiscoveryPairBody
  if (!body.api_key || !body.host_id) {
    throw new Error('Pairing succeeded but response is missing key/host id.')
  }

  if (ctx.deviceLabel.trim()) {
    window.localStorage.setItem('oxi:device-label', ctx.deviceLabel.trim())
  }
  storeApiKey(body.host_id, body.api_key)
  storeTunnelBase(body.host_id, session.tunnelUrl)
  if (body.discovery_id) storeDiscoveryLookupId(body.host_id, body.discovery_id)
  // Prefer the agent-supplied hostname (DESKTOP-…, TienNHs-MBP.lan) over
  // the user-typed device label and the truncated host_id. Older agents
  // omit `label`; we keep the same fallbacks to stay backward-compatible.
  const friendlyLabel =
    body.label?.trim() || ctx.deviceLabel.trim() || body.host_id.slice(0, 8)
  recordSavedHost({
    host_id: body.host_id,
    label: friendlyLabel,
    api_key_last4: body.api_key_last4 ?? body.api_key.slice(-4),
  })
  // RootRoute ('/') redirects to /welcome when currentHostId is null. In
  // discovery mode fetchHost is a no-op, so seed the store directly from
  // the pair response — otherwise the SPA bounces back to /welcome.
  useHostStore.setState({
    currentHostId: body.host_id,
    label: body.label?.trim() || ctx.deviceLabel.trim() || null,
    platform: body.platform ?? null,
    loading: false,
    error: null,
  })

  const remote = makeRemoteClient(session.tunnelUrl, body.api_key)
  const tunnelBase = remote.baseUrl

  if (body.status === 'pending' || res.status === 202) {
    ctx.navigate('/approval-waiting', {
      state: {
        session_id: body.session_id,
        device_id: body.device_id,
        device_label: ctx.deviceLabel.trim() || undefined,
        tunnel_base: tunnelBase,
      },
    })
    return
  }
  ctx.navigate('/', { state: { tunnel_base: tunnelBase } })
}

// Cross-origin permanent-key (sk-…) pair. Hashes the plaintext to a 16-hex
// SHA-256 prefix, resolves the agent's tunnel via the discovery worker
// (worker holds only the lookup_id, never the plaintext), then POSTs the
// permanent_key to /api/pairing/exchange. CSRF is exempt for cross-origin
// pairing; auth happens at the agent via verify_permanent_key.
async function submitDiscoveryPermanent(rawKey: string, ctx: PairCtx): Promise<void> {
  const trimmedKey = rawKey.trim()
  const session = await lookupPermanentKey(trimmedKey).catch(() => null)
  if (!session) {
    throw new Error(
      'That permanent key is unknown or the host is offline. Generate a new one from the host dashboard, or check that the agent is running.',
    )
  }
  const res = await fetch(`${session.tunnelUrl}/api/pairing/exchange`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    credentials: 'omit',
    body: JSON.stringify({
      permanent_key: trimmedKey,
      device_label: ctx.deviceLabel.trim() || undefined,
    }),
  })
  if (!res.ok) {
    throw new Error('Permanent key is invalid or has not been generated yet.')
  }
  const body = (await res.json().catch(() => ({}))) as DiscoveryPairBody
  if (!body.api_key || !body.host_id) {
    throw new Error('Pairing succeeded but response is missing key/host id.')
  }
  if (ctx.deviceLabel.trim()) {
    window.localStorage.setItem('oxi:device-label', ctx.deviceLabel.trim())
  }
  storeApiKey(body.host_id, body.api_key)
  storeTunnelBase(body.host_id, session.tunnelUrl)
  if (body.discovery_id) storeDiscoveryLookupId(body.host_id, body.discovery_id)
  const friendlyLabel =
    body.label?.trim() || ctx.deviceLabel.trim() || body.host_id.slice(0, 8)
  recordSavedHost({
    host_id: body.host_id,
    label: friendlyLabel,
    api_key_last4: body.api_key_last4 ?? body.api_key.slice(-4),
  })
  useHostStore.setState({
    currentHostId: body.host_id,
    label: body.label?.trim() || ctx.deviceLabel.trim() || null,
    platform: body.platform ?? null,
    loading: false,
    error: null,
  })
  if (body.approval_status === 'pending') {
    ctx.navigate('/approval-waiting', {
      state: {
        device_id: body.device_id,
        device_label: ctx.deviceLabel.trim() || undefined,
        tunnel_base: session.tunnelUrl,
      },
    })
    return
  }
  ctx.navigate('/', { state: { tunnel_base: session.tunnelUrl } })
}

// Cross-origin manual pair via the discovery worker. Mirrors the QR flow
// but uses a user-typed code instead of the temp_key embedded in a QR:
//   1. SPA → worker: GET /api/session/lookup?k=<code>  → tunnelUrl
//   2. SPA → tunnel: POST /api/{login/one-time | pairing/exchange} { … }
// Shape decides the agent endpoint (sk-…, OTK, or pairing code).
async function submitDiscoveryCode(raw: string, ctx: PairCtx): Promise<void> {
  const trimmed = raw.trim()
  if (isPermanentKey(trimmed)) {
    return submitDiscoveryPermanent(trimmed, ctx)
  }
  const cleaned = sanitizeAccessKey(raw)
  const otkAttempt = cleaned.toLowerCase()
  const codeAttempt = cleaned.toUpperCase()
  let kind: 'otk' | 'code'
  let lookupKey: string
  if (isLikelyOtk(otkAttempt)) {
    kind = 'otk'
    lookupKey = otkAttempt
  } else if (isLikelyPairingCode(codeAttempt)) {
    kind = 'code'
    lookupKey = codeAttempt
  } else {
    throw new Error(
      'Enter a permanent key (sk-…), one-time key (16 chars), or pairing code (8 chars) shown on your computer.',
    )
  }
  const session = await lookupAny(lookupKey)
  if (!session) {
    throw new Error('That code is unknown or expired. Generate a fresh one on your computer.')
  }

  const endpoint = kind === 'otk' ? '/api/login/one-time' : '/api/pairing/exchange'
  const reqBody = kind === 'otk'
    ? { token: lookupKey }
    : { code: lookupKey, device_label: ctx.deviceLabel.trim() || undefined }
  const res = await fetch(`${session.tunnelUrl}${endpoint}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    credentials: 'omit',
    body: JSON.stringify(reqBody),
  })
  if (kind === 'otk') {
    if (res.status !== 200 && res.status !== 202) {
      throw new Error('That key is invalid or expired.')
    }
  } else if (!res.ok) {
    throw new Error('That code is invalid or already used.')
  }
  const body = (await res.json().catch(() => ({}))) as DiscoveryPairBody
  if (!body.api_key || !body.host_id) {
    throw new Error('Pairing succeeded but response is missing key/host id.')
  }
  if (ctx.deviceLabel.trim()) {
    window.localStorage.setItem('oxi:device-label', ctx.deviceLabel.trim())
  }
  storeApiKey(body.host_id, body.api_key)
  storeTunnelBase(body.host_id, session.tunnelUrl)
  if (body.discovery_id) storeDiscoveryLookupId(body.host_id, body.discovery_id)
  const friendlyLabel =
    body.label?.trim() || ctx.deviceLabel.trim() || body.host_id.slice(0, 8)
  recordSavedHost({
    host_id: body.host_id,
    label: friendlyLabel,
    api_key_last4: body.api_key_last4 ?? body.api_key.slice(-4),
  })
  useHostStore.setState({
    currentHostId: body.host_id,
    label: body.label?.trim() || ctx.deviceLabel.trim() || null,
    platform: body.platform ?? null,
    loading: false,
    error: null,
  })
  if (body.approval_status === 'pending' || body.status === 'pending' || res.status === 202) {
    ctx.navigate('/approval-waiting', {
      state: {
        session_id: body.session_id,
        device_id: body.device_id,
        device_label: ctx.deviceLabel.trim() || undefined,
        tunnel_base: session.tunnelUrl,
      },
    })
    return
  }
  ctx.navigate('/', { state: { tunnel_base: session.tunnelUrl } })
}

// Single submit path. Auto-detects by `sk-` prefix:
//   - permanent key   → /api/pairing/exchange { permanent_key }
//   - everything else → try /api/login/one-time, fall back to /api/pairing/exchange { code }
export async function submitAccessKey(raw: string, ctx: PairCtx): Promise<void> {
  if (isDiscoveryMode()) {
    return submitDiscoveryCode(raw, ctx)
  }
  if (isPermanentKey(raw)) {
    const res = await fetch('/api/pairing/exchange', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        permanent_key: raw.trim(),
        device_label: ctx.deviceLabel.trim() || undefined,
      }),
    })
    if (!res.ok) {
      throw new Error('Permanent key is invalid or has not been generated yet.')
    }
    return handlePairingSuccess(res, ctx)
  }
  const cleaned = sanitizeAccessKey(raw)
  const otkAttempt = cleaned.toLowerCase()
  const otkRes = await fetch('/api/login/one-time', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    credentials: 'include',
    body: JSON.stringify({ token: otkAttempt }),
  })
  if (otkRes.status === 200 || otkRes.status === 202) {
    return handleOtkSuccess(otkRes, ctx)
  }
  const codeAttempt = cleaned.toUpperCase()
  const codeRes = await fetch('/api/pairing/exchange', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      code: codeAttempt,
      device_label: ctx.deviceLabel.trim() || undefined,
    }),
  })
  if (!codeRes.ok) {
    throw new Error('That key is invalid or expired.')
  }
  return handlePairingSuccess(codeRes, ctx)
}

interface DiscoveryPairBody {
  status?: string
  approval_status?: string
  api_key?: string
  api_key_last4?: string
  device_id?: string
  host_id?: string
  session_id?: string
  // Friendly hostname (e.g. "DESKTOP-VE3J42Q", "TienNHs-MBP.lan") and OS
  // platform string. Surfaced by the agent so cross-origin discovery
  // SPAs can label saved hosts without a follow-up /api/host call. Both
  // are optional for backward compatibility with pre-0.1.15 agents.
  label?: string | null
  platform?: string | null
  // Stable per-host worker lookup id (agent 0.1.28+). Lets the SPA re-resolve
  // the current tunnel URL via the discovery worker after a Quick Tunnel
  // rotation. Older agents omit it; callers must treat undefined as "no
  // refresh path — fall back to the cached tunnel base".
  discovery_id?: string | null
}
