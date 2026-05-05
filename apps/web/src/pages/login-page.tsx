import { useEffect, useState } from 'react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { isDiscoveryMode, isLikelyTempKey } from '../lib/discovery-client'
import { listSavedHosts, type SavedHost } from '../lib/saved-hosts'
import { sanitizeAccessKey } from '../components/login/access-key-form'
import AccessKeyForm from '../components/login/access-key-form'
import DeviceLabelField from '../components/login/device-label-field'
import SavedHostsPanel from '../components/login/saved-hosts-panel'
import { submitAccessKey, submitDiscoveryPair } from '../lib/login-pair-flows'

// Pairing entry point. A single input accepts OTK (16-hex), pairing codes
// (6-16 alnum), or permanent keys (sk-…). The submit path auto-detects by
// `sk-` prefix. Deep-link `?k=` auto-submits without showing the form.
//
// Pair-flow logic (same-origin + discovery-mode cross-origin) lives in
// `lib/login-pair-flows.ts`. Saved-hosts UI lives in `<SavedHostsPanel>`.
//
// Note: this page does NOT auto-redirect when an existing same-origin
// session cookie is valid. Users land here from the welcome screen or
// topbar "Pair new host…" with the deliberate intent to add another host;
// bouncing to the active host's workspace breaks multi-host pairing. The
// SavedHostsPanel above the form lets returning users jump back without
// re-pairing.

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
  // Snapshot the saved-hosts list once on mount — adding a new pair after
  // the form is open shouldn't reorder the panel under the user's finger.
  const [savedHosts, setSavedHosts] = useState<SavedHost[]>(() => listSavedHosts())

  const rejectedError =
    searchParams.get('error') === 'rejected'
      ? 'The host declined this device. You can try again with a fresh key.'
      : null

  const runSubmit = async (raw: string) => {
    setError('')
    setLoading(true)
    try {
      await submitAccessKey(raw, { deviceLabel, navigate })
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Could not sign in.')
    } finally {
      setLoading(false)
    }
  }

  const runDiscoveryPair = async (tempKey: string, otk: string) => {
    setError('')
    setLoading(true)
    try {
      await submitDiscoveryPair(tempKey, otk, { deviceLabel, navigate })
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Could not pair via discovery.')
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
    const k = searchParams.get('k')
    if (!k) return

    if (isDiscoveryMode() && isLikelyTempKey(k.trim().toLowerCase())) {
      const otk = (searchParams.get('otk') ?? '').trim()
      const tempKey = k.trim().toLowerCase()
      if (otk && !loading) {
        window.history.replaceState({}, '', '/login')
        queueMicrotask(() => {
          void runDiscoveryPair(tempKey, otk)
        })
        return
      }
    }

    const OTK_RAW_LEN = 16
    const cleaned = sanitizeAccessKey(k).toLowerCase().slice(0, OTK_RAW_LEN)
    if (cleaned.length === OTK_RAW_LEN && !loading) {
      window.history.replaceState({}, '', '/login')
      queueMicrotask(() => {
        void runSubmit(cleaned)
      })
      return
    }
    queueMicrotask(() => {
      setAccessKey(cleaned)
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchParams])

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

        <SavedHostsPanel
          hosts={savedHosts}
          onForget={(hostId) =>
            setSavedHosts((list) => list.filter((x) => x.host_id !== hostId))
          }
          onError={setError}
        />

        <AccessKeyForm
          value={accessKey}
          onChange={setAccessKey}
          loading={loading}
          onSubmit={(raw) => void runSubmit(raw)}
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
