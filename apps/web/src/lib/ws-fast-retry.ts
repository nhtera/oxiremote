// Decision predicate for WebSocket reconnect logic: when a WS closes without
// ever having opened (i.e. `onclose` / `onerror` fired before `onopen`), it's
// a handshake failure rather than a mid-session disconnect. The most common
// cause in this codebase is Cloudflare Quick Tunnel edge-stream-listener
// hiccups — the agent never sees these requests at all, but they normally
// heal on the next QUIC stream attempt within ~50-200ms. Burning one
// immediate retry per disconnect cycle masks them from the user before any
// modal or backoff timer runs.
//
// Usage pattern:
//   const everOpenedRef = useRef(false)   // reset to false in connect()
//   const fastRetryUsedRef = useRef(false) // reset to false on successful open
//   ws.onopen = () => { everOpenedRef.current = true; fastRetryUsedRef.current = false; ... }
//   ws.onclose = () => {
//     if (shouldFastRetryOnHandshakeFailure(everOpenedRef.current, fastRetryUsedRef.current)) {
//       fastRetryUsedRef.current = true
//       queueMicrotask(connect)
//       return
//     }
//     scheduleReconnect()
//   }
export function shouldFastRetryOnHandshakeFailure(
  everOpenedThisCycle: boolean,
  fastRetryUsed: boolean,
): boolean {
  return !everOpenedThisCycle && !fastRetryUsed
}
