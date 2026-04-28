const STATIC_ALLOWED = new Set<string>([
  'https://oxiremote.app',
  'https://www.oxiremote.app',
  'https://remote.erai.dev',
  'https://www.remote.erai.dev',
  'http://localhost:5173',
  'http://localhost:4173',
])
const PAGES_DEV_SUFFIX = '.pages.dev'

export function isAllowedOrigin(origin: string | null): boolean {
  if (!origin) return false
  if (STATIC_ALLOWED.has(origin)) return true
  try {
    const u = new URL(origin)
    return u.protocol === 'https:' && u.hostname.endsWith(PAGES_DEV_SUFFIX)
  } catch {
    return false
  }
}

export function corsHeaders(origin: string | null): Record<string, string> {
  const headers: Record<string, string> = {
    'Access-Control-Allow-Methods': 'GET, POST, OPTIONS',
    'Access-Control-Allow-Headers': 'Content-Type',
    'Access-Control-Max-Age': '86400',
    Vary: 'Origin',
  }
  if (origin && isAllowedOrigin(origin)) {
    headers['Access-Control-Allow-Origin'] = origin
  }
  return headers
}

export function handlePreflight(origin: string | null): Response {
  return new Response(null, { status: 204, headers: corsHeaders(origin) })
}
