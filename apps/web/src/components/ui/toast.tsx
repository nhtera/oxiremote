import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { ToastContext, type ToastApi, type ToastKind, type ToastOptions } from './toast-context'

interface Toast extends Required<Omit<ToastOptions, 'message'>> {
  id: number
  message?: string
}

const DEFAULT_DURATION = 4000
const MAX_STACK = 3

const TONE: Record<ToastKind, string> = {
  info: 'bg-surface-alt text-text-primary border-border',
  success: 'bg-success/10 text-success border-success/30',
  warning: 'bg-warning/10 text-warning border-warning/30',
  danger: 'bg-danger/10 text-danger border-danger/30',
}

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([])
  const idRef = useRef(0)
  const timersRef = useRef<Map<number, number>>(new Map())

  const dismiss = useCallback((id: number) => {
    const tm = timersRef.current.get(id)
    if (tm) window.clearTimeout(tm)
    timersRef.current.delete(id)
    setToasts((xs) => xs.filter((t) => t.id !== id))
  }, [])

  const show = useCallback(
    (opts: ToastOptions) => {
      const id = ++idRef.current
      const toast: Toast = {
        id,
        kind: opts.kind ?? 'info',
        title: opts.title,
        message: opts.message,
        durationMs: opts.durationMs ?? DEFAULT_DURATION,
      }
      setToasts((xs) => [...xs.slice(-(MAX_STACK - 1)), toast])
      const tm = window.setTimeout(() => dismiss(id), toast.durationMs)
      timersRef.current.set(id, tm)
      return id
    },
    [dismiss],
  )

  // Cleanup pending timers on unmount.
  useEffect(() => {
    const map = timersRef.current
    return () => {
      for (const tm of map.values()) window.clearTimeout(tm)
      map.clear()
    }
  }, [])

  const api = useMemo<ToastApi>(() => ({ show, dismiss }), [show, dismiss])

  return (
    <ToastContext.Provider value={api}>
      {children}
      <ToastViewport toasts={toasts} onDismiss={dismiss} />
    </ToastContext.Provider>
  )
}

function ToastViewport({ toasts, onDismiss }: { toasts: Toast[]; onDismiss: (id: number) => void }) {
  if (toasts.length === 0) return null
  return (
    <div
      className="fixed inset-x-0 z-[60] flex flex-col items-center gap-2 px-3 pointer-events-none"
      style={{ bottom: 'calc(5rem + env(safe-area-inset-bottom, 0px))' }}
      aria-live="polite"
      aria-atomic="false"
    >
      {toasts.map((t) => (
        <ToastCard key={t.id} toast={t} onDismiss={() => onDismiss(t.id)} />
      ))}
    </div>
  )
}

function ToastCard({ toast, onDismiss }: { toast: Toast; onDismiss: () => void }) {
  const tone = TONE[toast.kind]
  return (
    <div
      role="status"
      className={`pointer-events-auto w-full max-w-sm border rounded-lg shadow-lg px-3 py-2 ${tone}`}
    >
      <div className="flex items-start gap-2">
        <div className="flex-1 min-w-0">
          <div className="text-sm font-medium truncate">{toast.title}</div>
          {toast.message && <div className="text-xs opacity-80 mt-0.5 break-words">{toast.message}</div>}
        </div>
        <button
          onClick={onDismiss}
          aria-label="Dismiss notification"
          className="text-current opacity-60 hover:opacity-100 px-1 leading-none"
        >
          ✕
        </button>
      </div>
    </div>
  )
}
