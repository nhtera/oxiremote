// Keyboard shortcut definitions for the workspace shortcut overlay.
// Plain data — no logic. Imported by keyboard-shortcut-overlay.tsx.

export type Shortcut = {
  key: string
  action: string
  category: 'terminal' | 'workspace'
}

export const WORKSPACE_SHORTCUTS: Shortcut[] = [
  // Terminal hotkeys
  { key: 'Ctrl + C', action: 'Interrupt / cancel process', category: 'terminal' },
  { key: 'Ctrl + D', action: 'Send EOF / close shell', category: 'terminal' },
  { key: 'Ctrl + Z', action: 'Suspend process to background', category: 'terminal' },
  { key: 'Ctrl + L', action: 'Clear terminal screen', category: 'terminal' },
  { key: 'Ctrl + A', action: 'Move cursor to line start', category: 'terminal' },
  { key: 'Ctrl + E', action: 'Move cursor to line end', category: 'terminal' },
  { key: 'Ctrl + U', action: 'Delete to line start', category: 'terminal' },
  { key: 'Ctrl + K', action: 'Delete to line end', category: 'terminal' },
  { key: 'Ctrl + R', action: 'Reverse history search', category: 'terminal' },
  { key: 'Ctrl + W', action: 'Delete previous word', category: 'terminal' },
  // Workspace hotkeys
  { key: '?', action: 'Toggle shortcut overlay', category: 'workspace' },
  { key: 'Tab bar + click', action: 'Switch terminal session', category: 'workspace' },
  { key: '1-up / 2-up / 3-up', action: 'Set pane split layout', category: 'workspace' },
  { key: 'Paste (keybar)', action: 'Paste clipboard to terminal (mobile)', category: 'workspace' },
  { key: 'Esc (overlay)', action: 'Close this overlay', category: 'workspace' },
]
