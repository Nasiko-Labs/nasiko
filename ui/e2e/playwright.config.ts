// Browser tests (plan §8 Phase 9): the demo flows in mock mode, with an axe scan on every page. One dev server per
// edition this checkout has (scripts/editions.ts): the core's specs (here) run against OSS; an edition's own specs
// live in its app dir (`<app>/e2e`) and run against that edition. Ports start at E2E_PORT (3917), so a
// `npm run dev` on :3000 (or anything else there) is never reused by accident.
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { defineConfig, devices } from '@playwright/test'
import { findEditions } from '../scripts/editions.ts'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const BASE_PORT = Number(process.env.E2E_PORT ?? 3917)
const editions = findEditions(ROOT).map((e, i) => ({
  ...e,
  url: `http://127.0.0.1:${BASE_PORT + i}`,
}))
const oss = editions.find((e) => e.id === 'oss')
if (!oss) throw new Error('No oss/ app: the core specs run against the OSS edition')

const use = {
  trace: 'retain-on-failure' as const,
  // MSW's service worker answers every /api call in mock mode.
  serviceWorkers: 'allow' as const,
  // Pages fade and rise in; axe must measure colours at rest, not mid-animation.
  contextOptions: { reducedMotion: 'reduce' as const },
}

export default defineConfig({
  // A cold dev server compiles each page (React Compiler via Babel) on its first request.
  timeout: 60_000,
  expect: { timeout: 20_000 },
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [['github'], ['html', { open: 'never' }]] : 'list',
  use,
  projects: [
    {
      name: 'desktop',
      testDir: '.',
      use: {
        ...devices['Desktop Chrome'],
        viewport: { width: 1280, height: 900 },
        baseURL: oss.url,
      },
      grepInvert: /@phone/,
    },
    {
      name: 'phone',
      testDir: '.',
      use: { ...devices['Pixel 7'], baseURL: oss.url },
      grep: /@phone/,
    },
    ...editions
      .filter((e) => e.id !== 'oss')
      .map((e) => ({
        name: e.id,
        testDir: join(ROOT, e.dir, 'e2e'),
        use: {
          ...devices['Desktop Chrome'],
          viewport: { width: 1280, height: 900 },
          baseURL: e.url,
        },
      })),
  ],
  webServer: editions.map((e) => ({
    command: `npx vite --config ${e.dir}/vite.config.ts --host 127.0.0.1 --port ${e.url.split(':').at(-1)} --strictPort`,
    url: e.url,
    // The repo root: the command runs from this config's folder otherwise.
    cwd: ROOT,
    reuseExistingServer: false,
    timeout: 120_000,
  })),
})
