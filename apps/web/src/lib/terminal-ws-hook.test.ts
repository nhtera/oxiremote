import { describe, it, expect } from 'vitest'
import {
  BACKOFF_MS,
  MAX_RECONNECT_ATTEMPTS,
  SLOW_BACKOFF_MS,
  backoffMsForAttempt,
  shouldFastRetryOnHandshakeFailure,
} from './terminal-ws-hook'

// The hook itself opens real WebSockets — too entangled with React lifecycle
// for unit tests. The contract worth pinning here is the two-phase backoff:
// fast ladder for transient drops (covered by the modal countdown), and a
// slow ladder for "host appears offline" that cycles indefinitely.

describe('fast-phase backoff schedule', () => {
  it('returns the configured value for each attempt within range', () => {
    expect(backoffMsForAttempt(1)).toBe(BACKOFF_MS[0])
    expect(backoffMsForAttempt(2)).toBe(BACKOFF_MS[1])
    expect(backoffMsForAttempt(BACKOFF_MS.length)).toBe(BACKOFF_MS[BACKOFF_MS.length - 1])
  })

  it('clamps below 1 to the first slot (defensive — UI never asks for 0)', () => {
    expect(backoffMsForAttempt(0)).toBe(BACKOFF_MS[0])
    expect(backoffMsForAttempt(-3)).toBe(BACKOFF_MS[0])
  })

  it('clamps past the last slot to the cap (5s ceiling)', () => {
    expect(backoffMsForAttempt(BACKOFF_MS.length + 5)).toBe(BACKOFF_MS[BACKOFF_MS.length - 1])
    expect(backoffMsForAttempt(999)).toBe(BACKOFF_MS[BACKOFF_MS.length - 1])
  })

  it('cumulative fast-phase backoff finishes within ~15s so the modal can switch to "host appears offline" promptly', () => {
    let total = 0
    for (let i = 1; i <= BACKOFF_MS.length; i++) total += backoffMsForAttempt(i)
    expect(total).toBeLessThan(15_000)
  })
})

describe('reconnect budget', () => {
  it('MAX_RECONNECT_ATTEMPTS matches the fast ladder length (drives modal progress bar)', () => {
    expect(MAX_RECONNECT_ATTEMPTS).toBe(BACKOFF_MS.length)
  })

  it('fast ladder is short enough to feel responsive on transient drops', () => {
    expect(BACKOFF_MS.length).toBeGreaterThanOrEqual(3)
    expect(BACKOFF_MS.length).toBeLessThanOrEqual(8)
  })
})

describe('handshake-failure fast retry', () => {
  // Quick Tunnels hardcode ha-connections=1; transient edge-stream-listener
  // failures surface as a WS that closes without ever opening. We retry once
  // immediately to mask these from the user — but only once per disconnect
  // cycle, so a genuinely-broken endpoint falls through to the backoff ladder.
  it('fires when the WS closed before opening and the immediate retry is unspent', () => {
    expect(shouldFastRetryOnHandshakeFailure(false, false)).toBe(true)
  })

  it('does not fire after the immediate retry has already been spent this cycle', () => {
    expect(shouldFastRetryOnHandshakeFailure(false, true)).toBe(false)
  })

  it('does not fire on a mid-session disconnect (we did open at some point)', () => {
    expect(shouldFastRetryOnHandshakeFailure(true, false)).toBe(false)
    expect(shouldFastRetryOnHandshakeFailure(true, true)).toBe(false)
  })
})

describe('slow-phase backoff schedule', () => {
  it('starts at >= the fast cap so we don\'t hammer the network when the host is offline', () => {
    expect(SLOW_BACKOFF_MS[0]).toBeGreaterThanOrEqual(BACKOFF_MS[BACKOFF_MS.length - 1])
  })

  it('caps at 30s — long enough to spare battery, short enough to feel alive', () => {
    const cap = SLOW_BACKOFF_MS[SLOW_BACKOFF_MS.length - 1]
    expect(cap).toBeGreaterThanOrEqual(15_000)
    expect(cap).toBeLessThanOrEqual(60_000)
  })

  it('is monotonically non-decreasing (no surprise speed-ups mid-cycle)', () => {
    for (let i = 1; i < SLOW_BACKOFF_MS.length; i++) {
      expect(SLOW_BACKOFF_MS[i]).toBeGreaterThanOrEqual(SLOW_BACKOFF_MS[i - 1])
    }
  })
})
