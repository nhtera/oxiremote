// Synchronous codec capability probes for remote desktop pipeline selection.
//
// Kept separate from use-desktop-video-session.ts so desktop-page.tsx can
// import just the detectors without pulling in the full WebRTC hook.

/** Phase-03: feature-detect H.264 WebRTC receive. All evergreen browsers
 *  (Chrome 2013+, Safari 14+, Firefox 2017+) satisfy this check. */
export function supportsH264Video(): boolean {
  return typeof RTCPeerConnection !== 'undefined' && typeof MediaStream !== 'undefined'
}

/** Phase-05: feature-detect HEVC WebRTC receive via the browser's RTP codec
 *  registry. Chrome 136+ and Safari 18+ advertise `video/H265`; Firefox omits
 *  it. `.toLowerCase()` normalises the mixed-case variants across UA strings. */
export function supportsHEVCVideo(): boolean {
  if (typeof RTCRtpReceiver === 'undefined') return false
  const caps = RTCRtpReceiver.getCapabilities('video')
  if (!caps) return false
  return caps.codecs.some((c) => c.mimeType.toLowerCase() === 'video/h265')
}
