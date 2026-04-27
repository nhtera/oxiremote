import { useEffect, useState } from 'react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { useHostStore } from '../state/host-store'
import { loadApiKey, storeApiKey } from '../lib/api-client'
import {
  formatRelative,
  listSavedHosts,
  recordSavedHost,
  removeSavedHost,
  type SavedHost,
} from '../lib/saved-hosts'

// Pairing entry point. Single access-key field accepts both OTK (16-hex) and
// pairing codes (6-16 alnum). Submit tries OTK endpoint first; on non-200/202
// falls back to pairing-code exchange. The deep-link ?k= path auto-submits
// without showing the form.

const OTK_RAW_LEN = 16

export default function LoginPage() {
  const [searchParams] = useSearchParams()
  const navigate = useNavigate()

  // Single unified input — raw value, no auto-formatting
  const [accessKey, setAccessKey] = useState('')
  const [deviceLabel, setDeviceLabel] = useState(
    typeof window !== 'undefined'
      ? window.localStorage.getItem('oxi:device-label') ?? ''
      : '',
  )
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(false)
  const [checkingAuth, setCheckingAuth] = useState(true)
  // Snapshot the saved-hosts list once on mount — adding a new pair after
  // the form is open shouldn't reorder the panel under the user's finger.
  const [savedHosts, setSavedHosts] = useState<SavedHost[]>(() => listSavedHosts())

  const rejectedError =
    searchParams.get('error') === 'rejected'
      ? 'The host declined this device. You can try again with a fresh key.'
      : null

  // Skip the form entirely if the session cookie is already valid.
  useEffect(() => {
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
      // OTK auto-approved path: stamp the saved-hosts list so the user
      // gets a one-tap reconnect next visit. The api_key isn't returned
      // here (cookie-based session); fall back to "" for last4.
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
    // Read response body once.
    const body = await res.json().catch(() => ({}))

    if (deviceLabel.trim()) {
      window.localStorage.setItem('oxi:device-label', deviceLabel.trim())
    }
    // Persist API key BEFORE any navigation. The pending-approval path also
    // needs the key in localStorage — once the operator approves and the
    // user lands on the workspace, every tunnel request needs the
    // `Authorization: Bearer …` header. Cookie auth alone covers loopback
    // but not the tunnel.
    //
    // Pending-state subtlety: while approval_status === 'pending' the server
    // rejects Bearer auth (verify_api_key SQL filters non-approved devices),
    // so /api/host returns 401 → fetchHost has no host_id → we cannot key
    // localStorage by hostId yet. Stash the api_key in sessionStorage under
    // device_id; ApprovalWaitingPage promotes it to oxi_api_key_<hostId> on
    // approval observation. This keeps the "no Bearer auth before approval"
    // contract while preserving the key for the post-approval navigation.
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

    // Pairing-code pending: operator must approve before access is granted.
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

  // Sanitize raw input: strip whitespace + dashes, for OTK also lowercase.
  function sanitizeKey(s: string): string {
    return s.replace(/[\s-]/g, '')
  }

  // Try OTK endpoint first; on non-200/202 fall back to pairing exchange.
  // One extra RTT on pairing-code paths — invisible to the user behind the
  // shared "Connecting…" spinner.
  const submitAccessKey = async (raw: string) => {
    setError('')
    setLoading(true)
    try {
      // Try OTK first — sanitized to lowercase 16-hex shape
      const otkAttempt = raw.toLowerCase()
      const otkRes = await fetch('/api/login/one-time', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        credentials: 'include',
        body: JSON.stringify({ token: otkAttempt }),
      })
      if (otkRes.status === 200 || otkRes.status === 202) {
        return await handleOtkSuccess(otkRes)
      }
      // Fall back to pairing-code exchange — uppercased 6-16 alnum
      const codeAttempt = raw.toUpperCase()
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

  // Pre-fill from `?k=<token>` (QR deep-link). Auto-submit when the token
  // looks plausible. Scrubs the URL so it doesn't sit in history.
  // submitAccessKey + setAccessKey are scheduled via queueMicrotask so they
  // run after the effect body returns — the React-hooks lint rule traces
  // sync setState calls but stops at the microtask boundary.
  useEffect(() => {
    if (checkingAuth) return
    const k = searchParams.get('k')
    if (!k) return
    const cleaned = sanitizeKey(k).toLowerCase().slice(0, OTK_RAW_LEN)
    if (cleaned.length === OTK_RAW_LEN && !loading) {
      window.history.replaceState({}, '', '/login')
      queueMicrotask(() => {
        void submitAccessKey(cleaned)
      })
      return
    }
    // Token didn't make it intact (truncation / sanitiser drop) — leave the
    // form pre-filled so the user can fix it.
    queueMicrotask(() => {
      setAccessKey(cleaned)
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchParams, checkingAuth])

  const tryReconnect = async (h: SavedHost) => {
    setError('')
    // Surface the per-host API key into the active-host slot so the fetch
    // interceptor attaches Bearer for /api/me. If the cookie session is
    // also alive we'll get a 200; otherwise prune the entry and return.
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

  // Cleaned length for validity check — pairing code minimum is 6 chars.
  const cleanedKey = sanitizeKey(accessKey)
  const keyValid = cleanedKey.length >= 6

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
              {savedHosts.map((h) => (
                <button
                  key={h.host_id}
                  type="button"
                  onClick={() => tryReconnect(h)}
                  className="w-full flex items-center gap-2 px-3 py-2 bg-surface-alt border border-border rounded-lg hover:bg-surface-hover transition-colors text-left"
                >
                  <span className="w-1.5 h-1.5 rounded-full bg-text-muted shrink-0" />
                  <div className="flex-1 min-w-0">
                    <div className="text-sm text-text-primary truncate">{h.label}</div>
                    <div className="text-xs text-text-muted truncate">
                      {formatRelative(h.last_seen)}
                      {h.api_key_last4 ? ` · ····${h.api_key_last4}` : ''}
                    </div>
                  </div>
                </button>
              ))}
            </div>
            <div className="text-center text-xs text-text-muted mt-3">
              — or pair a new device —
            </div>
          </section>
        )}

        <AccessKeyForm
          value={accessKey}
          onChange={setAccessKey}
          valid={keyValid}
          onSubmit={() => void submitAccessKey(cleanedKey)}
          loading={loading}
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

interface AccessKeyFormProps {
  value: string
  onChange: (s: string) => void
  valid: boolean
  onSubmit: () => void
  loading: boolean
}

function AccessKeyForm({ value, onChange, valid, onSubmit, loading }: AccessKeyFormProps) {
  return (
    <div>
      <label
        htmlFor="oxi-access-key"
        className="block text-xs font-medium text-text-muted mb-1.5"
      >
        Access key
      </label>
      <div className="relative">
        <input
          id="oxi-access-key"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && valid && !loading) onSubmit()
          }}
          placeholder="xxxx-xxxx-xxxx-xxxx or ABCDEFGH"
          autoComplete="off"
          autoCapitalize="none"
          autoCorrect="off"
          spellCheck={false}
          className="w-full px-3 py-3 text-base bg-surface-alt border border-border rounded-lg text-text-primary font-mono focus:outline-none focus:border-accent/50 focus:ring-2 focus:ring-accent/20"
        />
        {valid && !loading && (
          <span
            aria-hidden="true"
            className="absolute right-3 top-1/2 -translate-y-1/2 text-success"
            title="Looks good"
          >
            <svg
              viewBox="0 0 24 24"
              className="w-5 h-5"
              fill="none"
              stroke="currentColor"
              strokeWidth="2.25"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <polyline points="20 6 9 17 4 12" />
            </svg>
          </span>
        )}
      </div>
      <button
        onClick={onSubmit}
        disabled={loading || !valid}
        className="w-full mt-3 py-3 text-sm font-medium bg-accent/15 text-accent border border-accent/30 rounded-lg hover:bg-accent/25 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
      >
        {loading ? 'Connecting…' : 'Pair this device'}
      </button>
    </div>
  )
}

interface DeviceLabelFieldProps {
  value: string
  onChange: (s: string) => void
}

function DeviceLabelField({ value, onChange }: DeviceLabelFieldProps) {
  return (
    <details className="mt-4 group">
      <summary className="cursor-pointer text-xs text-text-muted hover:text-text-secondary list-none flex items-center gap-1 select-none">
        <svg
          viewBox="0 0 24 24"
          className="w-3 h-3 transition-transform group-open:rotate-90"
          fill="none"
          stroke="currentColor"
          strokeWidth="2.5"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <polyline points="9 18 15 12 9 6" />
        </svg>
        Name this device (optional)
      </summary>
      <input
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder="iPhone 15 Pro"
        maxLength={80}
        className="w-full mt-2 px-3 py-2.5 text-sm bg-surface-alt border border-border rounded-lg text-text-primary focus:outline-none focus:border-accent/50"
      />
      <p className="mt-1.5 text-xs text-text-muted">
        Helps you identify this device in the host's device list later.
      </p>
    </details>
  )
}
