import { useEffect, useRef } from 'react'
import {
  EditorState,
  Compartment,
  type Extension,
} from '@codemirror/state'
import {
  EditorView,
  keymap,
  lineNumbers,
  highlightActiveLine,
  highlightActiveLineGutter,
  drawSelection,
  dropCursor,
  rectangularSelection,
  crosshairCursor,
  highlightSpecialChars,
} from '@codemirror/view'
import {
  defaultKeymap,
  history,
  historyKeymap,
  indentWithTab,
} from '@codemirror/commands'
import {
  searchKeymap,
  highlightSelectionMatches,
} from '@codemirror/search'
import {
  indentOnInput,
  bracketMatching,
  foldGutter,
  foldKeymap,
} from '@codemirror/language'
import {
  autocompletion,
  completionKeymap,
  closeBrackets,
  closeBracketsKeymap,
} from '@codemirror/autocomplete'
import { languageForPath } from '../lib/code-mirror-extensions'
import { oxiDarkTheme, oxiHighlighting } from '../lib/code-mirror-theme'

interface Props {
  value: string
  path: string | null
  readOnly?: boolean
  onChange?: (value: string) => void
  onSave?: () => void
}

export default function CodeEditor({ value, path, readOnly, onChange, onSave }: Props) {
  const hostRef = useRef<HTMLDivElement>(null)
  const viewRef = useRef<EditorView | null>(null)
  const langCompRef = useRef<Compartment | null>(null)
  const readOnlyCompRef = useRef<Compartment | null>(null)
  const onSaveRef = useRef(onSave)
  const onChangeRef = useRef(onChange)

  onSaveRef.current = onSave
  onChangeRef.current = onChange

  // Create the editor once on mount.
  useEffect(() => {
    if (!hostRef.current) return
    const langComp = new Compartment()
    const readOnlyComp = new Compartment()
    langCompRef.current = langComp
    readOnlyCompRef.current = readOnlyComp

    const saveKeymap = keymap.of([
      {
        key: 'Mod-s',
        preventDefault: true,
        run: () => {
          onSaveRef.current?.()
          return true
        },
      },
    ])

    const state = EditorState.create({
      doc: value,
      extensions: [
        lineNumbers(),
        foldGutter(),
        highlightActiveLine(),
        highlightActiveLineGutter(),
        highlightSelectionMatches(),
        highlightSpecialChars(),
        drawSelection(),
        dropCursor(),
        rectangularSelection(),
        crosshairCursor(),
        EditorView.lineWrapping,
        history(),
        indentOnInput(),
        bracketMatching(),
        closeBrackets(),
        autocompletion(),
        oxiHighlighting,
        oxiDarkTheme,
        EditorState.tabSize.of(2),
        readOnlyComp.of([
          EditorView.editable.of(!readOnly),
          EditorState.readOnly.of(!!readOnly),
        ]),
        saveKeymap,
        keymap.of([
          ...closeBracketsKeymap,
          ...defaultKeymap,
          ...searchKeymap,
          ...historyKeymap,
          ...foldKeymap,
          ...completionKeymap,
          indentWithTab,
        ]),
        langComp.of([]),
        EditorView.updateListener.of((u) => {
          if (u.docChanged) {
            onChangeRef.current?.(u.state.doc.toString())
          }
        }),
      ],
    })

    const view = new EditorView({ state, parent: hostRef.current })
    viewRef.current = view

    return () => {
      view.destroy()
      viewRef.current = null
      langCompRef.current = null
      readOnlyCompRef.current = null
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // Swap doc when the opened file changes (path-keyed).
  useEffect(() => {
    const view = viewRef.current
    if (!view) return
    if (view.state.doc.toString() !== value) {
      view.dispatch({
        changes: { from: 0, to: view.state.doc.length, insert: value },
      })
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path])

  // Load language pack when path changes.
  useEffect(() => {
    let cancelled = false
    async function apply() {
      const view = viewRef.current
      const comp = langCompRef.current
      if (!view || !comp) return
      const lang: Extension | null = path ? await languageForPath(path) : null
      if (cancelled) return
      view.dispatch({ effects: comp.reconfigure(lang ? [lang] : []) })
    }
    void apply()
    return () => {
      cancelled = true
    }
  }, [path])

  // Reflect read-only changes without rebuilding the editor.
  useEffect(() => {
    const view = viewRef.current
    const comp = readOnlyCompRef.current
    if (!view || !comp) return
    view.dispatch({
      effects: comp.reconfigure([
        EditorView.editable.of(!readOnly),
        EditorState.readOnly.of(!!readOnly),
      ]),
    })
  }, [readOnly])

  return <div ref={hostRef} className="flex-1 min-h-0 overflow-hidden" />
}
