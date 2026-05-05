'use strict'

const fs = require('node:fs')
const { execFileSync } = require('node:child_process')

const GLIBC_VERSION_RE = /GLIBC_(\d+)\.(\d+)(?:\.(\d+))?/g

function isLinux() {
  return process.platform === 'linux'
}

function parseVersion(text) {
  const match = String(text || '').match(/(\d+)\.(\d+)(?:\.(\d+))?/)
  if (!match) return null
  return [Number(match[1]), Number(match[2]), Number(match[3] || 0)]
}

function compareVersion(a, b) {
  for (let i = 0; i < 3; i += 1) {
    if (a[i] !== b[i]) return a[i] - b[i]
  }
  return 0
}

function formatVersion(version) {
  return version[2]
    ? `${version[0]}.${version[1]}.${version[2]}`
    : `${version[0]}.${version[1]}`
}

function detectHostGlibc() {
  try {
    const out = execFileSync('getconf', ['GNU_LIBC_VERSION'], {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'ignore'],
    })
    const version = parseVersion(out)
    if (version) return version
  } catch {
    // fall through to ldd
  }
  try {
    const out = execFileSync('ldd', ['--version'], {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'ignore'],
    })
    const firstLine = out.split('\n', 1)[0]
    const version = parseVersion(firstLine)
    if (version) return version
  } catch {
    // glibc not present (e.g. musl / Alpine)
  }
  return null
}

function detectBinaryRequiredGlibc(filePath) {
  const buf = fs.readFileSync(filePath)
  const text = buf.toString('latin1')
  let highest = null
  GLIBC_VERSION_RE.lastIndex = 0
  let match
  while ((match = GLIBC_VERSION_RE.exec(text)) !== null) {
    const version = [Number(match[1]), Number(match[2]), Number(match[3] || 0)]
    if (!highest || compareVersion(version, highest) > 0) {
      highest = version
    }
  }
  return highest
}

function buildFromSourceHint() {
  return [
    'You can still install OxiRemote by building from source with Cargo:',
    '',
    '  # Requires Rust 1.83+ (https://rustup.rs)',
    '  git clone https://github.com/nhtera/oxiremote.git',
    '  cd oxiremote',
    '  bun install && bun run build:release',
    '  # Binary lands at agent/target/release/oxiremote',
    '',
    'See https://github.com/nhtera/oxiremote#readme for build prerequisites',
    '(Linux desktop pipeline needs libxdo, libgbm, libpipewire-0.3 dev headers).',
  ].join('\n')
}

function preflightGlibc(filePath) {
  if (!isLinux()) return
  if (process.env.OXIREMOTE_SKIP_GLIBC_CHECK === '1') return

  const required = detectBinaryRequiredGlibc(filePath)
  if (!required) {
    // Statically linked / musl binary, or no GLIBC_* version dependencies present.
    return
  }

  const host = detectHostGlibc()
  if (!host) {
    throw new Error(
      [
        `The prebuilt binary requires GLIBC_${formatVersion(required)}, but no GNU libc was detected on this host.`,
        "This usually means you're on a musl-based distro such as Alpine.",
        '',
        buildFromSourceHint(),
        '',
        'Set OXIREMOTE_SKIP_GLIBC_CHECK=1 to bypass this check at your own risk.',
      ].join('\n'),
    )
  }

  if (compareVersion(host, required) < 0) {
    throw new Error(
      [
        `Prebuilt OxiRemote binary requires GLIBC_${formatVersion(required)} but this system has glibc ${formatVersion(host)}.`,
        'Older distros (CentOS 7/8, RHEL 7/8, Debian 10, Ubuntu 20.04, etc.) ship an older glibc that is not compatible with the prebuilt artifact.',
        '',
        buildFromSourceHint(),
        '',
        'Set OXIREMOTE_SKIP_GLIBC_CHECK=1 to bypass this check at your own risk.',
      ].join('\n'),
    )
  }
}

module.exports = {
  preflightGlibc,
  detectHostGlibc,
  detectBinaryRequiredGlibc,
  _internal: { parseVersion, compareVersion, formatVersion },
}
