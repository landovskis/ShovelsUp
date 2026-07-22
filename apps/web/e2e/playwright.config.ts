import { defineConfig, devices } from '@playwright/test';

/**
 * REQ-011 (IMP-REQ-011-13) headless-browser system-test harness.
 *
 * fantoccini/chromedriver was evaluated first per the plan's own contract
 * and rejected outright by macOS Gatekeeper in this environment (unsigned,
 * non-notarized chromedriver binary, no interactive approval path). This
 * Playwright project is the fallback: Playwright manages its own
 * signed/notarized browser binaries (`npx playwright install chromium`),
 * which sidesteps the Gatekeeper problem entirely.
 *
 * This harness expects `shovelsup-web` to ALREADY be running and reachable
 * at `baseURL` before `npm test` runs (e.g. `cargo run -p shovelsup-web &`
 * from `apps/web/`, waited on until ready) — it deliberately does NOT use
 * Playwright's `webServer` option to auto-start the Rust server, since
 * doing so reliably requires the Postgres/Redis dev stack and migrations to
 * already be in a known-good state, which this harness cannot verify or
 * provision itself.
 */
export default defineConfig({
  testDir: './tests',
  globalSetup: './tests/global-setup.ts',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: [['list']],
  use: {
    baseURL: process.env.SHOVELSUP_BASE_URL || 'http://localhost:3000',
    trace: 'retain-on-failure',
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
});
