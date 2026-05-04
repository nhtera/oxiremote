import { lazy, Suspense, useEffect } from 'react'
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

// Wrapper that validates :hostId against the currently-paired host. Deep links
// from another host's notifications land here — we show a clear message instead
// of silently serving the wrong host's data.
function HostRoute({ children }: { children: React.ReactNode }) {
  const { hostId } = useParams<{ hostId: string }>()
  const { currentHostId, loading } = useHostStore()

  if (loading) {
    return (
      <div className="flex items-center justify-center h-full text-text-muted text-sm">
        Loading…
      </div>
    )
  }
  if (!currentHostId) {
    return <Navigate to="/login" replace />
  }
  if (hostId && hostId !== currentHostId) {
    return (
      <div className="flex flex-col items-center justify-center h-full gap-2 px-6 py-12 text-center">
        <div className="text-text-primary font-medium">Host not paired here</div>
        <div className="text-text-muted text-xs max-w-md">
          This notification came from a different host ({hostId.slice(0, 8)}…) than the one this device is paired with.
          Pair with that host from this device, or open the notification on a device paired to it.
        </div>
      </div>
    )
  }
  return <>{children}</>
}

function App() {
  const fetchHost = useHostStore((s) => s.fetchHost)
  const navigate = useNavigate()

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
  useEffect(() => {
    if (!('serviceWorker' in navigator)) return
    function onMessage(e: MessageEvent) {
      const data = e.data as { type?: string; path?: string } | null
      if (data?.type === 'oxi:deep-link' && typeof data.path === 'string' && data.path.startsWith('/')) {
        navigate(data.path)
      }
    }
    navigator.serviceWorker.addEventListener('message', onMessage)
    return () => navigator.serviceWorker.removeEventListener('message', onMessage)
  }, [navigate])

  return (
    <ToastProvider>
      <ConfirmProvider>
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
