import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import { BrowserRouter } from 'react-router-dom'
import App from './App.tsx'
import { installFetchInterceptor, migrateAllTunnelBasesToProxy } from './lib/api-client'

// Attach Bearer + CSRF to /api/* and /preview/* calls without per-file churn.
installFetchInterceptor()

// Phase 2: opportunistically rewrite any cached `*.trycloudflare.com` tunnel
// base to the worker-proxy URL for hosts where we know the discovery_id.
// Idempotent and silent for hosts that don't need it (named tunnels,
// already-migrated bases, embedded mode). Runs at most once per page load.
migrateAllTunnelBasesToProxy()

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <BrowserRouter>
      <App />
    </BrowserRouter>
  </StrictMode>,
)
