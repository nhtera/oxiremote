import { useEffect, useState } from 'react'
import StatusChip from '../ui/status-chip'
import { TerminalIcon, RemoteDesktopIcon, FilesIcon } from '../icons'
import ServiceCard from './service-card'
import DesktopServiceToggle from './desktop-service-toggle'

interface Props {
  desktopEnabled: boolean
  onDesktopEnabledChange: (next: boolean) => void
}

// Three-up service strip pinned to the top of the right pane on the agent
// home. Always visible regardless of the active tab so the operator can
// see / toggle services without losing scroll.
export default function ServicesStrip({ desktopEnabled, onDesktopEnabledChange }: Props) {
  const previews = usePreviewsCount()

  return (
    <div className="grid gap-3 md:grid-cols-3">
      <ServiceCard
        icon={<TerminalIcon size={18} />}
        title="Remote Terminal"
        subtitle="Full shell, history, path inject"
        trailing={<StatusChip variant="online">On</StatusChip>}
        emphasized
      />
      <ServiceCard
        icon={<RemoteDesktopIcon size={18} />}
        title="Remote Desktop"
        subtitle="Mouse, keyboard, JPEG/H.264"
        trailing={
          <DesktopServiceToggle enabled={desktopEnabled} onChange={onDesktopEnabledChange} />
        }
      />
      <ServiceCard
        icon={<FilesIcon size={18} />}
        title="Files & Sites"
        subtitle={
          previews == null
            ? 'Browse, upload, share previews'
            : `${previews} active preview${previews === 1 ? '' : 's'}`
        }
        trailing={<StatusChip variant="online">On</StatusChip>}
      />
    </div>
  )
}

const POLL_MS = 15_000

function usePreviewsCount(): number | null {
  const [count, setCount] = useState<number | null>(null)
  useEffect(() => {
    let cancelled = false
    async function refresh() {
      try {
        const res = await fetch('/api/previews', { credentials: 'include' })
        if (!res.ok || cancelled) return
        const data = (await res.json()) as unknown[]
        if (!cancelled) setCount(Array.isArray(data) ? data.length : 0)
      } catch {
        // localhost call; rare blip — keep last value
      }
    }
    refresh()
    const id = setInterval(refresh, POLL_MS)
    return () => {
      cancelled = true
      clearInterval(id)
    }
  }, [])
  return count
}
