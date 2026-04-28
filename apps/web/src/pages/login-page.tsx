import { useEffect, useState } from 'react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { useHostStore } from '../state/host-store'
import { loadApiKey, makeRemoteClient, storeApiKey, storeTunnelBase } from '../lib/api-client'
import {
  isDiscoveryMode,
  isLikelyOtk,
  isLikelyPairingCode,
  isLikelyTempKey,
  lookupAny,
  lookupSession,
} from '../lib/discovery-client'
import {
  formatRelative,
  listSavedHosts,
  recordSavedHost,
  removeSavedHost,
  type SavedHost,
} from '../lib/saved-hosts'
import AccessKeyForm, { isPermanentKey, sanitizeAccessKey } from '../components/login/access-key-form'
import DeviceLabelField from '../components/login/device-label-field'

// Pairing entry point. A single input accepts OTK (16-hex), pairing codes
// (6-16 alnum), or permanent keys (sk-…). The submit path auto-detects by
// `sk-` prefix. Deep-link `?k=` auto-submits without showing the form.

export default function LoginPage() {
  const [searchParams] = useSearchParams()
  const navigate = useNavigate()

  const [accessKey, setAccessKey] = useState('')
  const [deviceLabel, setDeviceLabel] = useState(
    typeof window !== 'undefined'
      ? window.localStorage.getItem('oxi:device-label') ?? ''
      : '',
  )
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(false)
  const [checkingAuth, setCheckingAuth] = useState(true)
  // Per-host reconnect spinner state: stores host_id of the card being reconnected.
  const [reconnectingId, setReconnectingId] = useState<string | null>(null)
  // Snapshot the saved-hosts list once on mount — adding a new pair after
  // the form is open shouldn't reorder the panel under the user's finger.
  const [savedHosts, setSavedHosts] = useState<SavedHost[]>(() => listSavedHosts())

  const rejectedError =
    searchParams.get('error') === 'rejected'
      ? 'The host declined this device. You can try again with a fresh key.'
      : null

  // Skip the form entirely if the session cookie is already valid. In
  // discovery mode the SPA's own origin (Pages) has no /api/me — the agent
  // is on a separate tunnel origin and we don't know it pre-pair, so probing
  // here would just hit the SPA fallback. Skip the probe entirely when the
  // build is configured for discovery; the QR-deep-link path takes over.
  useEffect(() => {
    if (isDiscoveryMode()) {
      setCheckingAuth(false)
      return
    }
    let cancelled = false
    fetch('/api/me', { credentials: 'include' })
      .then(async (res) => {
        if (cancelled) return
        if (res.ok) {
          await useHostStore.getState().fetchHost()
          navigate('/', { replace: true })
        } else {
          setCheckingAuth(false)
        }
      })
      .catch(() => {
        if (!cancelled) setCheckingAuth(false)
      })
    return () => {
      cancelled = true
    }
  }, [navigate])

  // Success handler for OTK path (cookie-based session, no api_key in body).
  const handleOtkSuccess = async (res: Response) => {
    if (res.status === 202) {
      const body = await res.json()
      navigate('/approval-waiting', {
        state: {
          session_id: body.session_id,
          device_label: deviceLabel.trim() || undefined,
        },
      })
      return
    }
    // status 200
    if (deviceLabel.trim()) {
      window.localStorage.setItem('oxi:device-label', deviceLabel.trim())
    }
    await useHostStore.getState().fetchHost()
    const hostState = useHostStore.getState()
    if (hostState.currentHostId) {
      const existingKey = loadApiKey(hostState.currentHostId) ?? ''
      recordSavedHost({
        host_id: hostState.currentHostId,
        label: hostState.label ?? deviceLabel.trim() ?? hostState.currentHostId.slice(0, 8),
        api_key_last4: existingKey.slice(-4),
      })
    }
    navigate('/')
  }

  // Success handler for pairing-code path (returns api_key + device_id).
  const handlePairingSuccess = async (res: Response) => {
    const body = await res.json().catch(() => ({}))

    if (deviceLabel.trim()) {
      window.localStorage.setItem('oxi:device-label', deviceLabel.trim())
    }
    // Persist API key BEFORE any navigation. The pending-approval path also
    // needs the key in localStorage — once the operator approves and the
    // user lands on the workspace, every tunnel request needs the
    // Authorization: Bearer header. Cookie auth alone covers loopback
    // but not the tunnel.
    //
    // Pending-state subtlety: while approval_status === 'pending' the server
    // rejects Bearer auth (verify_api_key SQL filters non-approved devices),
    // so /api/host returns 401 → fetchHost has no host_id → we cannot key
    // localStorage by hostId yet. Stash the api_key in sessionStorage under
    // device_id; ApprovalWaitingPage promotes it on approval observation.
    try {
      await useHostStore.getState().fetchHost()
      const hostState = useHostStore.getState()
      const hostId = hostState.currentHostId
      if (body.api_key && hostId) {
        storeApiKey(hostId, body.api_key)
        recordSavedHost({
          host_id: hostId,
          label: hostState.label ?? deviceLabel.trim() ?? hostId.slice(0, 8),
          api_key_last4: String(body.api_key).slice(-4),
        })
      } else if (body.api_key && body.device_id) {
        // Pending path: hostId not yet known. Stash for promotion below.
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
      navigate('/approval-waiting', {
        state: {
          device_id: body.device_id,
          device_label: deviceLabel.trim() || undefined,
        },
      })
      return
    }

    navigate('/')
  }

  // Cross-origin pairing through the discovery worker. The QR encoded
  //   <discoveryUrl>/login?k=<tempKey>&otk=<otkToken>
  // — `tempKey` resolves to the agent's tunnel URL via the worker; `otk` is
  // exchanged directly with the agent's `/api/login/one-time` endpoint and
  // returns the per-device api_key + host_id we need to bootstrap Bearer auth.
  // No cookies are used (different origin); CSRF is exempt for this endpoint.
  const submitDiscoveryPair = async (tempKey: string, otkToken: string) => {
    setError('')
    setLoading(true)
    try {
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
      const body = (await res.json().catch(() => ({}))) as {
        status?: string
        api_key?: string
        api_key_last4?: string
        device_id?: string
        host_id?: string
        session_id?: string
      }
      if (!body.api_key || !body.host_id) {
        throw new Error('Pairing succeeded but response is missing key/host id.')
      }

      if (deviceLabel.trim()) {
        window.localStorage.setItem('oxi:device-label', deviceLabel.trim())
      }

      // Stash before navigation — every subsequent tunnel request needs
      // Bearer; cross-origin can't use cookies.
      storeApiKey(body.host_id, body.api_key)
      storeTunnelBase(body.host_id, session.tunnelUrl)
      recordSavedHost({
        host_id: body.host_id,
        label: deviceLabel.trim() || body.host_id.slice(0, 8),
        api_key_last4: body.api_key_last4 ?? body.api_key.slice(-4),
      })
      // RootRoute ('/') redirects to /welcome when currentHostId is null.
      // In discovery mode fetchHost is a no-op (Phase 4.5), so seed the
      // store directly from the pair response — otherwise the SPA bounces
      // back to /welcome despite a successful pair.
      useHostStore.setState({
        currentHostId: body.host_id,
        label: deviceLabel.trim() || null,
        platform: null,
        loading: false,
        error: null,
      })

      // Bind a `RemoteClient` to the tunnel URL so /approval-waiting + the
      // workspace can issue cross-origin Bearer requests once Phase 4 lands.
      // For now we pass the resolved tunnel URL via location state so future
      // pages can pick it up without re-resolving the temp key.
      const remote = makeRemoteClient(session.tunnelUrl, body.api_key)
      const tunnelBase = remote.baseUrl

      if (body.status === 'pending' || res.status === 202) {
        navigate('/approval-waiting', {
          state: {
            session_id: body.session_id,
            device_id: body.device_id,
            device_label: deviceLabel.trim() || undefined,
            tunnel_base: tunnelBase,
          },
        })
        return
      }

      navigate('/', { state: { tunnel_base: tunnelBase } })
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Could not pair via discovery.')
    } finally {
      setLoading(false)
    }
  }

  // Cross-origin manual pair via the discovery worker. Mirrors the QR flow
  // but uses a user-typed code instead of the temp_key embedded in a QR:
  //   1. SPA → worker: GET /api/session/lookup?k=<code>  → tunnelUrl
  //   2. SPA → tunnel: POST /api/{login/one-time | pairing/exchange} { … }
  // Shape decides the agent endpoint:
  //   - 16 lowercase base32 → OTK   → /api/login/one-time { token }
  //   - 8+ uppercase alnum  → code  → /api/pairing/exchange { code }
  // Both kinds are registered with the worker by the agent on issuance.
  const submitDiscoveryCode = async (raw: string) => {
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
        'Enter a one-time key (16 chars) or pairing code (8 chars) shown on your computer.',
      )
    }
    const session = await lookupAny(lookupKey)
    if (!session) {
      throw new Error('That code is unknown or expired. Generate a fresh one on your computer.')
    }

    if (kind === 'otk') {
      const res = await fetch(`${session.tunnelUrl}/api/login/one-time`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        credentials: 'omit',
        body: JSON.stringify({ token: lookupKey }),
      })
      if (res.status !== 200 && res.status !== 202) {
        throw new Error('That key is invalid or expired.')
      }
      const body = (await res.json().catch(() => ({}))) as {
        api_key?: string
        api_key_last4?: string
        device_id?: string
        host_id?: string
        session_id?: string
        status?: string
      }
      if (!body.api_key || !body.host_id) {
        throw new Error('Pairing succeeded but response is missing key/host id.')
      }
      if (deviceLabel.trim()) {
        window.localStorage.setItem('oxi:device-label', deviceLabel.trim())
      }
      storeApiKey(body.host_id, body.api_key)
      storeTunnelBase(body.host_id, session.tunnelUrl)
      recordSavedHost({
        host_id: body.host_id,
        label: deviceLabel.trim() || body.host_id.slice(0, 8),
        api_key_last4: body.api_key_last4 ?? body.api_key.slice(-4),
      })
      // RootRoute ('/') redirects to /welcome when currentHostId is null.
      // In discovery mode fetchHost is a no-op (Phase 4.5), so seed the
      // store directly from the pair response — otherwise the SPA bounces
      // back to /welcome despite a successful pair.
      useHostStore.setState({
        currentHostId: body.host_id,
        label: deviceLabel.trim() || null,
        platform: null,
        loading: false,
        error: null,
      })
      if (res.status === 202 || body.status === 'pending') {
        navigate('/approval-waiting', {
          state: {
            session_id: body.session_id,
            device_id: body.device_id,
            device_label: deviceLabel.trim() || undefined,
            tunnel_base: session.tunnelUrl,
          },
        })
        return
      }
      navigate('/', { state: { tunnel_base: session.tunnelUrl } })
      return
    }

    // kind === 'code'
    const res = await fetch(`${session.tunnelUrl}/api/pairing/exchange`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      credentials: 'omit',
      body: JSON.stringify({
        code: lookupKey,
        device_label: deviceLabel.trim() || undefined,
      }),
    })
    if (!res.ok) {
      throw new Error('That code is invalid or already used.')
    }
    const body = (await res.json().catch(() => ({}))) as {
      api_key?: string
      api_key_last4?: string
      device_id?: string
      host_id?: string
      approval_status?: string
    }
    if (!body.api_key || !body.host_id) {
      throw new Error('Pairing succeeded but response is missing key/host id.')
    }
    if (deviceLabel.trim()) {
      window.localStorage.setItem('oxi:device-label', deviceLabel.trim())
    }
    storeApiKey(body.host_id, body.api_key)
    recordSavedHost({
      host_id: body.host_id,
      label: deviceLabel.trim() || body.host_id.slice(0, 8),
      api_key_last4: body.api_key_last4 ?? body.api_key.slice(-4),
    })
    if (body.approval_status === 'pending') {
      navigate('/approval-waiting', {
        state: {
          device_id: body.device_id,
          device_label: deviceLabel.trim() || undefined,
          tunnel_base: session.tunnelUrl,
        },
      })
      return
    }
    navigate('/', { state: { tunnel_base: session.tunnelUrl } })
  }

  // Single submit path. Auto-detects by `sk-` prefix:
  //   - permanent key   → /api/pairing/exchange { permanent_key }
  //   - everything else → try /api/login/one-time, fall back to /api/pairing/exchange { code }
  const submitAccessKey = async (raw: string) => {
    setError('')
    setLoading(true)
    try {
      // Cross-origin SPA can't reach same-origin /api/* — route through the
      // discovery worker. Permanent-key entry still goes through the same
      // path: shape it to its uppercase form and the worker resolves the
      // tunnel just like a pairing code (the agent registers both).
      if (isDiscoveryMode()) {
        return await submitDiscoveryCode(raw)
      }
      if (isPermanentKey(raw)) {
        const res = await fetch('/api/pairing/exchange', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            permanent_key: raw.trim(),
            device_label: deviceLabel.trim() || undefined,
          }),
        })
        if (!res.ok) {
          throw new Error('Permanent key is invalid or has not been generated yet.')
        }
        return await handlePairingSuccess(res)
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
        return await handleOtkSuccess(otkRes)
      }
      const codeAttempt = cleaned.toUpperCase()
      const codeRes = await fetch('/api/pairing/exchange', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          code: codeAttempt,
          device_label: deviceLabel.trim() || undefined,
        }),
      })
      if (!codeRes.ok) {
        throw new Error('That key is invalid or expired.')
      }
      return await handlePairingSuccess(codeRes)
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Could not sign in.')
    } finally {
      setLoading(false)
    }
  }

  // Pre-fill from ?k=<token> (QR deep-link). Three shapes are possible:
  //   1. Discovery temp key (32 lowercase hex) + ?otk=<otk> — cross-origin
  //      pair against the agent resolved via the discovery worker.
  //   2. OTK (16 base32) — same-origin /api/login/one-time auto-submit.
  //   3. Anything else — pre-fill the form and let the user finish typing.
  // The URL is scrubbed in the auto-submit branches so the credentials don't
  // sit in browser history.
  useEffect(() => {
    if (checkingAuth) return
    const k = searchParams.get('k')
    if (!k) return

    if (isDiscoveryMode() && isLikelyTempKey(k.trim().toLowerCase())) {
      const otk = (searchParams.get('otk') ?? '').trim()
      const tempKey = k.trim().toLowerCase()
      if (otk && !loading) {
        window.history.replaceState({}, '', '/login')
        queueMicrotask(() => {
          void submitDiscoveryPair(tempKey, otk)
        })
        return
      }
    }

    const OTK_RAW_LEN = 16
    const cleaned = sanitizeAccessKey(k).toLowerCase().slice(0, OTK_RAW_LEN)
    if (cleaned.length === OTK_RAW_LEN && !loading) {
      window.history.replaceState({}, '', '/login')
      queueMicrotask(() => {
        void submitAccessKey(cleaned)
      })
      return
    }
    queueMicrotask(() => {
      setAccessKey(cleaned)
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchParams, checkingAuth])

  const tryReconnect = async (h: SavedHost) => {
    setError('')
    setReconnectingId(h.host_id)
    const key = loadApiKey(h.host_id)
    if (key) storeApiKey(h.host_id, key)
    try {
      const res = await fetch('/api/me', { credentials: 'include' })
      if (res.ok) {
        await useHostStore.getState().fetchHost()
        navigate('/', { replace: true })
        return
      }
    } catch {
      // Network-level error: treat as session expired below.
    } finally {
      setReconnectingId(null)
    }
    removeSavedHost(h.host_id)
    setSavedHosts((list) => list.filter((x) => x.host_id !== h.host_id))
    setError('That session expired. Pair this device again.')
  }

  if (checkingAuth) {
    return (
      <div className="min-h-dvh flex items-center justify-center text-text-muted text-sm">
        Loading…
      </div>
    )
  }

  return (
    <div className="min-h-dvh bg-dot-grid flex items-center justify-center px-6 py-10">
      <div className="w-full max-w-sm">
        <header className="mb-6">
          <h1 className="text-2xl font-semibold tracking-tight text-text-primary">
            Pair this device
          </h1>
          <p className="mt-1.5 text-sm text-text-secondary leading-relaxed">
            Run oxiremote on your computer, then paste the key shown next to its
            QR code, or scan the QR with your phone camera.
          </p>
        </header>

        {rejectedError && (
          <div className="mb-4 px-3 py-2.5 rounded-md bg-danger/10 border border-danger/30 text-danger text-sm">
            {rejectedError}
          </div>
        )}

        {savedHosts.length > 0 && (
          <section className="mb-5">
            <h2 className="text-xs font-medium text-text-muted mb-2">Recently paired</h2>
            <div className="space-y-1.5">
              {savedHosts.map((h) => {
                const hasKey = Boolean(loadApiKey(h.host_id))
                const isReconnecting = reconnectingId === h.host_id
                return (
                  <button
                    key={h.host_id}
                    type="button"
                    onClick={() => { if (!isReconnecting) void tryReconnect(h) }}
                    disabled={isReconnecting}
                    className="w-full flex items-center gap-2 px-3 py-2 bg-surface-alt border border-border rounded-lg hover:bg-surface-hover transition-colors text-left disabled:opacity-70"
                  >
                    {/* Status dot or inline spinner */}
                    {isReconnecting ? (
                      <span
                        aria-hidden="true"
                        className="w-3.5 h-3.5 rounded-full border-2 border-current border-t-transparent animate-spin text-text-muted shrink-0"
                      />
                    ) : (
                      <span
                        className={[
                          'w-1.5 h-1.5 rounded-full shrink-0',
                          hasKey ? 'bg-success' : 'bg-text-muted',
                        ].join(' ')}
                      />
                    )}
                    <div className="flex-1 min-w-0">
                      <div className="text-sm text-text-primary truncate">{h.label}</div>
                      <div className="text-xs text-text-muted truncate">
                        {formatRelative(h.last_seen)}
                        {h.api_key_last4 ? ` · ····${h.api_key_last4}` : ''}
                      </div>
                    </div>
                  </button>
                )
              })}
            </div>
            <div className="text-center text-xs text-text-muted mt-3">
              — or pair a new device —
            </div>
          </section>
        )}

        <AccessKeyForm
          value={accessKey}
          onChange={setAccessKey}
          loading={loading}
          onSubmit={(raw) => void submitAccessKey(raw)}
        />

        <DeviceLabelField value={deviceLabel} onChange={setDeviceLabel} />

        {error && (
          <div className="mt-4 px-3 py-2 rounded-md bg-danger/10 border border-danger/30 text-danger text-sm">
            {error}
            <div className="mt-1 text-xs text-danger/80">
              Generate a fresh key from the host dashboard if this one expired.
            </div>
          </div>
        )}
      </div>
    </div>
  )
}

