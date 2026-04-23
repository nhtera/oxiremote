import { useEffect } from 'react'
import { Navigate, Route, Routes, useNavigate, useParams } from 'react-router-dom'
import AppLayout from './components/app-layout'
import HomePage from './pages/home-page'
import LoginPage from './pages/login-page'
import TerminalPage from './pages/terminal-page'
import GitPage from './pages/git-page'
import FilesPage from './pages/files-page'
import PreviewPage from './pages/preview-page'
import { useHostStore } from './state/host-store'
import { registerServiceWorker } from './lib/push-client'

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
    <Routes>
      <Route path="/login" element={<LoginPage />} />

      {/* Host-scoped routes */}
      <Route element={<AppLayout />}>
        <Route path="/" element={<HomePage />} />

        <Route path="/h/:hostId" element={<HostRoute><TerminalPage /></HostRoute>} />
        <Route path="/h/:hostId/terminal" element={<HostRoute><TerminalPage /></HostRoute>} />
        <Route path="/h/:hostId/terminal/:sessionId" element={<HostRoute><TerminalPage /></HostRoute>} />
        <Route path="/h/:hostId/git" element={<HostRoute><GitPage /></HostRoute>} />
        <Route path="/h/:hostId/git/diff/:filePath" element={<HostRoute><GitPage /></HostRoute>} />
        <Route path="/h/:hostId/files" element={<HostRoute><FilesPage /></HostRoute>} />
        <Route path="/h/:hostId/preview" element={<HostRoute><PreviewPage /></HostRoute>} />

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
