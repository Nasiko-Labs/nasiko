// One Vitest project per edition this checkout has (scripts/editions.ts), each built from that edition's own Vite
// config: the shared packages' tests run against the OSS route tree, an edition's against its own. An edition's tests
// are its app's and its feature packages' (`<edition>/features/*`, beside `<edition>/app`), so this names none.
import { dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { defineConfig } from 'vitest/config'
import { findEditions } from './scripts/editions.ts'

const ROOT = dirname(fileURLToPath(import.meta.url))

export default defineConfig({
  test: {
    projects: findEditions(ROOT).map(({ id, dir }) => ({
      extends: `./${dir}/vite.config.ts`,
      root: ROOT,
      test: {
        name: id,
        environment: 'jsdom',
        setupFiles: ['./shared/testing/src/setup.ts'],
        // Prints once when nasiko-cloud-rs has server changes since the live fixtures were recorded (never fails).
        globalSetup: ['./shared/testing/src/liveStaleness.globalSetup.ts'],
        include: [
          ...(id === 'oss' ? ['shared/*/src/**/*.test.{ts,tsx}'] : []),
          `${dir}/src/**/*.test.{ts,tsx}`,
          `${dirname(dir)}/features/*/src/**/*.test.{ts,tsx}`,
        ],
        // A file's first page render waits for the React Compiler's Babel pass over the app (single-threaded, in the
        // main process), so the first tests of a parallel run need longer than Vitest's 5 s default.
        testTimeout: 20_000,
        css: false,
        // One time zone for every run (v1c E9): Chat's rail groups by local calendar day.
        env: { TZ: 'UTC' },
      },
    })),
    // `npm run coverage` fails below CLAUDE.md's minimum (60%); the target is 80%.
    coverage: {
      include: ['*/*/src/**/*.{ts,tsx}', '*/*/*/src/**/*.{ts,tsx}'],
      exclude: [
        '**/*.test.{ts,tsx}',
        '**/*.stories.tsx',
        'shared/testing/src/**',
        '**/mocks/**',
        '**/*.gen.ts',
        // Vendored, framework-free, and not ours to cover. Matched by shape
        // rather than by edition path: a published config names no edition.
        '**/src/genui/lang/**',
        '**/main.tsx',
      ],
      thresholds: { lines: 60, statements: 60, functions: 60, branches: 60 },
    },
  },
})
