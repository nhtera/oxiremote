#!/usr/bin/env bun
/**
 * Bundle-size gate.
 *
 * Asserts the gzipped size of the initial-route JS shipped in
 * apps/web/dist/assets/ is under MAX_GZIP_BYTES. CI runs this after
 * `bun run build:web` — a regression fails the job.
 *
 * "Initial route" = every script/modulepreload referenced directly from
 * dist/index.html. Lazy-imported chunks are emitted to disk but never fetched
 * on first paint, so they don't count against the budget.
 */
import { gzipSync } from 'node:zlib'
import { readdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'

const MAX_GZIP_BYTES = 250 * 1024
// Lazy-loaded desktop page chunk must stay small — it's what phones fetch
// when they open /h/{host}/desktop. Includes canvas decoder + WebRTC glue.
const MAX_DESKTOP_CHUNK_GZIP_BYTES = 60 * 1024

const here = new URL('.', import.meta.url).pathname
const distDir = join(here, '..', 'apps', 'web', 'dist')
const htmlPath = join(distDir, 'index.html')

let html: string
try {
  html = readFileSync(htmlPath, 'utf8')
} catch (err) {
  console.error(`\nbundle-size: could not read ${htmlPath}`)
  console.error(`Run 'bun run build:web' first.\n`)
  console.error(err)
  process.exit(1)
}

// Collect every asset referenced from <script src=…>, <link href=…>.
const refs = new Set<string>()
for (const match of html.matchAll(/\b(?:src|href)=["']([^"']+\.js)["']/g)) {
  refs.add(match[1])
}

if (refs.size === 0) {
  console.error('bundle-size: no JS assets found in index.html — parse failure?')
  process.exit(1)
}

let total = 0
const rows: Array<{ name: string; raw: number; gz: number }> = []
for (const ref of refs) {
  const path = join(distDir, ref.replace(/^\//, ''))
  const raw = readFileSync(path)
  const gz = gzipSync(raw).byteLength
  total += gz
  rows.push({ name: ref, raw: raw.byteLength, gz })
}

rows.sort((a, b) => b.gz - a.gz)
console.log('bundle-size report (initial route, gzipped):')
for (const r of rows) {
  console.log(`  ${r.name.padEnd(44)} ${String(r.gz).padStart(7)} B`)
}
console.log(`\n  total: ${total} B / limit ${MAX_GZIP_BYTES} B`)

if (total > MAX_GZIP_BYTES) {
  console.error(
    `\n✖ bundle-size: initial route exceeds budget by ${total - MAX_GZIP_BYTES} bytes.`,
  )
  process.exit(1)
}

console.log('✓ bundle-size within budget')

// Secondary gate: every lazy desktop chunk (page + workers + overlays)
// must stay under 60 KB gz. We iterate instead of picking the first match
// because phase-03 added `desktop-stats-overlay-*.js`, and pre-phase-03
// the alphabetically-first chunk happened to be the heaviest one — that
// coincidence stops holding once siblings exist.
const assetsDir = join(distDir, 'assets')
const desktopChunks = readdirSync(assetsDir).filter((f) => /^desktop-.*\.js$/.test(f))
if (desktopChunks.length === 0) {
  console.warn('desktop-chunk: no desktop-*.js found in assets/ (skipping gate)')
} else {
  let anyOver = false
  for (const chunk of desktopChunks) {
    const raw = readFileSync(join(assetsDir, chunk))
    const gz = gzipSync(raw).byteLength
    const over = gz > MAX_DESKTOP_CHUNK_GZIP_BYTES
    console.log(
      `\ndesktop-chunk: ${chunk.padEnd(44)} ${String(gz).padStart(6)} B / limit ${MAX_DESKTOP_CHUNK_GZIP_BYTES} B`,
    )
    if (over) {
      console.error(`✖ exceeds budget by ${gz - MAX_DESKTOP_CHUNK_GZIP_BYTES} bytes.`)
      anyOver = true
    }
  }
  if (anyOver) process.exit(1)
  console.log('✓ all desktop chunks within budget')
}
