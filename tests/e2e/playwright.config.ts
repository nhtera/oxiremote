import { defineConfig, devices } from '@playwright/test'

const PORT = Number(process.env.OXI_PORT ?? 8787)

export default defineConfig({
  testDir: '.',
  timeout: 60_000,
  fullyParallel: false,
  globalSetup: './global-setup.ts',
  reporter: process.env.CI ? 'github' : 'list',
  use: {
    baseURL: process.env.OXI_BASE_URL ?? `http://127.0.0.1:${PORT}`,
    trace: 'on-first-retry',
  },
  projects: [
    {
      name: 'iphone-14',
      use: { ...devices['iPhone 14'] },
    },
  ],
  // CI sets OXI_BASE_URL against a live agent; locally, we expect the dev
  // harness to be running via `bun dev` and skip the webServer hook. If you
  // want Playwright to boot the agent itself, set OXI_PLAYWRIGHT_BOOT=1.
  //
  // Note: the agent defaults to `Secure` cookies. Over plain HTTP loopback
  // the browser silently drops them, which makes every auth-required spec
  // 401. When booting via this hook we override the env explicitly; if you
  // run the agent yourself, start it with `OXI_SECURE_COOKIES=` (empty).
  webServer: process.env.OXI_PLAYWRIGHT_BOOT
    ? {
        command: 'cargo run --manifest-path agent/Cargo.toml --release',
        env: { OXI_SECURE_COOKIES: '' },
        url: `http://127.0.0.1:${PORT}/api/health`,
        reuseExistingServer: !process.env.CI,
        timeout: 120_000,
      }
    : undefined,
})
