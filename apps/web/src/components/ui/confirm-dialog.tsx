import { useCallback, useId, useMemo, useRef, useState, type ReactNode } from 'react'
import Dialog from './dialog'
import Button from './button'
import {
  ConfirmContext,
  type ConfirmApi,
  type ConfirmOptions,
  type PromptOptions,
} from './confirm-context'

type Pending =
  | { kind: 'confirm'; opts: ConfirmOptions; resolve: (v: boolean) => void }
  | { kind: 'prompt'; opts: PromptOptions; resolve: (v: string | null) => void }

export function ConfirmProvider({ children }: { children: ReactNode }) {
  const [pending, setPending] = useState<Pending | null>(null)
  const [value, setValue] = useState('')

  const confirm = useCallback((opts: ConfirmOptions) => {
    return new Promise<boolean>((resolve) => {
      setPending({ kind: 'confirm', opts, resolve })
    })
  }, [])

  const prompt = useCallback((opts: PromptOptions) => {
    return new Promise<string | null>((resolve) => {
      setValue(opts.initial ?? '')
      setPending({ kind: 'prompt', opts, resolve })
    })
  }, [])

  const api = useMemo<ConfirmApi>(() => ({ confirm, prompt }), [confirm, prompt])

  function close(result: boolean | string | null) {
    if (!pending) return
    if (pending.kind === 'confirm') pending.resolve(result === true)
    else pending.resolve(result === false ? null : (result as string | null))
    setPending(null)
    setValue('')
  }

  return (
    <ConfirmContext.Provider value={api}>
      {children}
      <ActiveDialog
        pending={pending}
        value={value}
        onValueChange={setValue}
        onCancel={() => close(pending?.kind === 'prompt' ? null : false)}
        onConfirm={() => close(pending?.kind === 'prompt' ? value : true)}
      />
    </ConfirmContext.Provider>
  )
}

interface ActiveProps {
  pending: Pending | null
  value: string
  onValueChange: (v: string) => void
  onCancel: () => void
  onConfirm: () => void
}

function ActiveDialog({ pending, value, onValueChange, onCancel, onConfirm }: ActiveProps) {
  const titleId = useId()
  const descId = useId()
  const inputRef = useRef<HTMLInputElement>(null)

  if (!pending) return null
  const { opts, kind } = pending
  const isPrompt = kind === 'prompt'
  const promptOpts = isPrompt ? (opts as PromptOptions) : null

  return (
    <Dialog
      open
      onClose={onCancel}
      ariaLabelledBy={titleId}
      ariaDescribedBy={opts.message ? descId : undefined}
    >
      <h2 id={titleId} className="text-base font-semibold text-text-primary">
        {opts.title}
      </h2>
      {opts.message && (
        <p id={descId} className="mt-1.5 text-sm text-text-secondary">
          {opts.message}
        </p>
      )}
      {isPrompt && (
        <input
          ref={inputRef}
          autoFocus
          value={value}
          onChange={(e) => onValueChange(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault()
              onConfirm()
            }
          }}
          placeholder={promptOpts?.placeholder}
          className="mt-4 w-full text-sm bg-surface-alt border border-border rounded-md px-2 py-1.5 text-text-primary outline-none focus:border-accent/50"
        />
      )}
      <div className="mt-5 flex justify-end gap-2">
        <Button variant="ghost" size="sm" onClick={onCancel}>
          {opts.cancelText ?? 'Cancel'}
        </Button>
        <Button
          variant={opts.danger ? 'danger' : 'primary'}
          size="sm"
          onClick={onConfirm}
          autoFocus={!isPrompt}
        >
          {opts.confirmText ?? (isPrompt ? 'OK' : opts.danger ? 'Delete' : 'Confirm')}
        </Button>
      </div>
    </Dialog>
  )
}
