import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'

type SessionSummary = { total: number; running: number }
type GitSummary = { staged: number; changed: number }

export default function HomePage() {
  const [sessions, setSessions] = useState<SessionSummary>({ total: 0, running: 0 })
  const [git, setGit] = useState<GitSummary>({ staged: 0, changed: 0 })
  const [previews, setPreviews] = useState(0)
  const [authed, setAuthed] = useState(true)

  useEffect(() => {
    fetch('/api/terminal/sessions')
      .then((r) => (r.ok ? r.json() : Promise.reject()))
      .then((data: { status: string }[]) => {
        setSessions({
          total: data.length,
          running: data.filter((s) => s.status === 'running').length,
        })
      })
      .catch(() => setAuthed(false))

    fetch('/api/git/status')
      .then((r) => (r.ok ? r.json() : []))
      .then((entries: { index: string; working: string }[]) => {
        setGit({
          staged: entries.filter((e) => e.index !== ' ' && e.index !== '?').length,
          changed: entries.filter((e) => e.working !== ' ' || e.index === '?').length,
        })
      })
      .catch(() => {})

    fetch('/api/previews', { credentials: 'include' })
      .then((r) => (r.ok ? r.json() : []))
      .then((data: unknown[]) => setPreviews(data.length))
      .catch(() => {})
  }, [])

  if (!authed) {
    return (
      <div className="flex items-center justify-center h-full p-8">
        <div className="text-center">
          <h1 className="text-xl font-semibold mb-2">OxiRemote</h1>
          <p className="text-text-secondary text-sm mb-4">
            Not authenticated. Pair first.
          </p>
          <a href="/login" className="text-accent hover:text-accent-hover text-sm underline">
            Go to login
          </a>
        </div>
      </div>
    )
  }

  return (
    <div className="p-4 md:p-6 max-w-2xl">
      <h1 className="text-lg font-semibold mb-4">Dashboard</h1>

      <div className="grid grid-cols-2 md:grid-cols-3 gap-3 mb-6">
        <StatCard
          label="Terminal"
          value={`${sessions.running} active`}
          sub={`${sessions.total} total`}
          to="/terminal"
        />
        <StatCard
          label="Git"
          value={`${git.staged} staged`}
          sub={`${git.changed} changed`}
          to="/git"
        />
        <StatCard
          label="Previews"
          value={`${previews} active`}
          to="/preview"
        />
      </div>

      <h2 className="text-sm text-text-secondary mb-3">Quick actions</h2>
      <div className="flex flex-wrap gap-2">
        <QuickAction to="/terminal" label="New terminal" />
        <QuickAction to="/git" label="View changes" />
        <QuickAction to="/files" label="Browse files" />
        <QuickAction to="/preview" label="Add preview" />
      </div>
    </div>
  )
}

function StatCard({ label, value, sub, to }: {
  label: string
  value: string
  sub?: string
  to: string
}) {
  return (
    <Link
      to={to}
      className="block bg-surface-alt border border-border rounded-lg p-3 hover:bg-surface-hover transition-colors"
    >
      <div className="text-xs text-text-muted mb-1">{label}</div>
      <div className="text-sm font-medium text-text-primary">{value}</div>
      {sub && <div className="text-xs text-text-muted mt-0.5">{sub}</div>}
    </Link>
  )
}

function QuickAction({ to, label }: { to: string; label: string }) {
  return (
    <Link
      to={to}
      className="px-3 py-1.5 text-sm bg-surface-alt border border-border rounded-md text-text-secondary hover:text-text-primary hover:bg-surface-hover transition-colors"
    >
      {label}
    </Link>
  )
}
