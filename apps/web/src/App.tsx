import { lazy, Suspense, useEffect, useRef, useState } from 'react'
import { Navigate, Route, Routes, useNavigate, useParams } from 'react-router-dom'
import AppLayout from './components/app-layout'
import AgentLayout from './components/agent-layout'
import DashboardPage from './pages/dashboard-page'
import LoginPage from './pages/login-page'
import ApprovalWaitingPage from './pages/approval-waiting-page'
import WelcomePage from './pages/welcome-page'
import TerminalPage from './pages/terminal-page'
import WorkspacePage from './pages/workspace-page'
import GitPage from './pages/git-page'
import FilesPage from './pages/files-page'
import WorkspacePickerPage from './pages/workspace-picker-page'
import PreviewPage from './pages/preview-page'
import HostDevicesPage from './pages/host-devices-page'
import HostLogsPage from './pages/host-logs-page'
import { ToastProvider, ConfirmProvider } from './components/ui'
import { useHostStore } from './state/host-store'
import { registerServiceWorker } from './lib/push-client'
import { getActiveHost, loadApiKey, loadTunnelBase, storeTunnelBase } from './lib/api-client'
import { switchActiveHost } from './lib/host-switch-helpers'
import { listSavedHosts } from './lib/saved-hosts'
import { useTunnelUrlSse } from './hooks/use-tunnel-url-sse'
import { isDiscoveryMode, getCurrentTunnelUrl } from './lib/discovery-client'
import { isAllowedTunnelHost, getNamedTunnelAllowlist } from './lib/url-validation'
import { HostMovedToast } from './components/host-moved-toast'

// Agent dashboard is localhost-only. Lazy-loaded so it never enters the
// tunnel-facing bundle for devices that can't reach these routes anyway.
const AgentHomePage = lazy(() => import('./pages/agent/agent-home-page'))
const AgentDevicesPage = lazy(() => import('./pages/agent/agent-devices-page'))
const AgentSettingsPage = lazy(() => import('./pages/agent/agent-settings-page'))
const AgentLogsPage = lazy(() => import('./pages/agent/agent-logs-page'))

// Lazy-loaded remote desktop page — heavy canvas worker kept in its own chunk.
const DesktopPage = lazy(() => import('./pages/desktop-page'))

function AgentFallback() {
  return (
    <div className="flex items-center justify-center h-full text-text-muted text-sm p-6">
      Loading…
    </div>
  )
}

// Redirects legacy paths (no hostId) to /h/:currentHostId/<page>
function LegacyRedirect({ page }: { page: string }) {
  const { currentHostId, loading } = useHostStore()

  if (loading) {
    return (
      <div className="flex items-center justify-center h-full text-text-muted text-sm">
        Loading…
      </div>
    )
  }

  if (!currentHostId) {
    // fetchHost returned without auth → login
    return <Navigate to="/login" replace />
  }

  return <Navigate to={`/h/${currentHostId}/${page}`} replace />
}

// Bare /h/:hostId → workspace landing.
function WorkspaceRedirect() {
  const { hostId } = useParams<{ hostId: string }>()
  return <Navigate to={`/h/${hostId}/workspace`} replace />
}

