#!/usr/bin/env bun
/**
 * Cross-platform launcher for `bun run dev:agent[:h264|:audio]`.
 *
 * The agent's default cargo features include `vp9` + `av1`, both of which
 * require `libvpx` / `libaom` resolved via `pkg-config`. macOS (brew) and
 * Linux (apt) install these system-wide, but a stock Windows dev box has
 * neither pkg-config nor the codec libs, so `cargo run` panics in
 * `crates/desktop/build.rs` before any source compiles.
 *
 * On Windows, drop `vp9` + `av1` from the dev feature set so iteration works
 * out of the box. Devs who actually need the SW VP9/AV1 encoders on Windows
 * can install vcpkg + pkgconfiglite per `.github/workflows/release.yml` and
 * call `cargo run` directly. macOS/Linux behavior is unchanged — we pass the
 * original feature flags through and keep the full codec mix.
 */
import { spawnSync } from 'node:child_process'
import { platform } from 'node:os'

type Profile = 'default' | 'h264' | 'audio'

const isWindows = platform() === 'win32'
const profile = (process.argv[2] ?? 'default') as Profile

// Maps each dev profile to the cargo args it appends after `cargo run
// --manifest-path agent/Cargo.toml`. Windows variants opt out of the default
// feature set and request only the audio-/h264-relevant subset.
const profiles: Record<Profile, { args: string[]; env?: Record<string, string> }> = {
  default: {
    args: isWindows ? ['--no-default-features', '--features', 'h264'] : [],
  },
  h264: {
    args: isWindows
      ? ['--no-default-features', '--features', 'h264']
      : ['--features', 'h264'],
    env: { OXI_VIDEO_PIPELINE: 'h264' },
  },
  audio: {
    args: isWindows
      ? ['--no-default-features', '--features', 'h264,audio']
      : ['--features', 'audio'],
  },
}

const selected = profiles[profile]
if (!selected) {
  console.error(`dev-cargo: unknown profile "${profile}". Use one of: default, h264, audio.`)
  process.exit(1)
}

const env = selected.env ? { ...process.env, ...selected.env } : process.env
const result = spawnSync(
  'cargo',
  ['run', '--manifest-path', 'agent/Cargo.toml', ...selected.args],
  { stdio: 'inherit', env, shell: false },
)

process.exit(result.status ?? 1)
