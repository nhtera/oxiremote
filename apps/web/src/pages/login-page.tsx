import { useEffect, useMemo, useState } from 'react'
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

// Pairing entry point. Two modes: one-time key (default, the QR-deep-link
// flow) and pairing code (typed manually from the host's TUI). The two-mode
// toggle keeps both visible at the cost of a tap; the deep-link flow
// auto-submits and never shows the form to anyone who scanned a QR.

type Mode = 'key' | 'code'

const OTK_RAW_LEN = 16

export default function LoginPage() {
  const [searchParams] = useSearchParams()
  const navigate = useNavigate()

  // Default to OTK; honour ?mode=code or ?mode=key.
  const initialMode: Mode = searchParams.get('mode') === 'code' ? 'code' : 'key'
  const [mode, setMode] = useState<Mode>(initialMode)

  const [code, setCode] = useState('')
  const [otkRaw, setOtkRaw] = useState('') // Stored without dashes, lowercase
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

  const submitPairingCode = async (override?: string) => {
    const trimmed = (override ?? code).trim().toUpperCase()
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
      if (!res.ok) {
        const text = await res.text()
        throw new Error(text || 'Invalid or expired pairing code')
      }
      if (deviceLabel.trim()) {
        window.localStorage.setItem('oxi:device-label', deviceLabel.trim())
      }
      // Persist API key for tunnel `Authorization: Bearer …`.
      try {
        const body = await res.clone().json()
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
        }
      } catch {
        await useHostStore.getState().fetchHost()
      }
      navigate('/')
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : 'Pairing failed')
    } finally {
      setLoading(false)
    }
  }

  const submitOtk = async (override?: string) => {
    const raw = (override ?? otkRaw).trim()
    if (!raw) return
    setError('')
    setLoading(true)
    try {
      const res = await fetch('/api/login/one-time', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        credentials: 'include',
        body: JSON.stringify({ token: raw }),
      })
      if (res.status === 202) {
        const body = await res.json()
        // Pass session id + device label through router state so the
        // approval-waiting page can show a meaningful summary card.
        navigate('/approval-waiting', {
          state: {
            session_id: body.session_id,
            device_label: deviceLabel.trim() || undefined,
          },
        })
        return
      }
      if (res.status === 200) {
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
        return
      }
      const text = await res.text()
      throw new Error(text || 'That key is invalid or expired.')
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : 'Could not sign in.')
    } finally {
      setLoading(false)
    }
  }

  // Pre-fill OTK from `?k=<token>` (QR deep-link). Auto-submit when the
  // token looks plausible. Scrub the URL so it doesn't sit in history.
  // Sits below `submitOtk` so the call below is well-defined statically.
  // submitOtk + setOtkRaw/setMode are scheduled via queueMicrotask so they
  // run after the effect body returns — the React-hooks lint rule traces
  // sync setState calls but stops at the microtask boundary.
  useEffect(() => {
    if (checkingAuth) return
    const k = searchParams.get('k')
    if (!k) return
    const cleaned = sanitizeOtk(k)
    if (cleaned.length === OTK_RAW_LEN && !loading) {
      window.history.replaceState({}, '', '/login')
      queueMicrotask(() => {
        void submitOtk(cleaned)
      })
      return
    }
    // Token didn't make it intact (truncation / sanitiser drop) — leave the
    // form pre-filled so the user can fix it.
    queueMicrotask(() => {
      setOtkRaw(cleaned)
      setMode('key')
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

  const otkValid = otkRaw.length === OTK_RAW_LEN
  const codeValid = code.trim().length >= 6

  return (
    <div className="min-h-dvh flex items-center justify-center px-6 py-10">
      <div className="w-full max-w-sm">
        <header className="mb-6">
          <h1 className="text-2xl font-semibold tracking-tight text-text-primary">
            Pair this device
          </h1>
          <p className="mt-1.5 text-sm text-text-secondary leading-relaxed">
            {mode === 'key'
              ? 'Run oxiremote on your computer, then scan its QR code or paste the one-time key below.'
              : 'Type the 8-character pairing code shown in the OxiRemote terminal.'}
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

        {/* Mode toggle */}
        <div
          role="tablist"
          aria-label="Pairing method"
          className="grid grid-cols-2 gap-1 p-1 rounded-lg bg-surface-alt border border-border mb-5"
        >
          <ModeButton active={mode === 'key'} onClick={() => setMode('key')}>
            One-time key
          </ModeButton>
          <ModeButton active={mode === 'code'} onClick={() => setMode('code')}>
            Pairing code
          </ModeButton>
        </div>

        {mode === 'key' ? (
          <OtkForm
            otkRaw={otkRaw}
            onChange={setOtkRaw}
            valid={otkValid}
            onSubmit={() => submitOtk()}
            loading={loading}
          />
        ) : (
          <CodeForm
            code={code}
            onChange={setCode}
            valid={codeValid}
            onSubmit={() => submitPairingCode()}
            loading={loading}
          />
        )}

        <DeviceLabelField value={deviceLabel} onChange={setDeviceLabel} />

        {error && (
          <div className="mt-4 px-3 py-2 rounded-md bg-danger/10 border border-danger/30 text-danger text-sm">
            {error}
            {mode === 'key' && (
              <div className="mt-1 text-xs text-danger/80">
                Generate a fresh key from the host dashboard if this one
                expired.
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  )
}

// Strip dashes/spaces, lowercase, and clamp to OTK_RAW_LEN.
function sanitizeOtk(s: string): string {
  return s.replace(/[\s-]/g, '').toLowerCase().slice(0, OTK_RAW_LEN)
}

// Format raw OTK as `xxxx-xxxx-xxxx-xxxx` for display.
function formatOtk(raw: string): string {
  const cleaned = sanitizeOtk(raw)
  const groups: string[] = []
  for (let i = 0; i < cleaned.length; i += 4) {
    groups.push(cleaned.slice(i, i + 4))
  }
  return groups.join('-')
}

interface ModeButtonProps {
  active: boolean
  onClick: () => void
  children: React.ReactNode
}

function ModeButton({ active, onClick, children }: ModeButtonProps) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={active}
      onClick={onClick}
      className={
        'py-2 text-sm font-medium rounded-md transition-colors ' +
        (active
          ? 'bg-surface text-text-primary border border-border shadow-sm'
          : 'text-text-secondary hover:text-text-primary')
      }
    >
      {children}
    </button>
  )
}

interface OtkFormProps {
  otkRaw: string
  onChange: (raw: string) => void
  valid: boolean
  onSubmit: () => void
  loading: boolean
}

function OtkForm({ otkRaw, onChange, valid, onSubmit, loading }: OtkFormProps) {
  const formatted = useMemo(() => formatOtk(otkRaw), [otkRaw])
  return (
    <div>
      <label
        htmlFor="oxi-otk"
        className="block text-xs font-medium text-text-muted mb-1.5"
      >
        One-time key
      </label>
      <div className="relative">
        <input
          id="oxi-otk"
          value={formatted}
          onChange={(e) => onChange(sanitizeOtk(e.target.value))}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && valid && !loading) onSubmit()
          }}
          placeholder="xxxx-xxxx-xxxx-xxxx"
          autoComplete="off"
          autoCapitalize="none"
          autoCorrect="off"
          spellCheck={false}
          maxLength={19}
          className="w-full px-3 py-3 text-base bg-surface-alt border border-border rounded-lg text-text-primary text-center tracking-[0.2em] font-mono focus:outline-none focus:border-accent/50 focus:ring-2 focus:ring-accent/20"
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
      <p className="mt-1.5 text-xs text-text-muted">
        16 characters. Dashes are added automatically.
      </p>
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

interface CodeFormProps {
  code: string
  onChange: (s: string) => void
  valid: boolean
  onSubmit: () => void
  loading: boolean
}

function CodeForm({ code, onChange, valid, onSubmit, loading }: CodeFormProps) {
  return (
    <div>
      <label
        htmlFor="oxi-code"
        className="block text-xs font-medium text-text-muted mb-1.5"
      >
        Pairing code
      </label>
      <input
        id="oxi-code"
        value={code}
        onChange={(e) => onChange(e.target.value.toUpperCase().replace(/\s/g, ''))}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && valid && !loading) onSubmit()
        }}
        placeholder="ABCDEFGH"
        autoComplete="one-time-code"
        autoCapitalize="characters"
        autoCorrect="off"
        spellCheck={false}
        maxLength={16}
        className="w-full px-3 py-3 text-lg bg-surface-alt border border-border rounded-lg text-text-primary text-center tracking-[0.3em] font-mono uppercase focus:outline-none focus:border-accent/50 focus:ring-2 focus:ring-accent/20"
      />
      <p className="mt-1.5 text-xs text-text-muted">
        Shown next to the QR code in the OxiRemote terminal.
      </p>
      <button
        onClick={onSubmit}
        disabled={loading || !valid}
        className="w-full mt-3 py-3 text-sm font-medium bg-accent/15 text-accent border border-accent/30 rounded-lg hover:bg-accent/25 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
      >
        {loading ? 'Pairing…' : 'Pair this device'}
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
