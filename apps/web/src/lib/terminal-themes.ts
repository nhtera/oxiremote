// xterm ITheme-compatible presets
export type TerminalTheme = {
  background: string
  foreground: string
  cursor: string
  cursorAccent?: string
  selectionBackground?: string
  black: string; red: string; green: string; yellow: string
  blue: string; magenta: string; cyan: string; white: string
  brightBlack: string; brightRed: string; brightGreen: string; brightYellow: string
  brightBlue: string; brightMagenta: string; brightCyan: string; brightWhite: string
}

export const terminalThemes: Record<string, TerminalTheme> = {
  default: {
    background: '#16171d', foreground: '#f3f4f6',
    cursor: '#6cb4ff', selectionBackground: '#2a2b3580',
    black: '#1e1e2e', red: '#f87171', green: '#4ade80', yellow: '#fbbf24',
    blue: '#6cb4ff', magenta: '#c084fc', cyan: '#22d3ee', white: '#e5e7eb',
    brightBlack: '#4b5563', brightRed: '#fca5a5', brightGreen: '#86efac', brightYellow: '#fde68a',
    brightBlue: '#93c9ff', brightMagenta: '#d8b4fe', brightCyan: '#67e8f9', brightWhite: '#f9fafb',
  },
  dracula: {
    background: '#282a36', foreground: '#f8f8f2',
    cursor: '#f8f8f2', selectionBackground: '#44475a',
    black: '#21222c', red: '#ff5555', green: '#50fa7b', yellow: '#f1fa8c',
    blue: '#bd93f9', magenta: '#ff79c6', cyan: '#8be9fd', white: '#f8f8f2',
    brightBlack: '#6272a4', brightRed: '#ff6e6e', brightGreen: '#69ff94', brightYellow: '#ffffa5',
    brightBlue: '#d6acff', brightMagenta: '#ff92df', brightCyan: '#a4ffff', brightWhite: '#ffffff',
  },
  'solarized-dark': {
    background: '#002b36', foreground: '#839496',
    cursor: '#93a1a1', selectionBackground: '#073642',
    black: '#073642', red: '#dc322f', green: '#859900', yellow: '#b58900',
    blue: '#268bd2', magenta: '#d33682', cyan: '#2aa198', white: '#eee8d5',
    brightBlack: '#002b36', brightRed: '#cb4b16', brightGreen: '#586e75', brightYellow: '#657b83',
    brightBlue: '#839496', brightMagenta: '#6c71c4', brightCyan: '#93a1a1', brightWhite: '#fdf6e3',
  },
  'solarized-light': {
    background: '#fdf6e3', foreground: '#657b83',
    cursor: '#586e75', selectionBackground: '#eee8d5',
    black: '#073642', red: '#dc322f', green: '#859900', yellow: '#b58900',
    blue: '#268bd2', magenta: '#d33682', cyan: '#2aa198', white: '#eee8d5',
    brightBlack: '#002b36', brightRed: '#cb4b16', brightGreen: '#586e75', brightYellow: '#657b83',
    brightBlue: '#839496', brightMagenta: '#6c71c4', brightCyan: '#93a1a1', brightWhite: '#fdf6e3',
  },
}

const STORAGE_KEY = 'oxi.terminalTheme'

export function loadTheme(): string {
  return localStorage.getItem(STORAGE_KEY) ?? 'default'
}

export function saveTheme(key: string): void {
  localStorage.setItem(STORAGE_KEY, key)
}
