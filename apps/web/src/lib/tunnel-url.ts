// Tunnel URL helpers. The agent stores `tunnel_url` as a bare host
// (e.g. `oxiremote.erai.dev`) — no scheme — because the cloudflared stderr
// scrape captures it that way. The SPA needs absolute URLs to use as
// link hrefs; without the scheme the browser treats them as relative
// paths against the current origin (bug seen in the wild as
// `localhost:8787/oxiremote.erai.dev/login`).

/** Coerce a tunnel host string to an absolute https URL.
 *  Pass-through if a scheme is already present; trims trailing slash. */
export function tunnelUrlAbs(hostOrUrl: string): string {
  const trimmed = hostOrUrl.replace(/\/$/, '')
  if (/^https?:\/\//i.test(trimmed)) return trimmed
  return `https://${trimmed}`
}
