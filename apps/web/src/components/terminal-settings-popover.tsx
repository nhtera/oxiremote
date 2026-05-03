import { terminalThemes, TERMINAL_THEME_LABELS } from '../lib/terminal-themes'
import {
  savePrefs,
  CURSOR_OPTIONS,
  FONT_SIZE_MAX,
  FONT_SIZE_MIN,
  SCROLLBACK_OPTIONS,
  type TerminalPrefs,
} from '../lib/terminal-prefs'

interface Props {
  prefs: TerminalPrefs
  onChange: (next: TerminalPrefs) => void
  onClose: () => void
}

// Floating settings popover anchored to the workspace toolbar's gear icon.
// Owns no state — every control writes through `savePrefs` (localStorage)
// and bubbles the merged result to the page so existing terminals can
// re-render with the new theme/font.
export default function TerminalSettingsPopover({ prefs, onChange, onClose }: Props) {
  const update = (patch: Partial<TerminalPrefs>) => {
    const merged = savePrefs(patch)
    onChange(merged)
  }
  return (
    <div className="absolute top-10 right-0 z-50 bg-surface-alt border border-border rounded-lg shadow-lg p-3 w-64 max-w-[calc(100vw-2rem)] space-y-3">
      <Section label="Theme">
        <div className="space-y-1">
          {Object.entries(terminalThemes).map(([key, t]) => (
            <button
              key={key}
              onClick={() => update({ theme: key })}
              className={`w-full text-left text-xs px-2 py-1.5 rounded transition-colors flex items-center gap-2 ${
                prefs.theme === key ? 'bg-accent/15 text-accent' : 'text-text-secondary hover:bg-surface-hover'
              }`}
            >
              <span
                className="w-4 h-4 rounded border border-border shrink-0"
                aria-hidden
                style={{ background: t.background, borderColor: t.foreground }}
              />
              <span className="truncate">{TERMINAL_THEME_LABELS[key] ?? key}</span>
            </button>
          ))}
        </div>
      </Section>
      <Section label="Font size">
        <div className="flex items-center gap-2">
          <input
            type="range"
            min={FONT_SIZE_MIN}
            max={FONT_SIZE_MAX}
            value={prefs.fontSize}
            onChange={(e) => update({ fontSize: Number(e.target.value) })}
            className="flex-1"
            aria-label="Font size"
          />
          <span className="text-xs text-text-muted w-8 text-right">{prefs.fontSize}px</span>
        </div>
      </Section>
      <Section label="Scrollback">
        <select
          value={prefs.scrollback}
          onChange={(e) => update({ scrollback: Number(e.target.value) })}
          className="w-full text-xs bg-surface border border-border rounded px-2 py-1 outline-none focus:border-accent/60"
        >
          {SCROLLBACK_OPTIONS.map((n) => (
            <option key={n} value={n}>{n.toLocaleString()} lines</option>
          ))}
        </select>
        <div className="text-[10px] text-text-muted mt-1">Applies to new sessions.</div>
      </Section>
      <Section label="Cursor">
        <div className="flex gap-1">
          {CURSOR_OPTIONS.map((opt) => (
            <button
              key={opt}
              onClick={() => update({ cursorStyle: opt })}
              className={`flex-1 text-xs px-2 py-1 rounded border transition-colors ${
                prefs.cursorStyle === opt
                  ? 'bg-accent/15 text-accent border-accent/40'
                  : 'bg-surface text-text-secondary border-border hover:bg-surface-hover'
              }`}
            >
              {opt}
            </button>
          ))}
        </div>
      </Section>
      <Section label="Selection">
        <label className="flex items-center gap-2 text-xs text-text-secondary cursor-pointer select-none">
          <input
            type="checkbox"
            checked={prefs.copyOnSelect}
            onChange={(e) => update({ copyOnSelect: e.target.checked })}
            className="accent-accent"
          />
          Copy on select
        </label>
        <div className="text-[10px] text-text-muted mt-1">Auto-copies highlighted text to the clipboard.</div>
      </Section>
      <button
        onClick={onClose}
        className="w-full text-xs px-2 py-1 rounded bg-surface text-text-muted hover:bg-surface-hover hover:text-text-primary border border-border transition-colors"
      >
        Done
      </button>
    </div>
  )
}

function Section({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div>
      <div className="text-[10px] font-medium uppercase tracking-wide text-text-muted mb-1.5">
        {label}
      </div>
      {children}
    </div>
  )
}