// Wrapper that validates :hostId against the currently-paired host. When the
// URL hostId is in saved-hosts AND has stored credentials + tunnel base, we
// auto-initiate a host switch (covers push deep-links from another host).
// When the host is genuinely unknown, we show the "not paired here" wall.
function HostRoute({ children }: { children: React.ReactNode }) {
  const { hostId } = useParams<{ hostId: string }>()
  const { currentHostId, loading } = useHostStore()
  const navigate = useNavigate()
  const [switching, setSwitching] = useState(false)
  const [switchError, setSwitchError] = useState<string | null>(null)
  // Track which hostId we have already attempted on this navigation. Without
  // this, a failed switch sets switchError, which re-renders, which re-fires
  // the effect, which triggers another switch — infinite loop. The ref
  // resets only when the URL hostId changes (different deep-link).
  const attemptedRef = useRef<string | null>(null)

  useEffect(() => {
    if (!hostId) return
    // New hostId in URL → fresh attempt window.
    if (attemptedRef.current !== hostId) {
      attemptedRef.current = null
    }
    if (hostId === currentHostId || loading || switching) return
    if (attemptedRef.current === hostId) return
    if (!loadApiKey(hostId) || !loadTunnelBase(hostId)) return
    // If the active-host pointer already matches the URL hostId, an external
    // caller (topbar dropdown, saved-hosts panel, login pair flow) flipped
    // the pointer and navigated us here — they're already running fetch
    // /api/me + fetchHost. Starting our own parallel switch duplicates every
    // probe and races on the active-host pointer; with a slow tunnel + the
    // API rate-limit (`429 Too Many Requests`) the doubling cascades. Trust
    // the external caller and wait for currentHostId to converge instead.
    if (getActiveHost() === hostId) return
    attemptedRef.current = hostId
    setSwitching(true)
    setSwitchError(null)
    switchActiveHost(hostId, navigate).then((result) => {
      if (!result.ok) setSwitchError(result.error)
      setSwitching(false)
    })
  }, [hostId, currentHostId, loading, navigate, switching])

  if (loading || switching) {
    return (
      <div className="flex items-center justify-center h-full text-text-muted text-sm">
        {switching ? 'Switching host…' : 'Loading…'}
      </div>
    )
  }
  if (!currentHostId) {
    return <Navigate to="/login" replace />
  }
  if (hostId && hostId !== currentHostId) {
    const canSwitch = Boolean(loadApiKey(hostId) && loadTunnelBase(hostId))
    return (
      <div className="flex flex-col items-center justify-center h-full gap-2 px-6 py-12 text-center">
        <div className="text-text-primary font-medium">
          {switchError === 'session-expired' ? 'Session expired' : canSwitch ? 'Host not reachable' : 'Host not paired here'}
        </div>
        <div className="text-text-muted text-xs max-w-md">
          {canSwitch
            ? 'Could not switch to this host. It may be offline or the session expired.'
            : `This link is for a different host (${hostId.slice(0, 8)}…) that isn't paired on this device.`}
        </div>
        <a href="/welcome" className="text-xs text-accent underline mt-1">
          {canSwitch ? 'Re-pair this host' : 'Pair a host'}
        </a>
      </div>
    )
  }
  return <>{children}</>
}

function App() {
  const fetchHost = useHostStore((s) => s.fetchHost)
  const navigate = useNavigate()

  // Proactive tunnel-URL reconnect: listen for supervisor-emitted
  // tunnel_url_changed SSE events and broadcast oxi:tunnel-url-changed so
  // active WS hooks (terminal, desktop) can reconnect within ~3s.
  useTunnelUrlSse()

  // Discovery-mode pre-warm: SSE is same-origin only, so cross-origin SPAs
  // (Cloudflare Pages discovery deploy) miss tunnel rotations until their
  // next API call. On mount, ask the discovery worker for the current
  // tunnel URL and update the cached base before any WS upgrade fires.
  // Eliminates the cold-load stale-URL case where the agent rotated its
  // Quick Tunnel since this tab last cached it — without it, the first
  // terminal/desktop WS would fail and the reconnect modal would flash
  // during recovery.
  useEffect(() => {
    if (!isDiscoveryMode()) return
    void (async () => {
      try {
        const fresh = await getCurrentTunnelUrl()
        if (!fresh) return
        const cached = loadTunnelBase()
        if (fresh === cached) return
        if (!isAllowedTunnelHost(fresh, getNamedTunnelAllowlist())) return
        const hostId = getActiveHost()
        if (hostId) storeTunnelBase(hostId, fresh)
      } catch {
        // Discovery worker unreachable — cached base will be tried first
        // and the existing on-failure refresh in scheduleReconnect handles
        // recovery. No-op here.
      }
    })()
  }, [])

  // Fetch host info once on mount; 401 is handled inside fetchHost
  useEffect(() => {
    fetchHost()
  }, [fetchHost])

  // Register SW once on mount — safe to call even without push permission.
  useEffect(() => {
    registerServiceWorker()
  }, [])

  // SW notificationclick posts {type:'oxi:deep-link', path} when it can't
  // call client.navigate(). React-Router handles SPA navigation from here.
  // For cross-host deep-links (path /h/<other>/...) the HostRoute guard does
  // the actual switch — we only flash the document title so the user sees
  // why the page is changing. Toast UI lives below ToastProvider, but this
  // handler is registered above it; document.title is the safe fallback.
  useEffect(() => {
    if (!('serviceWorker' in navigator)) return
    function onMessage(e: MessageEvent) {
      const data = e.data as { type?: string; path?: string } | null
      if (data?.type !== 'oxi:deep-link' || typeof data.path !== 'string' || !data.path.startsWith('/')) return
      const match = data.path.match(/^\/h\/([^/]+)/)
      const pathHostId = match?.[1] ?? null
      const { currentHostId } = useHostStore.getState()
      if (pathHostId && pathHostId !== currentHostId) {
        const saved = listSavedHosts().find((h) => h.host_id === pathHostId)
        if (saved) {
          const prevTitle = document.title
          document.title = `Switching to ${saved.label}…`
          setTimeout(() => { document.title = prevTitle }, 2000)
        }
      }
      navigate(data.path)
    }
    navigator.serviceWorker.addEventListener('message', onMessage)
    return () => navigator.serviceWorker.removeEventListener('message', onMessage)
  }, [navigate])

  return (
    <ToastProvider>
      <ConfirmProvider>
        <HostMovedToast />
        <AppRoutes />
      </ConfirmProvider>
    </ToastProvider>
  )
}

