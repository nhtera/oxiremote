// Hand-rolled inline SVG icons. We render everything as `currentColor` so each
// icon inherits its surrounding text color (active-link tint, hover, etc.) and
// uses the shared stroke weight + cap/join so the set looks like a family. No
// external icon library is pulled in — the bundle stays small and tree-shakes
// per route.

import type { SVGProps } from 'react'

type IconProps = SVGProps<SVGSVGElement> & { size?: number }

function base(size: number): SVGProps<SVGSVGElement> {
  return {
    width: size,
    height: size,
    viewBox: '0 0 24 24',
    fill: 'none',
    stroke: 'currentColor',
    strokeWidth: 1.75,
    strokeLinecap: 'round',
    strokeLinejoin: 'round',
    'aria-hidden': true,
  }
}

export const HomeIcon = ({ size = 16, ...rest }: IconProps) => (
  <svg {...base(size)} {...rest}>
    <path d="M3 12l9-9 9 9" />
    <path d="M5 10v10h14V10" />
  </svg>
)

export const TerminalIcon = ({ size = 16, ...rest }: IconProps) => (
  <svg {...base(size)} {...rest}>
    <polyline points="4 6 10 12 4 18" />
    <line x1="12" y1="18" x2="20" y2="18" />
  </svg>
)

export const GitIcon = ({ size = 16, ...rest }: IconProps) => (
  <svg {...base(size)} {...rest}>
    <circle cx="6" cy="5" r="2" />
    <circle cx="6" cy="19" r="2" />
    <circle cx="18" cy="12" r="2" />
    <path d="M6 7v10" />
    <path d="M6 12h6" />
    <path d="M12 12c0-3 4-3 4-5" />
  </svg>
)

export const FilesIcon = ({ size = 16, ...rest }: IconProps) => (
  <svg {...base(size)} {...rest}>
    <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
  </svg>
)

export const PreviewIcon = ({ size = 16, ...rest }: IconProps) => (
  <svg {...base(size)} {...rest}>
    <circle cx="12" cy="12" r="9" />
    <path d="M3 12h18" />
    <path d="M12 3a14 14 0 0 1 0 18a14 14 0 0 1 0-18" />
  </svg>
)

export const DevicesIcon = ({ size = 16, ...rest }: IconProps) => (
  <svg {...base(size)} {...rest}>
    <rect x="2" y="6" width="14" height="10" rx="1" />
    <rect x="14" y="9" width="8" height="11" rx="1" />
  </svg>
)

export const LogsIcon = ({ size = 16, ...rest }: IconProps) => (
  <svg {...base(size)} {...rest}>
    <line x1="4" y1="6" x2="20" y2="6" />
    <line x1="4" y1="12" x2="20" y2="12" />
    <line x1="4" y1="18" x2="14" y2="18" />
  </svg>
)

export const SettingsIcon = ({ size = 16, ...rest }: IconProps) => (
  <svg {...base(size)} {...rest}>
    <circle cx="12" cy="12" r="3" />
    <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09a1.65 1.65 0 0 0-1-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09a1.65 1.65 0 0 0 1.51-1 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33h0a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51h0a1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82v0a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
  </svg>
)

export const RemoteDesktopIcon = ({ size = 16, ...rest }: IconProps) => (
  <svg {...base(size)} {...rest}>
    <rect x="2" y="4" width="20" height="13" rx="2" />
    <line x1="8" y1="20" x2="16" y2="20" />
    <line x1="12" y1="17" x2="12" y2="20" />
  </svg>
)

export const WifiIcon = ({ size = 16, ...rest }: IconProps) => (
  <svg {...base(size)} {...rest}>
    <path d="M2 8.82a15 15 0 0 1 20 0" />
    <path d="M5 12.86a10 10 0 0 1 14 0" />
    <path d="M8.5 16.43a5 5 0 0 1 7 0" />
    <line x1="12" y1="20" x2="12.01" y2="20" />
  </svg>
)

export const RefreshIcon = ({ size = 16, ...rest }: IconProps) => (
  <svg {...base(size)} {...rest}>
    <polyline points="23 4 23 10 17 10" />
    <polyline points="1 20 1 14 7 14" />
    <path d="M3.51 9a9 9 0 0 1 14.85-3.36L23 10" />
    <path d="M20.49 15a9 9 0 0 1-14.85 3.36L1 14" />
  </svg>
)

export const ExternalLinkIcon = ({ size = 16, ...rest }: IconProps) => (
  <svg {...base(size)} {...rest}>
    <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
    <polyline points="15 3 21 3 21 9" />
    <line x1="10" y1="14" x2="21" y2="3" />
  </svg>
)

export const PaperclipIcon = ({ size = 16, ...rest }: IconProps) => (
  <svg {...base(size)} {...rest}>
    <path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48" />
  </svg>
)

export const CopyIcon = ({ size = 16, ...rest }: IconProps) => (
  <svg {...base(size)} {...rest}>
    <rect x="9" y="9" width="13" height="13" rx="2" />
    <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
  </svg>
)

export const EyeIcon = ({ size = 16, ...rest }: IconProps) => (
  <svg {...base(size)} {...rest}>
    <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z" />
    <circle cx="12" cy="12" r="3" />
  </svg>
)

export const ChevronDownIcon = ({ size = 16, ...rest }: IconProps) => (
  <svg {...base(size)} {...rest}>
    <polyline points="6 9 12 15 18 9" />
  </svg>
)

export const EyeOffIcon = ({ size = 16, ...rest }: IconProps) => (
  <svg {...base(size)} {...rest}>
    <path d="M17.94 17.94A10.94 10.94 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94" />
    <path d="M9.9 4.24A10.94 10.94 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19" />
    <path d="M14.12 14.12A3 3 0 0 1 9.88 9.88" />
    <line x1="1" y1="1" x2="23" y2="23" />
  </svg>
)

export const XIcon = ({ size = 16, ...rest }: IconProps) => (
  <svg {...base(size)} {...rest}>
    <line x1="6" y1="6" x2="18" y2="18" />
    <line x1="18" y1="6" x2="6" y2="18" />
  </svg>
)
