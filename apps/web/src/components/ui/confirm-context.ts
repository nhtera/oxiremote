import { createContext } from 'react'

export interface ConfirmOptions {
  title: string
  message?: string
  confirmText?: string
  cancelText?: string
  danger?: boolean
}

export interface PromptOptions extends ConfirmOptions {
  initial?: string
  placeholder?: string
}

export interface ConfirmApi {
  confirm: (opts: ConfirmOptions) => Promise<boolean>
  prompt: (opts: PromptOptions) => Promise<string | null>
}

export const ConfirmContext = createContext<ConfirmApi | null>(null)
