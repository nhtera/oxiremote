// Terminal user preferences. Single source of truth, persisted in
// localStorage under one key. The old theme-only key (`oxi.terminalTheme`)
// is migrated lazily on first read so existing users keep their picks.

const KEY = 'oxi:terminal-prefs'
const LEGACY_THEME_KEY = 'oxi.terminalTheme'

export type CursorStyle = 'block' | 'underline' | 'bar'

export interface TerminalPrefs {
  theme: string
  fontSize: number
  scrollback: number
  cursorStyle: CursorStyle
  /** When true, mouse/touch text selection auto-copies to the clipboard.
   *  Power-users want it; casual users hate the surprise. Off by default. */
  copyOnSelect: boolean
}

export const DEFAULT_PREFS: TerminalPrefs = {
  theme: 'default',
  fontSize: 14,
  scrollback: 5000,
  cursorStyle: 'block',
  copyOnSelect: false,
}

export const FONT_SIZE_MIN = 10
export const FONT_SIZE_MAX = 20
export const SCROLLBACK_OPTIONS = [1000, 5000, 10000] as const
export const CURSOR_OPTIONS: readonly CursorStyle[] = ['block', 'underline', 'bar']

function clampFontSize(n: unknown): number {
  if (typeof n !== 'number' || !Number.isFinite(n)) return DEFAULT_PREFS.fontSize
  return Math.min(FONT_SIZE_MAX, Math.max(FONT_SIZE_MIN, Math.round(n)))
}

function pickScrollback(n: unknown): number {
  if (typeof n !== 'number' || !Number.isFinite(n)) return DEFAULT_PREFS.scrollback
  // Snap to one of the supported values rather than allow arbitrary numbers
  // — keeps the UI a select, not a free-form input.
  const closest = SCROLLBACK_OPTIONS.reduce((acc, v) =>
    Math.abs(v - n) < Math.abs(acc - n) ? v : acc, SCROLLBACK_OPTIONS[0])
  return closest
}

function pickCursorStyle(v: unknown): CursorStyle {
  return CURSOR_OPTIONS.includes(v as CursorStyle) ? (v as CursorStyle) : DEFAULT_PREFS.cursorStyle
}

export function loadPrefs(): TerminalPrefs {
  let stored: Partial<TerminalPrefs> = {}
  try {
    const raw = localStorage.getItem(KEY)
    if (raw) stored = JSON.parse(raw) as Partial<TerminalPrefs>
  } catch {
    // Ignore corrupted JSON; fall back to defaults below.
  }
  // One-shot migration: if legacy theme key exists and we don't yet have a
  // prefs object, seed the theme from it. We don't delete the legacy key so
  // a downgrade still works.
  if (!stored.theme) {
    const legacy = (() => { try { return localStorage.getItem(LEGACY_THEME_KEY) } catch { return null } })()
    if (legacy) stored = { ...stored, theme: legacy }
  }
  return {
    theme: typeof stored.theme === 'string' ? stored.theme : DEFAULT_PREFS.theme,
    fontSize: clampFontSize(stored.fontSize),
    scrollback: pickScrollback(stored.scrollback),
    cursorStyle: pickCursorStyle(stored.cursorStyle),
    copyOnSelect:
      typeof stored.copyOnSelect === 'boolean' ? stored.copyOnSelect : DEFAULT_PREFS.copyOnSelect,
  }
}

export function savePrefs(patch: Partial<TerminalPrefs>): TerminalPrefs {
  const next = { ...loadPrefs(), ...patch }
  try {
    localStorage.setItem(KEY, JSON.stringify(next))
  } catch {
    // Storage quota / private mode — preferences just won't persist.
  }
  return next
}
