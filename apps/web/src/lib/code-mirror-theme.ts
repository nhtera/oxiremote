import { EditorView } from '@codemirror/view'
import { HighlightStyle, syntaxHighlighting } from '@codemirror/language'
import { tags as t } from '@lezer/highlight'
import type { Extension } from '@codemirror/state'

// Palette is a tuned One-Dark that matches OxiRemote's accent-blue + surface
// stack so the editor feels like part of the app rather than a separate widget.
const bg = '#16171d'              // --color-surface
const bgAlt = '#1b1d24'           // slightly lifted for gutter
const border = '#2e303a'          // --color-border
const fg = '#e5e7eb'              // softened --color-text-primary (avoid glow)
const comment = '#5b6070'         // muted
const caret = '#6cb4ff'           // --color-accent
const selection = 'rgba(108, 180, 255, 0.25)'
const selectionMatch = 'rgba(108, 180, 255, 0.15)'
const lineHighlight = 'rgba(255, 255, 255, 0.04)'
const lineNumber = '#4b5262'
const lineNumberActive = '#cbd5e1'

const red = '#e06c75'
const yellow = '#e5c07b'
const orange = '#d19a66'
const green = '#98c379'
const aqua = '#56b6c2'
const blue = '#7cb7ff'            // nudged from #61afef toward accent
const purple = '#c678dd'

export const oxiDarkTheme: Extension = EditorView.theme(
  {
    '&': {
      color: fg,
      backgroundColor: bg,
      height: '100%',
      fontSize: '13px',
    },
    '.cm-scroller': {
      fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace',
      lineHeight: '1.5',
    },
    '.cm-content': {
      caretColor: caret,
      padding: '6px 0',
    },
    '.cm-cursor, .cm-dropCursor': {
      borderLeftColor: caret,
      borderLeftWidth: '2px',
    },
    '.cm-gutters': {
      backgroundColor: bgAlt,
      color: lineNumber,
      border: 'none',
      borderRight: `1px solid ${border}`,
    },
    '.cm-lineNumbers .cm-gutterElement': {
      padding: '0 10px 0 6px',
      minWidth: '24px',
    },
    '.cm-activeLine': { backgroundColor: lineHighlight },
    '.cm-activeLineGutter': {
      backgroundColor: 'transparent',
      color: lineNumberActive,
    },
    '.cm-foldPlaceholder': {
      background: 'transparent',
      border: `1px solid ${border}`,
      color: '#9ca3af',
      borderRadius: '4px',
      padding: '0 4px',
      margin: '0 2px',
    },
    '&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, ::selection':
      {
        backgroundColor: selection,
      },
    '.cm-selectionBackground, ::selection': {
      backgroundColor: selection,
    },
    '.cm-selectionMatch': { backgroundColor: selectionMatch },
    '.cm-matchingBracket, .cm-nonmatchingBracket': {
      backgroundColor: 'transparent',
      borderBottom: `1px solid ${caret}`,
      color: 'inherit',
    },
    '.cm-searchMatch': {
      backgroundColor: 'rgba(229, 192, 123, 0.25)',
      outline: '1px solid rgba(229, 192, 123, 0.5)',
    },
    '.cm-searchMatch.cm-searchMatch-selected': {
      backgroundColor: 'rgba(229, 192, 123, 0.4)',
    },
    '.cm-tooltip': {
      background: bgAlt,
      border: `1px solid ${border}`,
      borderRadius: '6px',
      color: fg,
    },
    '.cm-tooltip-autocomplete > ul > li[aria-selected]': {
      background: 'rgba(108, 180, 255, 0.18)',
      color: fg,
    },
    '.cm-panels': { backgroundColor: bgAlt, color: fg },
    '.cm-panels-top': { borderBottom: `1px solid ${border}` },
    '.cm-panels-bottom': { borderTop: `1px solid ${border}` },
    '.cm-panel input, .cm-panel button': {
      background: bg,
      color: fg,
      border: `1px solid ${border}`,
      borderRadius: '4px',
      padding: '2px 6px',
    },
  },
  { dark: true },
)

const oxiHighlightStyle = HighlightStyle.define([
  { tag: t.comment, color: comment, fontStyle: 'italic' },
  { tag: [t.lineComment, t.blockComment, t.docComment], color: comment, fontStyle: 'italic' },

  { tag: [t.keyword, t.modifier, t.controlKeyword, t.operatorKeyword, t.self, t.null], color: purple },
  { tag: [t.definitionKeyword, t.moduleKeyword], color: purple },
  { tag: [t.bool, t.atom], color: orange },
  { tag: [t.number, t.integer, t.float], color: orange },

  { tag: t.string, color: green },
  { tag: [t.special(t.string), t.regexp, t.escape], color: aqua },

  { tag: [t.function(t.variableName), t.function(t.propertyName)], color: blue },
  { tag: [t.variableName, t.propertyName], color: fg },
  { tag: [t.definition(t.variableName), t.definition(t.propertyName)], color: fg },

  { tag: [t.typeName, t.className, t.namespace], color: yellow },
  { tag: [t.tagName, t.angleBracket], color: red },
  { tag: t.attributeName, color: orange },
  { tag: t.attributeValue, color: green },

  { tag: [t.operator, t.punctuation, t.separator, t.bracket, t.brace, t.paren], color: '#abb2bf' },
  { tag: [t.meta, t.processingInstruction], color: comment },

  { tag: t.heading, color: blue, fontWeight: 'bold' },
  { tag: [t.link, t.url], color: blue, textDecoration: 'underline' },
  { tag: t.emphasis, fontStyle: 'italic' },
  { tag: t.strong, fontWeight: 'bold' },
  { tag: t.strikethrough, textDecoration: 'line-through' },

  { tag: t.invalid, color: red, textDecoration: 'underline' },
])

export const oxiHighlighting: Extension = syntaxHighlighting(oxiHighlightStyle, { fallback: true })
