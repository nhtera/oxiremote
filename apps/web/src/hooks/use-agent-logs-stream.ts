import { useEffect, useRef, useState } from 'react'

// Shared SSE log stream + pause-buffer logic. Used by both the full /agent/logs
// page and the inline Logs tab on the agent home so they don't drift in
// behavior. Hard-cap matches the previous page-local constant.

export type LogLevel = 'info' | 'warn' | 'error'

export interface LogEntry {
  level: LogLevel
  module: string
  ts: number
  msg: string
}

const MAX_BUFFER = 5000

interface Result {
  entries: LogEntry[]
  setEntries: React.Dispatch<React.SetStateAction<LogEntry[]>>
  connected: boolean
  paused: boolean
  setPaused: (p: boolean) => void
  pendingCount: number
  resume: () => void
}

export function useAgentLogsStream(): Result {
  const [entries, setEntries] = useState<LogEntry[]>([])
  const [connected, setConnected] = useState(false)
  const [paused, setPaused] = useState(false)
  const [pendingCount, setPendingCount] = useState(0)

  // Pause buffer — incoming entries land here while paused, drain on resume so
  // the visible list stays steady while the operator reads.
  const pausedRef = useRef(false)
  const bufferRef = useRef<LogEntry[]>([])

  useEffect(() => {
    pausedRef.current = paused
  }, [paused])

  useEffect(() => {
    let cancelled = false

    fetch('/api/agent/logs/recent')
      .then((r) => (r.ok ? r.json() : null))
      .then((data) => {
        if (cancelled || !data?.entries) return
        const seeded: LogEntry[] = (data.entries as Array<Partial<LogEntry> & { type?: string }>)
          .filter((e) => e.type === 'log_entry')
          .map((e) => ({
            level: (e.level as LogLevel) ?? 'info',
            module: e.module ?? '',
            ts: Number(e.ts ?? 0),
            msg: e.msg ?? '',
          }))
        setEntries((prev) => {
          const merged = [...seeded, ...prev]
          return merged.length > MAX_BUFFER ? merged.slice(-MAX_BUFFER) : merged
        })
      })
      .catch(() => { /* SSE keeps streaming; backfill is best-effort */ })

    const es = new EventSource('/api/agent/events?filter=log')
    es.onopen = () => setConnected(true)
    es.onerror = () => setConnected(false)
    es.onmessage = (msg) => {
      try {
        const ev = JSON.parse(msg.data) as { type?: string } & Partial<LogEntry>
        if (ev.type !== 'log_entry') return
        const entry: LogEntry = {
          level: (ev.level as LogLevel) ?? 'info',
          module: ev.module ?? '',
          ts: Number(ev.ts ?? 0),
          msg: ev.msg ?? '',
        }
        if (pausedRef.current) {
          bufferRef.current.push(entry)
          if (bufferRef.current.length > MAX_BUFFER) {
            bufferRef.current = bufferRef.current.slice(-MAX_BUFFER)
          }
          setPendingCount(bufferRef.current.length)
        } else {
          setEntries((prev) => {
            const next = prev.length >= MAX_BUFFER ? prev.slice(-(MAX_BUFFER - 1)) : prev
            return [...next, entry]
          })
        }
      } catch {
        // drop malformed frames
      }
    }
    return () => {
      cancelled = true
      es.close()
    }
  }, [])

  function resume() {
    if (bufferRef.current.length > 0) {
      const drained = bufferRef.current
      bufferRef.current = []
      setEntries((prev) => {
        const merged = [...prev, ...drained]
        return merged.length > MAX_BUFFER ? merged.slice(-MAX_BUFFER) : merged
      })
    }
    setPendingCount(0)
    setPaused(false)
  }

  return { entries, setEntries, connected, paused, setPaused, pendingCount, resume }
}
