#!/usr/bin/env node
'use strict'

// Postinstall: download the GitHub release archive for this platform,
// SHA256-verify against the published manifest, extract the binary into bin/.
// Re-running with the same version is a no-op via a `bin/oxiremote.version`
// marker file.
//
// Env overrides:
//   OXIREMOTE_BINARY_URL          Mirror base URL (e.g. corp proxy).
//   OXIREMOTE_GITHUB_REPO         Repo override (default nhtera/oxiremote).
//   OXIREMOTE_VERSION             Pin a specific binary version.
//   OXIREMOTE_DISABLE_INSTALL=1   Skip postinstall (CI / pure-JS consumers).
//   OXIREMOTE_FORCE_DOWNLOAD=1    Re-download even if marker matches.
//   OXIREMOTE_OPTIONAL_INSTALL=1  Exit 0 on failure (don't fail npm install).
//   OXIREMOTE_SKIP_INSTALL=1      Legacy alias for DISABLE_INSTALL.
//   OXIREMOTE_SKIP_GLIBC_CHECK=1  Skip glibc compat check on Linux.

const fs = require('node:fs')
const path = require('node:path')
const crypto = require('node:crypto')
const https = require('node:https')
const http = require('node:http')
const { execSync } = require('node:child_process')

const {
  archiveBasename,
  archiveExt,
  checksumManifestName,
  checksumManifestUrl,
  detectTarget,
  executableName,
  releaseAssetUrl,
  releaseBinaryDirectory,
  resolveRepo,
} = require('./artifacts')
const { preflightGlibc } = require('./preflight-glibc')
const pkg = require('../package.json')

function resolveVersion() {
  const configured =
    process.env.OXIREMOTE_VERSION ||
    pkg.oxiremoteBinaryVersion ||
    pkg.version
  return String(configured).trim()
}

function get(url, redirectsLeft = 5) {
  return new Promise((resolve, reject) => {
    const client = url.startsWith('https:') ? https : http
    client
      .get(
        url,
        { headers: { 'User-Agent': `oxiremote-npm/${pkg.version}` } },
        (res) => {
          const status = res.statusCode || 0
          if (status >= 300 && status < 400 && res.headers.location) {
            if (redirectsLeft <= 0) {
              reject(new Error(`too many redirects fetching ${url}`))
              return
            }
            res.resume()
            resolve(get(new URL(res.headers.location, url).toString(), redirectsLeft - 1))
            return
          }
          if (status !== 200) {
            res.resume()
            reject(new Error(`HTTP ${status} fetching ${url}`))
            return
          }
          const chunks = []
          res.on('data', (c) => chunks.push(c))
          res.on('end', () => resolve(Buffer.concat(chunks)))
          res.on('error', reject)
        },
      )
      .on('error', reject)
  })
}

function readMarker(file) {
  try {
    return fs.readFileSync(file, 'utf8').trim()
  } catch {
    return ''
  }
}

function isAlreadyInstalled(binDir, version) {
  if (process.env.OXIREMOTE_FORCE_DOWNLOAD === '1') return false
  const finalPath = path.join(binDir, executableName())
  if (!fs.existsSync(finalPath)) return false
  const marker = readMarker(path.join(binDir, 'oxiremote.version'))
  return marker === version
}

function parseManifest(manifestText, archiveName) {
  const expected = manifestText
    .split('\n')
    .map((line) => line.trim().split(/\s+/))
    .find(([, name]) => name === archiveName)?.[0]
  if (!expected) {
    throw new Error(`SHA256 manifest does not list ${archiveName}`)
  }
  return expected.toLowerCase()
}

async function downloadAndExtract({ version, target, repo, binDir }) {
  const ext = archiveExt()
  const archiveName = archiveBasename(version, target)
  const stem = `oxiremote-${version}-${target}`
  const archiveUrl = releaseAssetUrl(archiveName, version, repo)
  const manifestUrl = checksumManifestUrl(version, repo)

  console.log(`[oxiremote] downloading ${archiveName}`)
  const [archive, manifest] = await Promise.all([get(archiveUrl), get(manifestUrl)])

  const expected = parseManifest(manifest.toString('utf8'), archiveName)
  const actual = crypto.createHash('sha256').update(archive).digest('hex')
  if (expected !== actual) {
    throw new Error(`checksum mismatch for ${archiveName}: expected ${expected}, got ${actual}`)
  }
  console.log('[oxiremote] checksum verified')

  fs.mkdirSync(binDir, { recursive: true })
  const stagingDir = fs.mkdtempSync(path.join(binDir, '_extract-'))
  const tmp = path.join(stagingDir, `archive.${ext}`)
  fs.writeFileSync(tmp, archive)

  try {
    execSync(
      ext === 'zip'
        ? `tar -xf "${tmp}" -C "${stagingDir}"`
        : `tar -xzf "${tmp}" -C "${stagingDir}"`,
      { stdio: 'inherit' },
    )
    const extracted = path.join(stagingDir, stem, executableName())
    if (!fs.existsSync(extracted)) {
      throw new Error(`archive did not contain ${stem}/${executableName()}`)
    }
    const finalPath = path.join(binDir, executableName())
    fs.rmSync(finalPath, { force: true })
    fs.renameSync(extracted, finalPath)
  } finally {
    fs.rmSync(stagingDir, { recursive: true, force: true })
  }

  const finalPath = path.join(binDir, executableName())
  preflightGlibc(finalPath)

  if (process.platform !== 'win32') {
    fs.chmodSync(finalPath, 0o755)
  }
  fs.writeFileSync(path.join(binDir, 'oxiremote.version'), version, 'utf8')
  console.log(`[oxiremote] installed ${finalPath}`)
}

async function main() {
  if (
    process.env.OXIREMOTE_DISABLE_INSTALL === '1' ||
    process.env.OXIREMOTE_SKIP_INSTALL === '1'
  ) {
    console.log('[oxiremote] OXIREMOTE_DISABLE_INSTALL=1 — skipping postinstall')
    return
  }

  const version = resolveVersion()
  const repo = resolveRepo()
  const binDir = releaseBinaryDirectory()

  if (isAlreadyInstalled(binDir, version)) {
    console.log(`[oxiremote] v${version} already installed — skipping download`)
    return
  }

  const { target } = detectTarget()
  await downloadAndExtract({ version, target, repo, binDir })
}

main().catch((err) => {
  console.error(`[oxiremote] install failed: ${err.message}`)
  console.error(
    '  Set OXIREMOTE_BINARY_URL to a custom mirror, or download manually from\n' +
      `  https://github.com/${resolveRepo()}/releases`,
  )
  if (process.env.OXIREMOTE_OPTIONAL_INSTALL === '1') {
    console.error('  OXIREMOTE_OPTIONAL_INSTALL=1 set — exiting 0 anyway.')
    process.exit(0)
  }
  process.exit(1)
})
