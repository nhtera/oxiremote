import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { TerminalPrefs } from '../../lib/terminal-prefs'
import type { PaneAssignments, PaneCount, PaneIndex } from '../../state/terminal-store'
import type { UploadError } from '../../hooks/use-file-upload'
import XtermPane from './xterm-pane'

export type PaneUpload = {
  id: string
  fileName: string
  paneIdx: PaneIndex
  pct: number
  state: 'uploading' | 'error'
  error?: UploadError | null
}

export type PanePreview = {
  id: string
  fileName: string
  file: File
  paneIdx: PaneIndex
}

type Props = {
  paneCount: PaneCount
  paneAssignments: PaneAssignments
  focusedPane: PaneIndex
  prefs: TerminalPrefs
  reconnectNonce: number
  onFocusPane: (idx: PaneIndex) => void
  onConnectedChange: (sessionId: string, connected: boolean) => void
  onReconnectAttempt: (sessionId: string, attempt: number) => void
  onReconnectExhausted: (sessionId: string) => void
  onError: (msg: string) => void
  registerSend: (sessionId: string, sendFn: ((data: string) => void) | null) => void
  /** Optional registry for per-session selection getters. When supplied, the
   *  workspace's mobile keybar can offer a "Copy selection" button. */
  registerGetSelection?: (sessionId: string, fn: (() => string) | null) => void
  /** Forwarded to each pane — drop handler dispatches files to the workspace.
   *  `paneIdx` lets the page anchor the chip to the dropped-on pane. */
  onAttachFiles?: (files: File[], paneIdx?: PaneIndex) => void
  uploads?: PaneUpload[]
  previews?: PanePreview[]
  onCancelUpload?: (id: string) => void
  onDismissUpload?: (id: string) => void
  onRetryUpload?: (id: string) => void
  onDismissPreview?: (id: string) => void
}

// Smallest pane width that still feels usable for a shell. Drag-resize
// clamps below this and a viewport narrower than 768px collapses to 1 pane.
const MIN_PANE_PCT = 15

// Stable empty arrays for pane slices — avoids handing each Pane a fresh
// `[]` on every render (which would defeat any future React.memo).
const EMPTY_UPLOADS: PaneUpload[] = []
const EMPTY_PREVIEWS: PanePreview[] = []

function evenSizes(n: PaneCount): number[] {
  const each = 100 / n
  return Array.from({ length: n }, () => each)
}

export default function MultiPaneGrid({
  paneCount, paneAssignments, focusedPane, prefs, reconnectNonce,
  onFocusPane, onConnectedChange, onReconnectAttempt, onReconnectExhausted, onError, registerSend,
  registerGetSelection,
  onAttachFiles,
  uploads, previews,
  onCancelUpload, onDismissUpload, onRetryUpload, onDismissPreview,
}: Props) {
  const [isNarrow, setIsNarrow] = useState(() => window.matchMedia('(max-width: 768px)').matches)
  useEffect(() => {
    const mq = window.matchMedia('(max-width: 768px)')
    const onChange = () => setIsNarrow(mq.matches)
    mq.addEventListener('change', onChange)
    return () => mq.removeEventListener('change', onChange)
  }, [])
  const effectiveCount: PaneCount = isNarrow ? 1 : paneCount

  // Pane sizes as percentages summing to 100. Recomputed when the grid
  // shape changes; the user's drag adjustments are local to the current shape.
  const [sizes, setSizes] = useState<number[]>(() => evenSizes(effectiveCount))
  useEffect(() => { setSizes(evenSizes(effectiveCount)) }, [effectiveCount])

  const containerRef = useRef<HTMLDivElement | null>(null)

  const onDragHandle = useCallback((handleIdx: number, startX: number) => {
    // Convert the pixel delta into a percentage of the container width and
    // shift it from the right neighbour into the left. Clamping keeps both
    // sides above MIN_PANE_PCT.
    const el = containerRef.current
    if (!el) return
    const widthPx = el.getBoundingClientRect().width
    if (widthPx <= 0) return
    const startSizes = [...sizes]

    const onMove = (e: PointerEvent) => {
      const dxPct = ((e.clientX - startX) / widthPx) * 100
      const left = startSizes[handleIdx] + dxPct
      const right = startSizes[handleIdx + 1] - dxPct
      if (left < MIN_PANE_PCT || right < MIN_PANE_PCT) return
      setSizes((prev) => {
        const next = [...prev]
        next[handleIdx] = left
        next[handleIdx + 1] = right
        return next
      })
    }
    const onUp = () => {
      window.removeEventListener('pointermove', onMove)
      window.removeEventListener('pointerup', onUp)
      document.body.style.cursor = ''
      document.body.style.userSelect = ''
    }
    document.body.style.cursor = 'col-resize'
    document.body.style.userSelect = 'none'
    window.addEventListener('pointermove', onMove)
    window.addEventListener('pointerup', onUp)
  }, [sizes])

  const indices: PaneIndex[] = Array.from({ length: effectiveCount }, (_, i) => i as PaneIndex)
  const effectiveFocus: PaneIndex = focusedPane >= effectiveCount ? 0 : focusedPane

  // Group chips by pane once per render so each Pane receives a stable slice
  // instead of all panes filtering the full array on every render.
  const uploadsByPane = useMemo(() => {
    const m = new Map<PaneIndex, PaneUpload[]>()
    for (const u of uploads ?? []) {
      const arr = m.get(u.paneIdx) ?? []
      arr.push(u)
      m.set(u.paneIdx, arr)
    }
    return m
  }, [uploads])
  const previewsByPane = useMemo(() => {
    const m = new Map<PaneIndex, PanePreview[]>()
    for (const p of previews ?? []) {
      const arr = m.get(p.paneIdx) ?? []
      arr.push(p)
      m.set(p.paneIdx, arr)
    }
    return m
  }, [previews])

  return (
    <div ref={containerRef} className="flex flex-1 min-h-0 min-w-0">
      {indices.map((idx) => {
        const sid = paneAssignments[idx]
        const isFocused = idx === effectiveFocus
        const basis = sizes[idx] ?? 100 / effectiveCount
        return (
          <Pane
            key={idx}
            paneIdx={idx}
            sessionId={sid}
            basis={basis}
            isFocused={isFocused}
            prefs={prefs}
            reconnectNonce={reconnectNonce}
            onFocus={() => onFocusPane(idx)}
            onConnectedChange={onConnectedChange}
            onReconnectAttempt={onReconnectAttempt}
            onReconnectExhausted={onReconnectExhausted}
            onError={onError}
            registerSend={registerSend}
            registerGetSelection={registerGetSelection}
            onAttachFiles={onAttachFiles}
            paneUploads={uploadsByPane.get(idx) ?? EMPTY_UPLOADS}
            panePreviews={previewsByPane.get(idx) ?? EMPTY_PREVIEWS}
            onCancelUpload={onCancelUpload}
            onDismissUpload={onDismissUpload}
            onRetryUpload={onRetryUpload}
            onDismissPreview={onDismissPreview}
            // The handle on the LEFT edge resizes between (idx-1, idx).
            onDragLeftHandle={idx > 0 ? (e) => onDragHandle(idx - 1, e.clientX) : null}
          />
        )
      })}
    </div>
  )
}

