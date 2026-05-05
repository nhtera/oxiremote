import { create } from 'zustand'
import { isDiscoveryMode } from '../lib/discovery-client'
import { loadTunnelBase } from '../lib/api-client'

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
  } catch (e) {
    useHostStore.setState({ loading: false, error: String(e) })
  }
}
