import type { Extension } from '@codemirror/state'

/**
 * Best-effort mapping from filename extension → dynamic import of the
 * matching CM6 language pack. All imports are lazy so the main bundle
 * never pulls in parsers it never uses.
 */
export async function languageForPath(path: string): Promise<Extension | null> {
  const ext = path.split('.').pop()?.toLowerCase() ?? ''
  switch (ext) {
    case 'ts':
    case 'tsx': {
      const { javascript } = await import('@codemirror/lang-javascript')
      return javascript({ jsx: ext === 'tsx', typescript: true })
    }
    case 'js':
    case 'jsx':
    case 'mjs':
    case 'cjs': {
      const { javascript } = await import('@codemirror/lang-javascript')
      return javascript({ jsx: ext === 'jsx' })
    }
    case 'json': {
      const { json } = await import('@codemirror/lang-json')
      return json()
    }
    case 'html':
    case 'htm': {
      const { html } = await import('@codemirror/lang-html')
      return html()
    }
    case 'css':
    case 'scss': {
      const { css } = await import('@codemirror/lang-css')
      return css()
    }
    case 'md':
    case 'markdown': {
      const { markdown } = await import('@codemirror/lang-markdown')
      return markdown()
    }
    case 'rs': {
      const { rust } = await import('@codemirror/lang-rust')
      return rust()
    }
    case 'py': {
      const { python } = await import('@codemirror/lang-python')
      return python()
    }
    default:
      return null
  }
}
