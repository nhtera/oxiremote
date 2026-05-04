// Continuous marquee of platform/tool wordmarks the agent integrates with.
// Two duplicate tracks side-by-side so the slide animation loops seamlessly.
const ITEMS = [
  'macOS',
  'Linux',
  'Windows',
  'GitHub Codespaces',
  'Cloudflare Tunnels',
  'WebRTC DataChannel',
  'WebPush',
  'Argon2id',
  'xcap · Capture',
  'mozjpeg',
  'VideoToolbox · H.264',
  'OpenH264',
  'PWA · Add to Home Screen',
  'systemd / launchd',
]

export default function TrustStrip() {
  return (
    <section
      aria-label="Compatible with"
      className="relative py-10 border-y border-border-soft bg-surface-alt/30 backdrop-blur-sm overflow-hidden"
    >
      <div
        className="absolute inset-y-0 left-0 w-32 z-10 pointer-events-none"
        style={{ background: 'linear-gradient(to right, var(--color-surface) 5%, transparent)' }}
      />
      <div
        className="absolute inset-y-0 right-0 w-32 z-10 pointer-events-none"
        style={{ background: 'linear-gradient(to left, var(--color-surface) 5%, transparent)' }}
      />

      <div className="flex w-max marquee-track">
        {[0, 1].map((dup) => (
          <ul
            key={dup}
            className="flex items-center gap-12 px-6 shrink-0 font-mono text-[12.5px] tracking-[0.06em] text-text-secondary"
            aria-hidden={dup === 1}
          >
            {ITEMS.map((label) => (
              <li key={`${dup}-${label}`} className="flex items-center gap-3 whitespace-nowrap">
                <span className="w-1.5 h-1.5 rounded-full bg-accent/60" />
                {label}
              </li>
            ))}
          </ul>
        ))}
      </div>
    </section>
  )
}
