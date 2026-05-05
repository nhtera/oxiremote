import { create } from 'zustand'
import { isDiscoveryMode } from '../lib/discovery-client'
import { loadApiKey, loadTunnelBase } from '../lib/api-client'
import { listSavedHosts, recordSavedHost } from '../lib/saved-hosts'

type HostState = {
  currentHostId: string | null
  label: string | null
  platform: string | null
  loading: boolean
  error: string | null
  fetchHost: (expectedHostId?: string) => Promise<void>
}

// In-flight dedup: only one /api/host probe runs at a time. Concurrent
// callers attach to the existing promise so a rapid double-switch can't
// race the store into the wrong host.
let fetchHostPromise: Promise<void> | null = null

export const useHostStore = create<HostState>(() => ({
  currentHostId: null,
  label: null,
  platform: null,
  loading: false,
  error: null,

  fetchHost: (expectedHostId?: string) => {
    if (fetchHostPromise) return fetchHostPromise
    fetchHostPromise = doFetchHost(expectedHostId).finally(() => {
      fetchHostPromise = null
    })
    return fetchHostPromise
  },
}))

async function doFetchHost(expectedHostId?: string): Promise<void> {
  // In discovery mode without a paired host, /api/host has nowhere to go
  // (the SPA origin has no API). Skip the probe to avoid the router
  // redirect-loop via the SPA fallback. Once a pair completes the
  // interceptor rewrites /api/host to the tunnel base — this branch
  // unblocks itself naturally.
  if (isDiscoveryMode() && !loadTunnelBase()) {
    useHostStore.setState({ loading: false })
    return
  }
  useHostStore.setState({ loading: true, error: null })
  try {
    const res = await fetch('/api/host', { credentials: 'include' })
    if (res.status === 401) {
      useHostStore.setState({ loading: false })
      return
    }
    if (!res.ok) {
      useHostStore.setState({ loading: false, error: `Failed to fetch host (${res.status})` })
      return
    }
    const data = (await res.json()) as { host_id: string; label: string; platform: string }
    if (expectedHostId && data.host_id !== expectedHostId) {
      // Agent at this tunnel base returned a different host identity than
      // the active-host pointer expected. Should not happen if tunnel bases
      // are stored correctly — surface loudly so misroutes are diagnosable.
      console.warn(
        `[oxiremote] fetchHost mismatch: expected ${expectedHostId}, got ${data.host_id}`,
      )
      useHostStore.setState({ loading: false, error: 'Host response mismatch' })
      return
    }
    useHostStore.setState({
      loading: false,
      currentHostId: data.host_id,
      label: data.label,
      platform: data.platform,
    })
    // Self-heal saved-hosts: when a saved entry's `label` is the legacy
    // truncated `host_id.slice(0,8)` placeholder (pre-0.1.15 discovery
    // pair), refresh it to the real hostname now that we have it. Only
    // touch entries that already exist — don't materialize a saved-host
    // for a fresh same-origin tab that hasn't been paired here.
    if (data.label) {
      const existing = listSavedHosts().find((h) => h.host_id === data.host_id)
      if (existing && existing.label !== data.label) {
        recordSavedHost({
          host_id: data.host_id,
          label: data.label,
          api_key_last4: existing.api_key_last4 || (loadApiKey(data.host_id) ?? '').slice(-4),
        })
      }
    }
  } catch (e) {
    useHostStore.setState({ loading: false, error: String(e) })
  }
}
