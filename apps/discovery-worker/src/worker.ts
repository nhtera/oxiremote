import {
  getSession,
  putSession,
  putTempKey,
  resolveTempKey,
  type KVLike,
  type SessionRecord,
} from './kv-store'
import { allow as rateAllow } from './rate-limiter'
import { corsHeaders, handlePreflight, isAllowedOrigin } from './cors-handler'

interface Env {
  DISCOVERY: KVLike
}

const STATE_CHANGING_PATHS = new Set<string>([
  '/api/session/create',
  '/api/session/update',
  '/api/temp-key/create',
  '/api/code/register',
])

const HEX64 = /^[a-f0-9]{64}$/
// Pairing codes are user-typed: short, all uppercase alnum (see
// auth::PAIRING_CODE_LEN). Allow 6-16 to keep room for future tweaks.
const PAIRING_CODE = /^[A-Z0-9]{6,16}$/
const TEMP_KEY_BYTES = 16
// Pairing codes on the agent are 5-min lived; allow up to 10 here as the
// upper bound the agent may request.
const CODE_MAX_TTL_MIN = 10
const CODE_DEFAULT_TTL_MIN = 5

function jsonResponse(body: unknown, status: number, origin: string | null): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      'Content-Type': 'application/json',
      ...corsHeaders(origin),
    },
  })
}

function clientIp(req: Request): string {
  return req.headers.get('cf-connecting-ip') ?? req.headers.get('x-forwarded-for') ?? 'unknown'
}

function generateTempKey(): string {
  const bytes = new Uint8Array(TEMP_KEY_BYTES)
  crypto.getRandomValues(bytes)
  let hex = ''
  for (const b of bytes) hex += b.toString(16).padStart(2, '0')
  return hex
}

function isHexHash(v: unknown): v is string {
  return typeof v === 'string' && HEX64.test(v)
}

async function readJson<T = unknown>(req: Request): Promise<T | null> {
  try {
    return (await req.json()) as T
  } catch {
    return null
  }
}

async function handleSessionCreate(req: Request, env: Env, origin: string | null): Promise<Response> {
  const body = await readJson<{ apiKey?: unknown }>(req)
  if (!body || !isHexHash(body.apiKey)) {
    return jsonResponse({ error: 'invalid apiKey' }, 400, origin)
  }
  const existing = await getSession(env.DISCOVERY, body.apiKey)
  const record: SessionRecord = {
    tunnelUrl: existing?.tunnelUrl ?? '',
    localIp: existing?.localIp,
    updatedAt: Date.now(),
  }
  await putSession(env.DISCOVERY, body.apiKey, record)
  return jsonResponse({ ok: true }, 200, origin)
}

async function handleSessionUpdate(req: Request, env: Env, origin: string | null): Promise<Response> {
  const body = await readJson<{ apiKey?: unknown; tunnelUrl?: unknown; localIp?: unknown }>(req)
  if (!body || !isHexHash(body.apiKey) || typeof body.tunnelUrl !== 'string' || body.tunnelUrl.length === 0) {
    return jsonResponse({ error: 'invalid body' }, 400, origin)
  }
  const record: SessionRecord = {
    tunnelUrl: body.tunnelUrl,
    localIp: typeof body.localIp === 'string' ? body.localIp : undefined,
    updatedAt: Date.now(),
  }
  await putSession(env.DISCOVERY, body.apiKey, record)
  return jsonResponse({ ok: true }, 200, origin)
}

async function handleTempKeyCreate(req: Request, env: Env, origin: string | null): Promise<Response> {
  const body = await readJson<{ apiKey?: unknown; expiryMinutes?: unknown }>(req)
  if (!body || !isHexHash(body.apiKey)) {
    return jsonResponse({ error: 'invalid apiKey' }, 400, origin)
  }
  const expiryMins =
    typeof body.expiryMinutes === 'number' && body.expiryMinutes > 0 && body.expiryMinutes <= 60
      ? Math.floor(body.expiryMinutes)
      : 30
  const ttlSecs = expiryMins * 60
  const session = await getSession(env.DISCOVERY, body.apiKey)
  if (!session) {
    return jsonResponse({ error: 'session not found' }, 404, origin)
  }
  const tempKey = generateTempKey()
  await putTempKey(env.DISCOVERY, tempKey, body.apiKey, ttlSecs)
  return jsonResponse({ tempKey, expiresAt: Date.now() + ttlSecs * 1000 }, 200, origin)
}

