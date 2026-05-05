#!/usr/bin/env node
'use strict'

// Local-loop smoke test for the npm wrapper. Avoids burning a real GitHub
// release tag when iterating on install.js / artifacts.js / preflight-glibc.js.
//
// What it does:
//   1. Reads the host's target triple (only this platform is testable).
//   2. Stages a release archive matching what GitHub Actions would publish:
//        oxiremote-<v>-<target>.tar.gz / .zip   (containing the local binary)
//        oxiremote-<v>-sha256.txt
//      Source binary path:
//        --binary <path>                           (explicit override)
//        agent/target/release/oxiremote(.exe)      (default — run cargo build first)
//   3. Spins up an HTTP server on 127.0.0.1 serving the staging dir.
//   4. Runs `npm-wrapper/scripts/install.js` with OXIREMOTE_BINARY_URL pointed
//      at the local server + OXIREMOTE_FORCE_DOWNLOAD=1.
//   5. Invokes the installed binary with --version and asserts it works.
//
// Usage:
//   node scripts/release/npm-wrapper-smoke.js
//   node scripts/release/npm-wrapper-smoke.js --binary path/to/oxiremote
//   PORT=8123 node scripts/release/npm-wrapper-smoke.js

const fs = require('node:fs')
const path = require('node:path')
const crypto = require('node:crypto')
const http = require('node:http')
const os = require('node:os')
const { execSync, spawn, spawnSync } = require('node:child_process')

const ROOT = path.resolve(__dirname, '..', '..')
const NPM_WRAPPER = path.join(ROOT, 'npm-wrapper')
const ARTIFACTS = require(path.join(NPM_WRAPPER, 'scripts', 'artifacts.js'))
const PKG = require(path.join(NPM_WRAPPER, 'package.json'))

function arg(name, fallback = null) {
  const ix = process.argv.indexOf(name)
  return ix >= 0 ? process.argv[ix + 1] : fallback
}

function defaultBinary() {
  const exe = ARTIFACTS.executableName()
  return path.join(ROOT, 'agent', 'target', 'release', exe)
}

function sha256(buf) {
  return crypto.createHash('sha256').update(buf).digest('hex')
}

function stageRelease(stagingDir, version, target, sourceBinary) {
  const ext = ARTIFACTS.archiveExt()
  const archiveName = ARTIFACTS.archiveBasename(version, target)
  const stem = `oxiremote-${version}-${target}`
  const stagedStem = path.join(stagingDir, stem)

  fs.mkdirSync(stagedStem, { recursive: true })
  fs.copyFileSync(
    sourceBinary,
    path.join(stagedStem, ARTIFACTS.executableName()),
  )

  const archivePath = path.join(stagingDir, archiveName)
  if (ext === 'zip') {
    execSync(`zip -j -X "${archivePath}" "${path.join(stagedStem, ARTIFACTS.executableName())}"`, {
      stdio: 'inherit',
    })
  } else {
    execSync(`tar -czf "${archivePath}" -C "${stagingDir}" "${stem}"`, {
      stdio: 'inherit',
    })
  }
  fs.rmSync(stagedStem, { recursive: true, force: true })

  const archiveBuf = fs.readFileSync(archivePath)
  const manifestName = ARTIFACTS.checksumManifestName(version)
  const manifestPath = path.join(stagingDir, manifestName)
  fs.writeFileSync(manifestPath, `${sha256(archiveBuf)}  ${archiveName}\n`)
  return { archiveName, manifestName }
}

function serve(stagingDir, port) {
  const realStagingDir = fs.realpathSync(stagingDir)
  return new Promise((resolve) => {
    const server = http.createServer((req, res) => {
      const safe = req.url.replace(/[?#].*$/, '').replace(/\.\.+/g, '')
      const file = path.join(realStagingDir, safe)
      if (!file.startsWith(realStagingDir) || !fs.existsSync(file)) {
        console.log(`[smoke] ${req.method} ${req.url} -> 404`)
        res.writeHead(404).end('not found')
        return
      }
      const stat = fs.statSync(file)
      console.log(`[smoke] ${req.method} ${req.url} -> 200 (${stat.size} bytes)`)
      res.writeHead(200, {
        'content-type': 'application/octet-stream',
        'content-length': String(stat.size),
      })
      fs.createReadStream(file).pipe(res)
    })
    server.listen(port, '127.0.0.1', () => resolve(server))
  })
}

async function main() {
  const sourceBinary = arg('--binary', defaultBinary())
  if (!fs.existsSync(sourceBinary)) {
    console.error(
      `[smoke] missing binary: ${sourceBinary}\n` +
        '  build first:  cargo build --manifest-path agent/Cargo.toml --release',
    )
    process.exit(1)
  }

  const version = PKG.oxiremoteBinaryVersion || PKG.version
  const { target } = ARTIFACTS.detectTarget()
  const port = Number(process.env.PORT) || 8765
  const stagingDir = fs.mkdtempSync(path.join(os.tmpdir(), 'oxi-smoke-'))

  console.log(`[smoke] staging release at ${stagingDir}`)
  console.log(`[smoke] version=${version} target=${target}`)
  const { archiveName, manifestName } = stageRelease(stagingDir, version, target, sourceBinary)
  console.log(`[smoke] staged ${archiveName} + ${manifestName}`)

  const server = await serve(stagingDir, port)
  console.log(`[smoke] serving at http://127.0.0.1:${port}/`)

  let exitCode = 0
  try {
    const installEnv = {
      ...process.env,
      OXIREMOTE_BINARY_URL: `http://127.0.0.1:${port}/`,
      OXIREMOTE_FORCE_DOWNLOAD: '1',
      OXIREMOTE_VERSION: version,
    }
    const installStatus = await new Promise((resolve, reject) => {
      const child = spawn('node', [path.join(NPM_WRAPPER, 'scripts', 'install.js')], {
        stdio: 'inherit',
        env: installEnv,
      })
      child.on('error', reject)
      child.on('exit', (code) => resolve(code ?? 1))
    })
    if (installStatus !== 0) throw new Error(`install.js exited ${installStatus}`)

    const binPath = path.join(NPM_WRAPPER, 'bin', ARTIFACTS.executableName())
    const probe = spawnSync(binPath, ['--version'], { stdio: 'pipe', encoding: 'utf8' })
    if (probe.status !== 0) {
      throw new Error(`installed binary --version exited ${probe.status}: ${probe.stderr}`)
    }
    console.log(`[smoke] binary reports: ${probe.stdout.trim()}`)
    console.log('[smoke] OK')
  } catch (err) {
    console.error(`[smoke] FAILED: ${err.message}`)
    exitCode = 1
  } finally {
    server.close()
    fs.rmSync(stagingDir, { recursive: true, force: true })
  }
  process.exit(exitCode)
}

main()
