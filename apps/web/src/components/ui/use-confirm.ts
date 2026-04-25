import { useContext } from 'react'
import { ConfirmContext } from './confirm-context'

export function useConfirm() {
  const ctx = useContext(ConfirmContext)
  if (!ctx) throw new Error('useConfirm() called outside <ConfirmProvider>')
  return ctx.confirm
}

export function usePrompt() {
  const ctx = useContext(ConfirmContext)
  if (!ctx) throw new Error('usePrompt() called outside <ConfirmProvider>')
  return ctx.prompt
}

export type { ConfirmOptions, PromptOptions, ConfirmApi } from './confirm-context'
