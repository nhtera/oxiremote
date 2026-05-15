// Codec capability probes for remote desktop pipeline selection.
//
// Two-layer detection:
//   1. `RTCRtpReceiver.getCapabilities('video')` — the receive-side codec
//      registry. Cheap, sync, but can overstate WebRTC RTP support on some
//      browsers (RTP-negotiable codec list ≠ decodable codec list).
//   2. SDP probe — create a throwaway peer connection, addTransceiver
//      recvonly, createOffer, then scan the offer SDP for the codec's
//      rtpmap. This matches EXACTLY what the agent's `offer_advertises_codec`
//      scan does, so SPA and agent agree on what's negotiable.
//
// The SDP probe runs once at app boot, results are cached for the lifetime
// of the page. The sync wrappers (`supports*Video`) return the cached value;
// callers must `await probeCodecSdpSupport()` before reading the sync probes
// to guarantee accurate results. `use-desktop-video-session.ts` does this
// inside its ws.onopen handler before sending `capabilitiesClient`.

/** SDP probe cache. `undefined` = not yet probed (sync probes fall back to
 *  getCapabilities). Populated by `probeCodecSdpSupport()`. */
interface SdpCodecCache {
  h264: boolean
  vp9: boolean
  av1: boolean
}
let sdpCache: SdpCodecCache | undefined
let sdpProbePromise: Promise<SdpCodecCache> | undefined

function rtpmapHasCodec(sdp: string, codecName: string): boolean {
  // Match the agent's case-insensitive `a=rtpmap:<PT> <NAME>/90000` scan.
  const re = new RegExp(`^a=rtpmap:\\d+\\s+${codecName}/90000`, 'im')
  return re.test(sdp)
}

/** Run the one-shot SDP probe. Cached for the lifetime of the page. Safe to
 *  call multiple times — concurrent calls share the same in-flight probe.
 *  Returns the cached result if already probed. */
export async function probeCodecSdpSupport(): Promise<SdpCodecCache> {
  if (sdpCache) return sdpCache
  if (sdpProbePromise) return sdpProbePromise
  if (typeof RTCPeerConnection === 'undefined') {
    sdpCache = { h264: false, vp9: false, av1: false }
    return sdpCache
  }
  sdpProbePromise = (async () => {
    const pc = new RTCPeerConnection()
    try {
      pc.addTransceiver('video', { direction: 'recvonly' })
      const offer = await pc.createOffer()
      const sdp = offer.sdp ?? ''
      const result: SdpCodecCache = {
        h264: rtpmapHasCodec(sdp, 'H264'),
        vp9: rtpmapHasCodec(sdp, 'VP9'),
        av1: rtpmapHasCodec(sdp, 'AV1'),
      }
      sdpCache = result
      return result
    } catch {
      sdpCache = { h264: false, vp9: false, av1: false }
      return sdpCache
    } finally {
      try { pc.close() } catch { /* already closed */ }
    }
  })()
  return sdpProbePromise
}

/** H.264 WebRTC receive — universal on evergreen browsers. */
export function supportsH264Video(): boolean {
  if (sdpCache) return sdpCache.h264
  return typeof RTCPeerConnection !== 'undefined' && typeof MediaStream !== 'undefined'
}

/** VP9 WebRTC receive. SDP probe preferred; getCapabilities fallback. */
export function supportsVp9Video(): boolean {
  if (sdpCache) return sdpCache.vp9
  if (typeof RTCRtpReceiver === 'undefined') return false
  const caps = RTCRtpReceiver.getCapabilities('video')
  if (!caps) return false
  return caps.codecs.some((c) => c.mimeType.toLowerCase() === 'video/vp9')
}

/** AV1 WebRTC receive. Safari has no plans for AV1 WebRTC as of 2026, so
 *  this probe is the only gate hiding AV1 from the Safari codec picker. */
export function supportsAv1Video(): boolean {
  if (sdpCache) return sdpCache.av1
  if (typeof RTCRtpReceiver === 'undefined') return false
  const caps = RTCRtpReceiver.getCapabilities('video')
  if (!caps) return false
  return caps.codecs.some((c) => c.mimeType.toLowerCase() === 'video/av1')
}