async function handleCodeRegister(req: Request, env: Env, origin: string | null): Promise<Response> {
  // Agent registers a user-facing pairing code (typed by the human into the
  // SPA login form) so the SPA can resolve which tunnel a given code belongs
  // to, then forward the code to that tunnel for the actual exchange. Without
  // this the cross-origin SPA has no way to route a manually-typed code.
  const body = await readJson<{ apiKey?: unknown; code?: unknown; expiryMinutes?: unknown }>(req)
  if (!body || !isHexHash(body.apiKey)) {
    return jsonResponse({ error: 'invalid apiKey' }, 400, origin)
  }
  if (typeof body.code !== 'string' || !PAIRING_CODE.test(body.code)) {
    return jsonResponse({ error: 'invalid code shape' }, 400, origin)
  }
  const expiryMins =
    typeof body.expiryMinutes === 'number' &&
    body.expiryMinutes > 0 &&
    body.expiryMinutes <= CODE_MAX_TTL_MIN
      ? Math.floor(body.expiryMinutes)
      : CODE_DEFAULT_TTL_MIN
  const ttlSecs = expiryMins * 60
  const session = await getSession(env.DISCOVERY, body.apiKey)
  if (!session) {
    return jsonResponse({ error: 'session not found' }, 404, origin)
  }
  // Reuse the temp-key index — codes are just human-friendly temp keys with
  // a tighter shape gate. Lookup via /api/session/lookup?k=<code> works
  // unchanged.
  await putTempKey(env.DISCOVERY, body.code, body.apiKey, ttlSecs)
  return jsonResponse({ ok: true, expiresAt: Date.now() + ttlSecs * 1000 }, 200, origin)
}

async function handleSessionLookup(req: Request, env: Env, origin: string | null): Promise<Response> {
  const url = new URL(req.url)
  const k = url.searchParams.get('k')
  if (!k) return jsonResponse({ error: 'missing k' }, 400, origin)
  const session = await resolveTempKey(env.DISCOVERY, k)
  if (!session || !session.tunnelUrl) {
    return jsonResponse({ error: 'not found' }, 404, origin)
  }
  return jsonResponse(
    { tunnelUrl: session.tunnelUrl, localIp: session.localIp ?? null },
    200,
    origin,
  )
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    const url = new URL(req.url)
    const origin = req.headers.get('origin')

    if (req.method === 'OPTIONS') return handlePreflight(origin)

    if (STATE_CHANGING_PATHS.has(url.pathname)) {
      if (origin !== null && !isAllowedOrigin(origin)) {
        return new Response('forbidden', { status: 403 })
      }
      if (!rateAllow(clientIp(req))) {
        return jsonResponse({ error: 'rate limited' }, 429, origin)
      }
    }

    if (req.method === 'POST' && url.pathname === '/api/session/create') {
      return handleSessionCreate(req, env, origin)
    }
    if (req.method === 'POST' && url.pathname === '/api/session/update') {
      return handleSessionUpdate(req, env, origin)
    }
    if (req.method === 'POST' && url.pathname === '/api/temp-key/create') {
      return handleTempKeyCreate(req, env, origin)
    }
    if (req.method === 'POST' && url.pathname === '/api/code/register') {
      return handleCodeRegister(req, env, origin)
    }
    if (req.method === 'GET' && url.pathname === '/api/session/lookup') {
      return handleSessionLookup(req, env, origin)
    }

    return new Response('not found', { status: 404 })
  },
}
