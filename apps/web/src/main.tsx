import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import { BrowserRouter } from 'react-router-dom'
import App from './App.tsx'
import { installFetchInterceptor } from './lib/api-client'
import { checkAndReloadOnVersionMismatch } from './lib/version-check'

// Attach Bearer + CSRF to /api/* and /preview/* calls without per-file churn.
installFetchInterceptor()

// Phase 05 / H14: force a cache-busting reload when the bundle's embedded
// version doesn't match the running agent's `/api/version`. Closes the cache
// window where a stale Pages-cached SPA could keep using the legacy
// api_key-in-subprotocol auth path. Awaited so React never mounts on a stale
// bundle — otherwise the user sees a flash of old UI before the reload fires.
async function bootstrap() {
  if (await checkAndReloadOnVersionMismatch()) return
  createRoot(document.getElementById('root')!).render(
    <StrictMode>
      <BrowserRouter>
        <App />
      </BrowserRouter>
    </StrictMode>,
  )
}

void bootstrap()
