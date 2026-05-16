// Cursor sideband channel — bridge for the server's `cursor` DataChannel
// (negotiated stream id 3). Decodes pose + shape JSON frames into typed
// callbacks the session hooks consume.
//
// Wire protocol (see agent/src/cursor_sideband.rs):
//   {"t":"p","x":<f32 0..1>,"y":<f32 0..1>,"id":<u64-as-number>,"h":<bool>}
//   {"t":"s","id":<u64-as-number>,"w":<u32>,"h":<u32>,"hx":<u32>,"hy":<u32>,"px":"<base64-rgba>"}
//
// Shape arrives at most once per unique cursor id; the server caches
// `sprites_sent` so it never re-emits an already-seen id during a session.

export interface CursorPose {
  x: number
  y: number
  id: number
  hidden: boolean
}

export interface CursorShape {
  id: number
  width: number
  height: number
  hotspotX: number
  hotspotY: number
  /** Top-down RGBA, tightly packed; `width * height * 4` bytes. */
  rgba: Uint8ClampedArray
}

/** Combined cursor snapshot the consumer renders. `sprite` is `null`
 *  until the first shape for `pose.id` arrives (the agent always sends a
 *  shape before its first using-pose, so this is normally non-null on
 *  the second render after a session opens). */
export interface CursorSnapshot {
  pose: CursorPose
  sprite: CursorShape | null
}

/**
 * Attach a pre-bound negotiated DataChannel (id=3, ordered) to a peer
 * connection. The internal sprite cache is hidden behind `onSnapshot`,
 * which the consumer wires straight into a React state setter — no
 * during-render ref reads required. Caller owns the returned channel;
 * closing the PC closes it.
 */
export function attachCursorChannel(
  pc: RTCPeerConnection,
  onSnapshot: (snap: CursorSnapshot | null) => void,
): RTCDataChannel {
  const dc = pc.createDataChannel('cursor', {
    ordered: true,
    negotiated: true,
    id: 3,
  })
  const sprites = new Map<number, CursorShape>()
  let currentPose: CursorPose | null = null

  dc.onmessage = (e) => {
    if (typeof e.data !== 'string') return
    let msg: unknown
    try {
      msg = JSON.parse(e.data)
    } catch {
      return
    }
    if (!msg || typeof msg !== 'object') return
    const m = msg as Record<string, unknown>
    if (m.t === 'p') {
      const pose: CursorPose = {
        x: typeof m.x === 'number' ? m.x : 0,
        y: typeof m.y === 'number' ? m.y : 0,
        id: typeof m.id === 'number' ? m.id : 0,
        hidden: m.h === true,
      }
      currentPose = pose
      const sprite = sprites.get(pose.id) ?? null
      onSnapshot({ pose, sprite })
    } else if (m.t === 's') {
      const rgba = typeof m.px === 'string' ? base64ToBytes(m.px) : null
      if (!rgba) return
      const shape: CursorShape = {
        id: typeof m.id === 'number' ? m.id : 0,
        width: typeof m.w === 'number' ? m.w : 0,
        height: typeof m.h === 'number' ? m.h : 0,
        hotspotX: typeof m.hx === 'number' ? m.hx : 0,
        hotspotY: typeof m.hy === 'number' ? m.hy : 0,
        rgba,
      }
      sprites.set(shape.id, shape)
      // If we already had a pose using this id but no sprite yet, retry now.
      if (currentPose && currentPose.id === shape.id) {
        onSnapshot({ pose: currentPose, sprite: shape })
      }
    }
  }
  return dc
}

function base64ToBytes(b64: string): Uint8ClampedArray | null {
  try {
    const bin = atob(b64)
    const out = new Uint8ClampedArray(bin.length)
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i)
    return out
  } catch {
    return null
  }
}
