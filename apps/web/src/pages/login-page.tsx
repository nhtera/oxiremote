import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useHostStore } from '../state/host-store'

export default function LoginPage() {
  const [code, setCode] = useState('')
  const [deviceLabel, setDeviceLabel] = useState(
    typeof window !== 'undefined' ? window.localStorage.getItem('oxi:device-label') ?? '' : '',
  )
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(false)
  const navigate = useNavigate()

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
      // Pairing set the cookie — refresh the host store so route guards
      // (LegacyRedirect, AppLayout sidebar) see the authenticated state.
      await useHostStore.getState().fetchHost()
      navigate('/')
    } catch (e: any) {
      setError(e.message || 'Pairing failed')
    } finally {
      setLoading(false)
    }
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
