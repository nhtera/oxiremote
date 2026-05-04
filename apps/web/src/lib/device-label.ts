// Format helpers for device labels and relative timestamps shown in
// /agent/* operator widgets. Backend stores device labels up to 80 chars —
// either user-supplied (e.g. "Tien's iPhone") or a User-Agent fallback
// (e.g. "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/...").
// Raw UA strings are unreadable in a list, so we re-format them into a
// short "OS · Browser" cue.

const OS_PATTERNS: Array<[RegExp, string]> = [
  [/\(iPhone[;\s)]/i, 'iPhone'],
  [/\(iPad[;\s)]/i, 'iPad'],
  [/\(iPod[;\s)]/i, 'iPod'],
  [/Android/i, 'Android'],
  [/\(Macintosh[;\s)]/i, 'Mac'],
  [/Windows NT/i, 'Windows'],
  [/CrOS/i, 'ChromeOS'],
  [/X11.*Linux/i, 'Linux'],
]

// Browser detection is order-sensitive — Edge embeds Chrome, Chrome embeds
// Safari, etc. Match the most specific token first.
const BROWSER_PATTERNS: Array<[RegExp, string]> = [
  [/\bEdg(e|A|iOS)?\//i, 'Edge'],
  [/\b(OPR|Opera)\//i, 'Opera'],
  [/\bFirefox\//i, 'Firefox'],
  [/\bChrome\//i, 'Chrome'],
  [/\bSafari\//i, 'Safari'],
]

function platformFallback(platform: string | undefined): string {
  if (!platform) return ''
  const p = platform.toLowerCase()
  if (p.includes('ios') || p.includes('iphone')) return 'iPhone'
  if (p.includes('ipad')) return 'iPad'
  if (p.includes('android')) return 'Android'
  if (p.includes('mac')) return 'Mac'
  if (p.includes('win')) return 'Windows'
  if (p.includes('linux')) return 'Linux'
  return ''
}

/** Convert a raw device_label (possibly a UA fragment) into a friendly cue.
 *  Trusts user-set labels (anything not starting with "Mozilla"). For UA
 *  fragments returns "OS · Browser", "OS", or platform-only when nothing
 *  parses. Falls back to "(unnamed)" when nothing is usable. */
export function prettyDeviceLabel(raw: string, platform?: string): string {
  const label = (raw || '').trim()
  if (label && !label.startsWith('Mozilla')) {
    if (label === 'unknown') return platformFallback(platform) || 'unknown'
    return label
  }

  let os = ''
  for (const [re, name] of OS_PATTERNS) {
    if (re.test(label)) { os = name; break }
  }
  if (!os) os = platformFallback(platform)

  let browser = ''
  for (const [re, name] of BROWSER_PATTERNS) {
    if (re.test(label)) { browser = name; break }
  }

  if (os && browser) return `${os} · ${browser}`
  if (os) return os
  if (browser) return browser
  return label || '(unnamed)'
}

interface RelativeOpts {
  /** Reject impossible timestamps. Default: 1 year past, 5 minutes future. */
  maxPastMs?: number
  /** Returned when the timestamp is missing/zero or outside the sane window. */
  fallback?: string
}

const DEFAULT_MAX_PAST_MS = 365 * 24 * 3600 * 1000
const FUTURE_TOLERANCE_MS = 5 * 60 * 1000

function fmtDelta(seconds: number): string {
  if (seconds < 5) return 'just now'
  if (seconds < 60) return `${seconds}s ago`
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`
  if (seconds < 86_400) return `${Math.floor(seconds / 3600)}h ago`
  return `${Math.floor(seconds / 86_400)}d ago`
}

/** Relative time from a millisecond Unix timestamp. Returns the fallback
 *  string when the timestamp is missing, zero, in the future, or older than
 *  one year — which prevents `Date.now() - 0` from rendering as "493864h ago". */
export function relativeTimeMs(ms: number, opts: RelativeOpts = {}): string {
  const fallback = opts.fallback ?? '—'
  if (!Number.isFinite(ms) || ms <= 0) return fallback
  const now = Date.now()
  if (ms > now + FUTURE_TOLERANCE_MS) return fallback
  if (now - ms > (opts.maxPastMs ?? DEFAULT_MAX_PAST_MS)) return fallback
  return fmtDelta(Math.floor((now - ms) / 1000))
}

/** Same as `relativeTimeMs` but accepts a seconds-precision Unix timestamp. */
export function relativeTimeSec(sec: number, opts: RelativeOpts = {}): string {
  return relativeTimeMs(sec * 1000, opts)
}
