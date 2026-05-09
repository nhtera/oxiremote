import { describe, it, expect, beforeEach, afterEach } from 'vitest'
import { isAllowedTunnelHost, getNamedTunnelAllowlist } from './url-validation'

// ── isAllowedTunnelHost ──────────────────────────────────────────────────────

describe('isAllowedTunnelHost', () => {
  it('accepts a valid trycloudflare.com subdomain over https', () => {
    expect(isAllowedTunnelHost('https://abc-def-ghi.trycloudflare.com', [])).toBe(true)
  })

  it('rejects http scheme even for trycloudflare.com', () => {
    expect(isAllowedTunnelHost('http://x.trycloudflare.com', [])).toBe(false)
  })

  it('rejects a domain not in any allowlist', () => {
    expect(isAllowedTunnelHost('https://evil.com', [])).toBe(false)
  })

  it('rejects a domain not matching the named allowlist', () => {
    expect(isAllowedTunnelHost('https://evil.com', ['my-tunnel.example.com'])).toBe(false)
  })

  it('accepts exact match in named allowlist', () => {
    expect(isAllowedTunnelHost('https://my-tunnel.example.com', ['my-tunnel.example.com'])).toBe(true)
  })

  it('accepts a subdomain of a named allowlist entry', () => {
    expect(isAllowedTunnelHost('https://api.my-tunnel.example.com', ['my-tunnel.example.com'])).toBe(true)
  })

  it('rejects a malformed URL', () => {
    expect(isAllowedTunnelHost('not a url', [])).toBe(false)
  })

  it('rejects a ws:// scheme', () => {
    expect(isAllowedTunnelHost('ws://abc.trycloudflare.com', [])).toBe(false)
  })

  it('is case-insensitive for the hostname', () => {
    expect(isAllowedTunnelHost('https://ABC-DEF.trycloudflare.com', [])).toBe(true)
  })

  it('rejects a domain that merely contains the allowlist entry as a suffix trick', () => {
    // "evil-my-tunnel.example.com" should NOT match "my-tunnel.example.com"
    expect(isAllowedTunnelHost('https://evil-my-tunnel.example.com', ['my-tunnel.example.com'])).toBe(false)
  })
})

// ── getNamedTunnelAllowlist ──────────────────────────────────────────────────

describe('getNamedTunnelAllowlist', () => {
  beforeEach(() => localStorage.clear())
  afterEach(() => localStorage.clear())

  it('returns empty array when no named tunnel hostname is stored', () => {
    expect(getNamedTunnelAllowlist()).toEqual([])
  })

  it('returns the stored hostname when present', () => {
    // Simulate what storeNamedTunnelHostname + setActiveHost writes.
    localStorage.setItem('oxi_active_host', 'host1')
    localStorage.setItem('oxi_named_tunnel_hostname_host1', 'oxi.example.com')
    expect(getNamedTunnelAllowlist()).toEqual(['oxi.example.com'])
  })

  it('returns empty array when stored value is empty (Quick Tunnel)', () => {
    localStorage.setItem('oxi_active_host', 'host1')
    localStorage.setItem('oxi_named_tunnel_hostname_host1', '')
    expect(getNamedTunnelAllowlist()).toEqual([])
  })
})
