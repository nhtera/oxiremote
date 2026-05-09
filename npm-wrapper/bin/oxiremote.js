#!/usr/bin/env node
// Tiny shim: exec the platform-specific binary that postinstall fetched.
// Stdio is inherited so the TUI / interactive prompts work transparently.
//
// Defaults applied here match the hosted SPA at remote.erai.dev + the public
// discovery worker. They are *only* set when the env var is missing — anyone
// pointing at a self-hosted SPA / their own discovery worker can override
// them in the parent shell and the shim leaves their value alone.

'use strict'

const path = require('node:path')
const { spawn } = require('node:child_process')

// Cross-reference: these values are also baked into the Rust binary at
// agent/src/env_defaults.rs (DEFAULT_DISCOVERY_URL etc.) so non-npm install
// paths get the same defaults. If any URL changes here, update that file too.
const DEFAULTS = {
  OXI_DISCOVERY_URL: 'https://oxiremote-discovery.tienn.workers.dev',
  OXI_WEB_URL: 'https://remote.erai.dev',
  OXI_CORS_ORIGINS: 'https://remote.erai.dev',
  OXI_SECURE_COOKIES: '1',
}

const env = { ...process.env }
for (const [k, v] of Object.entries(DEFAULTS)) {
  if (env[k] === undefined || env[k] === '') env[k] = v
}

const binName = process.platform === 'win32' ? 'oxiremote.exe' : 'oxiremote'
const binPath = path.join(__dirname, binName)

const child = spawn(binPath, process.argv.slice(2), {
  stdio: 'inherit',
  windowsHide: false,
  env,
})

child.on('exit', (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal)
    return
  }
  process.exit(code ?? 0)
})

child.on('error', (err) => {
  console.error(
    `[oxiremote] failed to spawn ${binPath}: ${err.message}\n` +
      'The postinstall download may have failed; try `npm rebuild oxiremote` or reinstall.',
  )
  process.exit(1)
})