// Paired devices land on the workspace shell; unpaired devices go to welcome.
// Avoids a flash of "Loading…" by waiting for the host store.
function RootRoute() {
  const { currentHostId, loading } = useHostStore()
  if (loading) {
    return (
      <div className="flex items-center justify-center h-full text-text-muted text-sm">
        Loading…
      </div>
    )
  }
  return currentHostId
    ? <Navigate to={`/h/${currentHostId}/workspace`} replace />
    : <Navigate to="/welcome" replace />
}

function AppRoutes() {
  return (
    <Routes>
      <Route path="/welcome" element={<WelcomePage />} />
      <Route path="/login" element={<LoginPage />} />
      <Route path="/approval-waiting" element={<ApprovalWaitingPage />} />

      {/* Host-local dashboard — localhost-only (enforced by agent route_scope) */}
      <Route element={<AgentLayout />}>
        <Route
          path="/agent"
          element={
            <Suspense fallback={<AgentFallback />}>
              <AgentHomePage />
            </Suspense>
          }
        />
        <Route
          path="/agent/devices"
          element={
            <Suspense fallback={<AgentFallback />}>
              <AgentDevicesPage />
            </Suspense>
          }
        />
        <Route
          path="/agent/settings"
          element={
            <Suspense fallback={<AgentFallback />}>
              <AgentSettingsPage />
            </Suspense>
          }
        />
        <Route
          path="/agent/logs"
          element={
            <Suspense fallback={<AgentFallback />}>
              <AgentLogsPage />
            </Suspense>
          }
        />
      </Route>

      {/* Host-scoped routes */}
      <Route element={<AppLayout />}>
        <Route path="/" element={<RootRoute />} />

        <Route path="/h/:hostId" element={<HostRoute><WorkspaceRedirect /></HostRoute>} />
        <Route path="/h/:hostId/workspace" element={<HostRoute><WorkspacePage /></HostRoute>} />
        <Route path="/h/:hostId/workspace/:sessionId" element={<HostRoute><WorkspacePage /></HostRoute>} />
        <Route path="/h/:hostId/dashboard" element={<HostRoute><DashboardPage /></HostRoute>} />
        {/* Legacy terminal routes — preserved so notification deep-links keep working */}
        <Route path="/h/:hostId/terminal" element={<HostRoute><TerminalPage /></HostRoute>} />
        <Route path="/h/:hostId/terminal/:sessionId" element={<HostRoute><TerminalPage /></HostRoute>} />
        <Route path="/h/:hostId/git" element={<HostRoute><GitPage /></HostRoute>} />
        <Route path="/h/:hostId/git/diff/:filePath" element={<HostRoute><GitPage /></HostRoute>} />
        <Route path="/h/:hostId/workspaces" element={<HostRoute><WorkspacePickerPage /></HostRoute>} />
        <Route path="/h/:hostId/files" element={<HostRoute><FilesPage /></HostRoute>} />
        <Route path="/h/:hostId/preview" element={<HostRoute><PreviewPage /></HostRoute>} />
        <Route path="/h/:hostId/devices" element={<HostRoute><HostDevicesPage /></HostRoute>} />
        <Route path="/h/:hostId/logs" element={<HostRoute><HostLogsPage /></HostRoute>} />
        <Route
          path="/h/:hostId/desktop"
          element={
            <HostRoute>
              <Suspense fallback={<AgentFallback />}>
                <DesktopPage />
              </Suspense>
            </HostRoute>
          }
        />

        {/* Legacy redirects */}
        <Route path="/terminal" element={<LegacyRedirect page="terminal" />} />
        <Route path="/git" element={<LegacyRedirect page="git" />} />
        <Route path="/files" element={<LegacyRedirect page="files" />} />
        <Route path="/preview" element={<LegacyRedirect page="preview" />} />
      </Route>
    </Routes>
  )
}

export default App
