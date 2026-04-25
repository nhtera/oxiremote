import { useContext } from 'react'
import { ToastContext, type ToastApi } from './toast-context'

export function useToast(): ToastApi {
  const ctx = useContext(ToastContext)
  if (!ctx) throw new Error('useToast() called outside <ToastProvider>')
  return ctx
}

export type { ToastKind, ToastOptions, ToastApi } from './toast-context'
