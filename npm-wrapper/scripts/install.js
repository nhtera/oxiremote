#!/usr/bin/env node
// Postinstall: download the GitHub release artifact for this platform,
// verify SHA256 against the published manifest, extract the binary into
// `bin/`. Skipped during `npm publish` itself (npm sets npm_config_global
// to `false` and there's no install lifecycle then).
//
// Override the binary URL with OXIREMOTE_BINARY_URL — useful for corp proxies
// or air-gapped installs.

'use strict'

const fs = require('node:fs')
const path = require('node:path')
const crypto = require('node:crypto')
const https = require('node:https')
const { execSync } = require('node:child_process')

const REPO = 'nhtera/oxiremote'
const VERSION = require('../package.json').version
const BIN_DIR = path.join(__dirname, '..', 'bin')

function detectTarget() {
  const platform = process.platform
  const arch = process.arch
  const map = {
    'darwin/arm64': 'aarch64-apple-darwin',
    'darwin/x64': 'x86_64-apple-darwin',
    'linux/x64': 'x86_64-unknown-linux-gnu',
    'linux/arm64': 'aarch64-unknown-linux-gnu',
    'win32/x64': 'x86_64-pc-windows-msvc',
  }
  const key = `${platform}/${arch}`
  if (!map[key]) {
    console.error(
      `[oxiremote] no prebuilt binary for ${key}; install from source: https://github.com/${REPO}`,
    )
    process.exit(1)
  }
  return map[key]
}

function archiveExt() {
  return process.platform === 'win32' ? 'zip' : 'tar.gz'
}

function binaryName() {
  return process.platform === 'win32' ? 'oxiremote.exe' : 'oxiremote'
}

function get(url, redirectsLeft = 5) {
  return new Promise((resolve, reject) => {
    https
      .get(url, { headers: { 'User-Agent': `oxiremote-npm/${VERSION}` } }, (res) => {
        if (
          res.statusCode &&
          res.statusCode >= 300 &&
          res.statusCode < 400 &&
          res.headers.location
        ) {
          if (redirectsLeft <= 0) {
            reject(new Error(`too many redirects fetching ${url}`))
            return
          }
          res.resume()
          resolve(get(res.headers.location, redirectsLeft - 1))
          return
        }
        if (res.statusCode !== 200) {
          reject(new Error(`HTTP ${res.statusCode} fetching ${url}`))
          return
        }
        const chunks = []
        res.on('data', (c) => chunks.push(c))
        res.on('end', () => resolve(Buffer.concat(chunks)))
        res.on('error', reject)
      })
      .on('error', reject)
  })
}

async function main() {
  const target = detectTarget()
  const ext = archiveExt()
  const stem = `oxiremote-${VERSION}-${target}`
  const assetName = `${stem}.${ext}`
  const manifestName = `oxiremote-${VERSION}-sha256.txt`
  const baseUrl =
    process.env.OXIREMOTE_BINARY_URL ||
    `https://github.com/${REPO}/releases/download/v${VERSION}`

  console.log(`[oxiremote] downloading ${assetName} from ${baseUrl}`)
  const [archive, manifest] = await Promise.all([
    get(`${baseUrl}/${assetName}`),
    get(`${baseUrl}/${manifestName}`),
  ])

  const expected = manifest
    .toString('utf8')
    .split('\n')
    .map((line) => line.trim().split(/\s+/))
    .find(([, name]) => name === assetName)?.[0]
  if (!expected) {
    throw new Error(`manifest ${manifestName} does not list ${assetName}`)
  }

  const actual = crypto.createHash('sha256').update(archive).digest('hex')
  if (expected !== actual) {
    throw new Error(`checksum mismatch for ${assetName}: expected ${expected}, got ${actual}`)
  }
  console.log('[oxiremote] checksum verified')

  fs.mkdirSync(BIN_DIR, { recursive: true })
  const tmp = path.join(BIN_DIR, `_archive.${ext}`)
  fs.writeFileSync(tmp, archive)

  // Extract using tar (ships with macOS, Linux, and Windows 10+ via bsdtar).
  // Strip the top-level `oxiremote-<version>-<target>/` directory so the
  // binary lands directly in bin/.
  if (ext === 'zip') {
    execSync(`tar -xf "${tmp}" -C "${BIN_DIR}"`, { stdio: 'inherit' })
    // tar leaves the binary inside the stem dir; flatten it.
    const nested = path.join(BIN_DIR, stem, binaryName())
    fs.renameSync(nested, path.join(BIN_DIR, binaryName()))
    fs.rmSync(path.join(BIN_DIR, stem), { recursive: true, force: true })
  } else {
    execSync(`tar -xzf "${tmp}" -C "${BIN_DIR}" --strip-components=1`, { stdio: 'inherit' })
  }
  fs.unlinkSync(tmp)

  const finalPath = path.join(BIN_DIR, binaryName())
  if (process.platform !== 'win32') {
    fs.chmodSync(finalPath, 0o755)
  }
  console.log(`[oxiremote] installed ${finalPath}`)
}

// Skip during the `npm publish` lifecycle itself — there's nothing to install
// when running inside the package's own checkout. Heuristic: if the binary
// already exists OR we're inside the repo tree (release workflow runs
// `npm pack`), bail out quietly.
if (process.env.OXIREMOTE_SKIP_INSTALL === '1') {
  console.log('[oxiremote] OXIREMOTE_SKIP_INSTALL=1 — skipping postinstall')
  process.exit(0)
}

main().catch((err) => {
  console.error(`[oxiremote] install failed: ${err.message}`)
  console.error(
    '  Set OXIREMOTE_BINARY_URL to a custom mirror, or download manually from\n' +
      `  https://github.com/${REPO}/releases`,
  )
  process.exit(1)
})