type PaneProps = {
  paneIdx: PaneIndex
  sessionId: string | null
  basis: number
  isFocused: boolean
  prefs: TerminalPrefs
  reconnectNonce: number
  onFocus: () => void
  onConnectedChange: (sessionId: string, connected: boolean) => void
  onReconnectAttempt: (sessionId: string, attempt: number) => void
  onReconnectExhausted: (sessionId: string) => void
  onError: (msg: string) => void
  registerSend: (sessionId: string, sendFn: ((data: string) => void) | null) => void
  /** Optional registry for per-session selection getters. When supplied, the
   *  workspace's mobile keybar can offer a "Copy selection" button. */
  registerGetSelection?: (sessionId: string, fn: (() => string) | null) => void
  onAttachFiles?: (files: File[], paneIdx?: PaneIndex) => void
  paneUploads: PaneUpload[]
  panePreviews: PanePreview[]
  onCancelUpload?: (id: string) => void
  onDismissUpload?: (id: string) => void
  onRetryUpload?: (id: string) => void
  onDismissPreview?: (id: string) => void
  onDragLeftHandle: ((e: React.PointerEvent) => void) | null
}

function Pane({
  paneIdx, sessionId, basis, isFocused, prefs, reconnectNonce,
  onFocus, onConnectedChange, onReconnectAttempt, onReconnectExhausted, onError, registerSend,
  registerGetSelection, onAttachFiles,
  paneUploads, panePreviews,
  onCancelUpload, onDismissUpload, onRetryUpload, onDismissPreview,
  onDragLeftHandle,
}: PaneProps) {
  return (
    <>
      {onDragLeftHandle && (
        <div
          role="separator"
          aria-orientation="vertical"
          aria-label={`Resize pane ${paneIdx}`}
          onPointerDown={(e) => { e.preventDefault(); onDragLeftHandle(e) }}
          className="w-1 cursor-col-resize bg-border hover:bg-accent/50 transition-colors shrink-0"
        />
      )}
      <div
        className="flex flex-col min-w-0 min-h-0 overflow-hidden"
        style={{ flexBasis: `${basis}%`, flexGrow: 0, flexShrink: 0 }}
        onClick={onFocus}
      >
        {sessionId ? (
          <XtermPane
            key={sessionId}
            sessionId={sessionId}
            paneIdx={paneIdx}
            prefs={prefs}
            isFocused={isFocused}
            onFocus={onFocus}
            onConnectedChange={onConnectedChange}
            onReconnectAttempt={onReconnectAttempt}
            onReconnectExhausted={onReconnectExhausted}
            onError={onError}
            registerSend={registerSend}
            registerGetSelection={registerGetSelection}
            onAttachFiles={onAttachFiles}
            uploads={paneUploads}
            previews={panePreviews}
            onCancelUpload={onCancelUpload}
            onDismissUpload={onDismissUpload}
            onRetryUpload={onRetryUpload}
            onDismissPreview={onDismissPreview}
            reconnectNonce={reconnectNonce}
          />
        ) : (
          <EmptyPane focused={isFocused} />
        )}
      </div>
    </>
  )
}

function EmptyPane({ focused }: { focused: boolean }) {
  return (
    <div className={`flex flex-1 min-w-0 min-h-0 items-center justify-center text-center text-text-muted text-xs px-4 ${
      focused ? 'ring-1 ring-inset ring-accent/40' : ''
    }`}>
      <div>
        <div className="text-text-secondary mb-1">Empty pane</div>
        <div>Click a tab to load it here.</div>
      </div>
    </div>
  )
}
