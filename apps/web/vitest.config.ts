import { defineConfig } from 'vitest/config'

// Pure-logic web unit tests. Keep this lean: heavy interaction tests live in
// Playwright (tests/e2e/*). happy-dom is here for any module that touches
// `window` (zustand subscribers, etc.) — most tests should not need it.
export default defineConfig({
  test: {
    environment: 'happy-dom',
    include: ['src/**/*.test.ts', 'src/**/*.test.tsx'],
    globals: false,
  },
})
