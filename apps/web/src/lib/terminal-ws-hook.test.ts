import { describe, it, expect } from 'vitest'
import { BACKOFF_MS, MAX_RECONNECT_ATTEMPTS, backoffMsForAttempt } from './terminal-ws-hook'

// The hook itself opens real WebSockets — too entangled with React lifecycle
// for unit tests. The contract worth pinning here is the backoff schedule and
// the reconnect cap, since both feed UI countdowns and the reconnect modal.

describe('backoff schedule', () => {
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

  it('cumulative backoff finishes within ~30s so reconnect cap surfaces before user gives up', () => {
    let total = 0
    for (let i = 1; i <= MAX_RECONNECT_ATTEMPTS; i++) total += backoffMsForAttempt(i)
    expect(total).toBeLessThan(30_000)
  })
})

describe('reconnect cap', () => {
  it('is set above 5 (allows transient blips) but not absurdly high', () => {
    expect(MAX_RECONNECT_ATTEMPTS).toBeGreaterThan(5)
    expect(MAX_RECONNECT_ATTEMPTS).toBeLessThanOrEqual(12)
  })
})
