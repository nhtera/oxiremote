'use strict'

const path = require('node:path')
const os = require('node:os')

const CHECKSUM_MANIFEST_TEMPLATE = (version) => `oxiremote-${version}-sha256.txt`

const TARGET_MATRIX = {
  linux: {
    x64: 'x86_64-unknown-linux-gnu',
    arm64: 'aarch64-unknown-linux-gnu',
  },
  darwin: {
    arm64: 'aarch64-apple-darwin',
  },
  win32: {
    x64: 'x86_64-pc-windows-msvc',
  },
}

function archiveExt(platform = process.platform) {
  return platform === 'win32' ? 'zip' : 'tar.gz'
}

function executableName(platform = process.platform) {
  return platform === 'win32' ? 'oxiremote.exe' : 'oxiremote'
}

function archiveBasename(version, target, platform = process.platform) {
  return `oxiremote-${version}-${target}.${archiveExt(platform)}`
}

function detectTarget() {
  const platform = os.platform()
  const arch = os.arch()
  const platformTargets = TARGET_MATRIX[platform]
  if (!platformTargets) {
    const supported = Object.keys(TARGET_MATRIX)
      .map((p) => `'${p}'`)
      .join(', ')
    throw new Error(
      `Unsupported platform: ${platform}. Supported platforms: ${supported}.\n\n${unsupportedBuildHint()}`,
    )
  }
  const target = platformTargets[arch]
  if (!target) {
    const supported = Object.keys(platformTargets)
      .map((a) => `'${a}'`)
      .join(', ')
    throw new Error(
      `Unsupported architecture: ${arch} on ${platform}. Supported architectures: ${supported}.\n\n${unsupportedBuildHint()}`,
    )
  }
  return { platform, arch, target }
}

function unsupportedBuildHint() {
  return [
    'No prebuilt OxiRemote binary is available for this platform/architecture.',
    'You can still install by building from source with Cargo:',
    '',
    '  # Requires Rust 1.83+ (https://rustup.rs)',
    '  git clone https://github.com/nhtera/oxiremote.git',
    '  cd oxiremote',
    '  bun install && bun run build:release',
    '  # Binary lands at agent/target/release/oxiremote',
    '',
    'Or grab a release archive directly:',
    '',
    '  https://github.com/nhtera/oxiremote/releases',
  ].join('\n')
}

function resolveRepo() {
  return (
    process.env.OXIREMOTE_GITHUB_REPO ||
    'nhtera/oxiremote'
  )
}

function releaseBaseUrl(version, repo = resolveRepo()) {
  const override = process.env.OXIREMOTE_BINARY_URL
  if (override) {
    const trimmed = String(override).trim()
    return trimmed.endsWith('/') ? trimmed : `${trimmed}/`
  }
  return `https://github.com/${repo}/releases/download/v${version}/`
}

function releaseAssetUrl(baseName, version, repo = resolveRepo()) {
  return new URL(baseName, releaseBaseUrl(version, repo)).toString()
}

function checksumManifestName(version) {
  return CHECKSUM_MANIFEST_TEMPLATE(version)
}

function checksumManifestUrl(version, repo = resolveRepo()) {
  return releaseAssetUrl(checksumManifestName(version), version, repo)
}

function releaseBinaryDirectory() {
  return path.join(__dirname, '..', 'bin')
}

function allArchiveBasenames(version) {
  const names = []
  for (const [platform, archs] of Object.entries(TARGET_MATRIX)) {
    for (const target of Object.values(archs)) {
      names.push(archiveBasename(version, target, platform))
    }
  }
  return Array.from(new Set(names))
}

function allReleaseAssetNames(version) {
  return [...allArchiveBasenames(version), checksumManifestName(version)]
}

module.exports = {
  TARGET_MATRIX,
  detectTarget,
  archiveBasename,
  archiveExt,
  executableName,
  releaseAssetUrl,
  releaseBaseUrl,
  releaseBinaryDirectory,
  resolveRepo,
  checksumManifestName,
  checksumManifestUrl,
  allArchiveBasenames,
  allReleaseAssetNames,
  unsupportedBuildHint,
}
