// Translucent overlay rendered above the remote-desktop video element when
// the host Mac is on the lock screen. macOS does not allow the agent to
// capture the lock screen for security reasons (would require entitlements +
// a launchd-managed helper, deferred to plan 260512-0738), so the operator
// gets honest copy + a path to recover instead of a silent black frame.
//
// Pointer-events-none so the operator can still drag / pinch the underlying
// video container (mobile gestures stay live for things like the
// disconnect modal). No keystrokes are accepted by macOS while Secure Input
// is on anyway, so we don't need to actively block input here.

interface Props {
  /** Optional copy reminding the operator how to avoid this state. The page
   *  passes the dashboard URL when known so the link is one tap away. */
  hint?: string
}

export default function LockedOverlay({ hint }: Props) {
  return (
    <div
      className="absolute inset-0 z-20 flex flex-col items-center justify-center gap-3 bg-black/70 text-center text-white pointer-events-none px-6"
      role="status"
      aria-live="polite"
    >
      <div className="text-5xl" aria-hidden="true">🔒</div>
      <div className="text-lg font-medium">Host Mac is locked</div>
      <div className="max-w-sm text-sm opacity-80 leading-relaxed">
        OxiRemote can&apos;t show the lock screen for security reasons. Unlock
        the Mac at the device to continue, or close this session and reconnect
        with &ldquo;Keep Mac awake during sessions&rdquo; enabled.
      </div>
      {hint && <div className="text-xs opacity-60">{hint}</div>}
    </div>
  )
}
