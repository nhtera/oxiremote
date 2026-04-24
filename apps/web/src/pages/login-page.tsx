import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useHostStore } from '../state/host-store'
import { storeApiKey } from '../lib/api-client'

export default function LoginPage() {
  const [code, setCode] = useState('')
  const [deviceLabel, setDeviceLabel] = useState(
    typeof window !== 'undefined' ? window.localStorage.getItem('oxi:device-label') ?? '' : '',
  )
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(false)
  const [checkingAuth, setCheckingAuth] = useState(true)
  const navigate = useNavigate()

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
        <h1 className="text-xl font-semibold mb-1">OxiRemote</h1>
        <p className="text-text-secondary text-sm mb-6">
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

        {error && (
          <div className="mt-3 text-danger text-sm text-center">{error}</div>
        )}
      </div>
    </div>
  )
}
