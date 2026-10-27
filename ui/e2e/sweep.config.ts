// The visual sweep (e2e/sweep.visual.ts): every page of every edition in mock mode, compared with a baseline taken
// before a change that must not change what anyone sees, such as moving packages or swapping the primitives.
//   npm run sweep -- --update-snapshots   on the base: writes the baseline to e2e/.sweep/ (gitignored)
//   npm run sweep                         on the change: every difference fails, with a diff image in test-results/
// Separate from e2e/playwright.config.ts: it serves every edition, also those with no specs of their own, and its
// file name keeps it out of that config's `*.spec.ts` match.
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { defineConfig, devices } from '@playwright/test'
import { findEditions } from '../scripts/editions.ts'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const BASE_PORT = Number(process.env.SWEEP_PORT ?? 3947)
/** The demo script's day (e2e/demo.spec.ts): the seed and the app's clock both read as 15:00 UTC on it. */
export const SWEEP_ANCHOR = '2026-09-26'
const editions = findEditions(ROOT).map((e, i) => ({ ...e, port: BASE_PORT + i }))

export default defineConfig({
  testDir: '.',
  testMatch: 'sweep.visual.ts',
  snapshotPathTemplate: '{testDir}/.sweep/{projectName}/{arg}{ext}',
  timeout: 30 * 60_000,
  expect: {
    timeout: 20_000,
    toHaveScreenshot: { animations: 'disabled', caret: 'hide', scale: 'css' },
  },
  fullyParallel: true,
  reporter: 'list',
  use: {
    // A step that hangs fails in seconds instead of holding the run to the half-hour test timeout.
    actionTimeout: 15_000,
    navigationTimeout: 30_000,
    serviceWorkers: 'allow',
    contextOptions: { reducedMotion: 'reduce' },
    trace: 'retain-on-failure',
  },
  projects: editions.map((e) => ({
    name: e.id,
    metadata: { dir: e.dir },
    use: {
      ...devices['Desktop Chrome'],
      viewport: { width: 1280, height: 900 },
      baseURL: `http://127.0.0.1:${e.port}`,
    },
  })),
  webServer: editions.map((e) => ({
    command: `npx vite --config ${e.dir}/vite.config.ts --host 127.0.0.1 --port ${e.port} --strictPort`,
    url: `http://127.0.0.1:${e.port}`,
    cwd: ROOT,
    env: { VITE_NASIKO_SEED_ANCHOR: SWEEP_ANCHOR },
    reuseExistingServer: false,
    timeout: 120_000,
  })),
})
