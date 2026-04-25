#!/usr/bin/env node
// Tiny shim: exec the platform-specific binary that postinstall fetched.
// Stdio is inherited so the TUI / interactive prompts work transparently.

'use strict'

const path = require('node:path')
const { spawn } = require('node:child_process')

const binName = process.platform === 'win32' ? 'oxiremote.exe' : 'oxiremote'
const binPath = path.join(__dirname, binName)

const child = spawn(binPath, process.argv.slice(2), {
  stdio: 'inherit',
  windowsHide: false,
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
