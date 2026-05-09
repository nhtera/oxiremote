// Listens for the `oxi:host-moved` browser event emitted by the transport
// retry path when the discovery fallback successfully re-resolves the tunnel
// URL. Surfaces a brief "Host moved — reconnected" toast.
//
// Must be mounted inside <ToastProvider>. The parent App already wraps all
// routes with ToastProvider, so placing this component inside the app shell
// works without any provider changes.

import { useEffect } from 'react'
import { useToast } from './ui/use-toast'
import { HOST_MOVED_EVENT } from '../lib/transport'

export function HostMovedToast() {
  const { show } = useToast()

  useEffect(() => {
    function onHostMoved() {
      show({
        kind: 'success',
        title: 'Host moved — reconnected',
        durationMs: 3000,
      })
    }
    window.addEventListener(HOST_MOVED_EVENT, onHostMoved)
    return () => window.removeEventListener(HOST_MOVED_EVENT, onHostMoved)
  }, [show])

  return null
}
