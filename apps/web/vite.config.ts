import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

// Read the agent version from agent/Cargo.toml so the SPA bundle ships with
// the same version string the running agent reports via /api/version. The
// SPA force-reloads on mismatch (Phase 05 / H14) — this closes the cache
// window where a stale Pages SPA could keep using the legacy
// api_key-in-subprotocol path against an updated agent.
function readAgentVersion(): string {
  try {
    const cargoToml = readFileSync(
      resolve(__dirname, '../../agent/Cargo.toml'),
      'utf8',
    )
    // Match the first `version = "x.y.z"` after `[package]`.
    const m = cargoToml.match(/^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m)
    return m ? m[1] : '0.0.0'
  } catch {
    return '0.0.0'
  }
}

export default defineConfig({
  plugins: [tailwindcss(), react()],
  define: {
    __APP_VERSION__: JSON.stringify(readAgentVersion()),
  },
  server: {
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:8787',
        changeOrigin: true,
        ws: true,
      },
      '/preview': {
        target: 'http://127.0.0.1:8787',
        changeOrigin: true,
        ws: true,
      },
      '/ws': {
        target: 'ws://127.0.0.1:8787',
        changeOrigin: true,
        ws: true,
      },
    },
  },
})
