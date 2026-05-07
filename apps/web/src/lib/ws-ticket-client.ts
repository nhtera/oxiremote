/**
 * Single-use WebSocket upgrade ticket client (Phase 05 / H14).
 *
 * Replaces the legacy `oxi-bearer-v1` subprotocol that carried the long-lived
 * api_key on every WS upgrade. The SPA mints a 60s one-shot ticket via
 * `POST /api/ws-ticket` (Bearer + CSRF gated), then opens the WS with
 * `["oxi-ticket-v1", <ticket>]`. The agent atomically claims the ticket via
 * `DashMap::remove`, so a leaked ticket can be replayed at most zero times.
 *
 * Returns null on any error — callers fall back to the legacy bearer
 * subprotocol so an old agent that doesn't yet expose `/api/ws-ticket`
 * keeps working during the rollout window. After two releases the fallback
 * goes away.
 */

export const WS_TICKET_PROTOCOL = 'oxi-ticket-v1'

interface MintResponse {
  ticket: string
  expires_at: number
}

/** Mint a fresh ticket from the active agent. Returns the ticket string or
 *  null when the endpoint is unavailable / unauthenticated / network-down.
 *  Cross-origin discovery mode goes through the fetch interceptor that
 *  rewrites `/api/*` to the saved tunnel base. */
export async function mintWsTicket(): Promise<string | null> {
  try {
    // Method must be POST so csrf_guard validates the X-OXI-CSRF header on
    // tunnel-origin calls. The interceptor in api-client.ts attaches both
    // Authorization and X-OXI-CSRF automatically.
    const res = await fetch('/api/ws-ticket', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: '{}',
    })
    if (!res.ok) return null
    const body = (await res.json()) as Partial<MintResponse>
    if (typeof body.ticket !== 'string' || body.ticket.length === 0) return null
    return body.ticket
  } catch {
    return null
  }
}
