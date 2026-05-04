// Small pill shown in the terminal tab bar when an AI coding agent CLI has
// been detected as the foreground process of a session (e.g. "claude").
// Dark-theme only; uses accent colour family to stand out without being noisy.

type Props = {
  agentName: string
}

const AGENT_ICONS: Record<string, string> = {
  claude: 'C',
  codex: 'X',
  cursor: 'Cr',
  opencode: 'OC',
}

export default function AgentDetectedBadge({ agentName }: Props) {
  const abbr = AGENT_ICONS[agentName] ?? agentName.slice(0, 2).toUpperCase()
  return (
    <span
      className="inline-flex items-center gap-0.5 px-1 py-px rounded text-[9px] font-semibold leading-none bg-accent/20 text-accent border border-accent/30 shrink-0"
      title={`${agentName} is running in this session`}
      aria-label={`${agentName} detected`}
    >
      <span aria-hidden="true">{abbr}</span>
    </span>
  )
}
