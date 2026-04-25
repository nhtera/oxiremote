import { createContext } from 'react'

export type ToastKind = 'info' | 'success' | 'warning' | 'danger'

export interface ToastOptions {
  kind?: ToastKind
  title: string
  message?: string
  durationMs?: number
}

export interface ToastApi {
  show: (opts: ToastOptions) => number
  dismiss: (id: number) => void
}

export const ToastContext = createContext<ToastApi | null>(null)
