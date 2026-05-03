import { useEffect, useState } from 'react'
import { ensureTransport, readTransportInfo, type TransportInfo } from '../lib/transport'

// React-facing read of the LAN-vs-tunnel race result. The race itself lives
// in `lib/transport.ts` (one race per session, cached in sessionStorage) —
// this hook just kicks it on first mount and returns the picked winner so
// pages/badges can render the LAN/Tunnel pill.
//
// Returns 'unknown' while the race is in flight or no LAN base is configured.
export function useTransportRace(): TransportInfo {
  const [info, setInfo] = useState<TransportInfo>(() => readTransportInfo())

  useEffect(() => {
    if (info.transport !== 'unknown') return
    let cancelled = false
    ensureTransport()
      .then((next) => {
        if (!cancelled) setInfo(next)
      })
      .catch(() => { /* race is best-effort; pill stays 'unknown' */ })
    return () => {
      cancelled = true
    }
  }, [info.transport])

  return info
}
