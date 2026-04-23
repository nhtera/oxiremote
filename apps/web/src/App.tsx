import { useEffect } from 'react'
import { Navigate, Route, Routes, useParams } from 'react-router-dom'
import AppLayout from './components/app-layout'
import HomePage from './pages/home-page'
import LoginPage from './pages/login-page'
import TerminalPage from './pages/terminal-page'
import GitPage from './pages/git-page'
import FilesPage from './pages/files-page'
import PreviewPage from './pages/preview-page'
import { useHostStore } from './state/host-store'

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

// Wrapper that injects :hostId param into page context if needed in future
function HostRoute({ children }: { children: React.ReactNode }) {
  // hostId available via useParams() in child components if needed
  const { hostId } = useParams<{ hostId: string }>()
  void hostId
  return <>{children}</>
}

function App() {
  const fetchHost = useHostStore((s) => s.fetchHost)

  // Fetch host info once on mount; 401 is handled inside fetchHost
  useEffect(() => {
    fetchHost()
  }, [fetchHost])

  return (
    <Routes>
      <Route path="/login" element={<LoginPage />} />

      {/* Host-scoped routes */}
      <Route element={<AppLayout />}>
        <Route path="/" element={<HomePage />} />

        <Route path="/h/:hostId" element={<HostRoute><TerminalPage /></HostRoute>} />
        <Route path="/h/:hostId/terminal" element={<HostRoute><TerminalPage /></HostRoute>} />
        <Route path="/h/:hostId/git" element={<HostRoute><GitPage /></HostRoute>} />
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
