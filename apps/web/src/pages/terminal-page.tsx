import { Navigate, useParams } from 'react-router-dom'

// Phase 06: terminal lives inside the workspace shell now. This page is kept
// as a redirect so old deep links and notifications keep working.
export default function TerminalPage() {
  const { hostId, sessionId } = useParams<{ hostId: string; sessionId?: string }>()
  if (!hostId) return <Navigate to="/" replace />
  const target = sessionId
    ? `/h/${hostId}/workspace/${sessionId}`
    : `/h/${hostId}/workspace`
  return <Navigate to={target} replace />
}
