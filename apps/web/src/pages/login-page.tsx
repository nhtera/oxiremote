import { useEffect, useState } from 'react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { useHostStore } from '../state/host-store'
import { storeApiKey } from '../lib/api-client'

export default function LoginPage() {
  const [code, setCode] = useState('')
  const [otkToken, setOtkToken] = useState('')
  const [deviceLabel, setDeviceLabel] = useState(
    typeof window !== 'undefined' ? window.localStorage.getItem('oxi:device-label') ?? '' : '',
  )
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(false)
  const [checkingAuth, setCheckingAuth] = useState(true)
  const navigate = useNavigate()
  const [searchParams] = useSearchParams()

  // Show rejection banner when redirected back with ?error=rejected
  const rejectedError =
    searchParams.get('error') === 'rejected' ? 'Device rejected by host.' : null

  // If the session cookie is already valid, skip the pairing form entirely.
  // Avoids the user getting stuck here after opening the app from a bookmark.
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

  // Pre-fill OTK input if URL contains ?k=<token>. Auto-submit when the
  // token is present and the user isn't already authed — this is the
  // QR-deep-link flow ("scan QR → land here → enter OTK is automatic").
  // Scrub the token from the visible URL after consumption so it doesn't
  // sit in the address bar / browser history.
  useEffect(() => {
    if (checkingAuth) return
    const k = searchParams.get('k')
    if (!k) return
    setOtkToken(k)
    // OTKs are 16 chars in our generator; loose-match here so a future
    // length tweak doesn't silently break the auto-submit.
    if (k.length >= 12 && k.length <= 32 && !loading) {
      window.history.replaceState({}, '', '/login')
      void handleOtkLogin(k)
    }
    // We deliberately don't depend on `handleOtkLogin` (stable) or `loading`
    // (would re-fire on flip). Auto-submit happens once per page load.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchParams, checkingAuth])

  const handlePair = async () => {
    const trimmed = code.trim()
    if (!trimmed) return
    setError('')
    setLoading(true)
    try {
      const res = await fetch('/api/pairing/exchange', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          code: trimmed,
          device_label: deviceLabel.trim() || undefined,
        }),
      })
      if (!res.ok) throw new Error(await res.text() || 'Invalid or expired code')
      if (deviceLabel.trim()) {
        window.localStorage.setItem('oxi:device-label', deviceLabel.trim())
      }
      // Persist the API key tied to this host so subsequent tunnel requests
      // can attach `Authorization: Bearer …`.
      try {
        const body = await res.clone().json()
        await useHostStore.getState().fetchHost()
        const hostId = useHostStore.getState().currentHostId
        if (body.api_key && hostId) {
          storeApiKey(hostId, body.api_key)
        }
      } catch {
        await useHostStore.getState().fetchHost()
      }
      navigate('/')
    } catch (e: any) {
      setError(e.message || 'Pairing failed')
    } finally {
      setLoading(false)
    }
  }

  // One-time key login flow. Accepts an optional override so the
  // deep-link auto-submit doesn't have to wait for a re-render after
  // `setOtkToken`.
  const handleOtkLogin = async (override?: string) => {
    const trimmed = (override ?? otkToken).trim()
    if (!trimmed) return
    setError('')
    setLoading(true)
    try {
      const res = await fetch('/api/login/one-time', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        credentials: 'include',
        body: JSON.stringify({ token: trimmed }),
      })
      if (res.status === 202) {
        // Pending approval — navigate to waiting screen
        const body = await res.json()
        navigate('/approval-waiting', { state: { session_id: body.session_id } })
        return
      }
      if (res.status === 200) {
        // Auto-approved — go straight home
        await useHostStore.getState().fetchHost()
        navigate('/')
        return
      }
      const text = await res.text()
      throw new Error(text || 'Invalid or expired one-time key')
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : 'OTK login failed'
      setError(msg)
    } finally {
      setLoading(false)
    }
  }

  if (checkingAuth) {
    return (
      <div className="min-h-dvh flex items-center justify-center text-text-muted text-sm">
        Loading…
      </div>
    )
  }

  return (
    <div className="min-h-dvh flex items-center justify-center p-6">
      <div className="w-full max-w-sm">
        <h1 className="text-2xl font-semibold tracking-tight text-text-primary mb-1">OxiRemote</h1>

        {/* Rejection banner — shown when redirected back with ?error=rejected */}
        {rejectedError && (
          <div className="mb-4 px-3 py-2 rounded-md bg-danger/10 border border-danger/30 text-danger text-sm">
            {rejectedError}
          </div>
        )}

        {/* Pairing code section */}
        <p className="text-text-secondary text-sm mb-4">
          Enter the pairing code shown in your terminal.
        </p>

        <input
          value={code}
          onChange={(e) => setCode(e.target.value)}
          placeholder="ABCDEFGH"
          maxLength={16}
          autoComplete="one-time-code"
          onKeyDown={(e) => e.key === 'Enter' && handlePair()}
          className="w-full px-3 py-3 text-lg bg-surface-alt border border-border rounded-lg text-text-primary text-center tracking-widest font-mono uppercase focus:outline-none focus:border-accent/50"
        />

        <input
          value={deviceLabel}
          onChange={(e) => setDeviceLabel(e.target.value)}
          placeholder="This device name (optional)"
          maxLength={80}
          className="w-full mt-3 px-3 py-3 text-sm bg-surface-alt border border-border rounded-lg text-text-primary focus:outline-none focus:border-accent/50"
        />

        <button
          onClick={handlePair}
          disabled={loading || !code.trim()}
          className="w-full mt-3 py-3 text-sm font-medium bg-accent/15 text-accent border border-accent/30 rounded-lg hover:bg-accent/25 transition-colors disabled:opacity-40"
        >
          {loading ? 'Pairing…' : 'Pair'}
        </button>

        {/* Divider */}
        <div className="flex items-center gap-3 my-5">
          <div className="flex-1 h-px bg-border" />
          <span className="text-xs text-text-muted">or use one-time key</span>
          <div className="flex-1 h-px bg-border" />
        </div>

        {/* One-time key section */}
        <input
          value={otkToken}
          onChange={(e) => setOtkToken(e.target.value)}
          placeholder="16-character one-time key"
          maxLength={20}
          autoComplete="off"
          onKeyDown={(e) => e.key === 'Enter' && handleOtkLogin(undefined)}
          className="w-full px-3 py-3 text-sm bg-surface-alt border border-border rounded-lg text-text-primary font-mono focus:outline-none focus:border-warning/50"
        />

        <button
          onClick={() => handleOtkLogin()}
          disabled={loading || !otkToken.trim()}
          className="w-full mt-3 py-3 text-sm font-medium bg-warning/15 text-warning border border-warning/30 rounded-lg hover:bg-warning/25 transition-colors disabled:opacity-40"
        >
          {loading ? 'Connecting…' : 'Connect with Key'}
        </button>

        {error && (
          <div className="mt-3 text-danger text-sm text-center">{error}</div>
        )}
      </div>
    </div>
  )
}
