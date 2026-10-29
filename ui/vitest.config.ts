// One Vitest project per edition this checkout has (scripts/editions.ts), each built from that edition's own Vite
// config: the shared packages' tests run against the OSS route tree, an edition's against its own. An edition's tests
// are its app's and its feature packages' (`<edition>/features/*`, beside `<edition>/app`), so this names none.
import { existsSync, readdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { defineConfig } from 'vitest/config'
import { findEditions } from './scripts/editions.ts'

const ROOT = dirname(fileURLToPath(import.meta.url))
// An edition's feature packages may bring their own test setup: `<edition>/features/<package>/src/test/setup.ts`.
const featureSetups = (dir: string) => {
  const features = join(ROOT, dirname(dir), 'features')
  if (!existsSync(features)) return []
  return readdirSync(features)
    .map((pkg) => `./${dirname(dir)}/features/${pkg}/src/test/setup.ts`)
    .filter((file) => existsSync(join(ROOT, file)))
}

export default defineConfig({
  test: {
    projects: findEditions(ROOT).map(({ id, dir }) => ({
      extends: `./${dir}/vite.config.ts`,
      root: ROOT,
      test: {
        name: id,
        environment: 'jsdom',
        setupFiles: ['./shared/testing/src/setup.ts', ...featureSetups(dir)],
        // Prints once when nasiko-cloud-rs has server changes since the live fixtures were recorded (never fails).
        globalSetup: ['./shared/testing/src/liveStaleness.globalSetup.ts'],
        include: [
          ...(id === 'oss' ? ['shared/*/src/**/*.test.{ts,tsx}'] : []),
          `${dir}/src/**/*.test.{ts,tsx}`,
          `${dirname(dir)}/features/*/src/**/*.test.{ts,tsx}`,
        ],
        // A file's first page render waits for the React Compiler's Babel pass over the app (single-threaded, in the
        // main process), so the first tests of a parallel run need longer than Vitest's 5 s default. A CI runner
        // (CI=true) gets more, above setup.ts's 30 s findBy wait there.
        testTimeout: process.env.CI === 'true' ? 60_000 : 20_000,
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
      // A CI shard (VITEST_SHARD set) holds a slice of the suite, so its own coverage is meaningless against the
      // whole-tree minimum; the thresholds apply once, when `vitest --merge-reports --coverage` joins the shards.
      thresholds: process.env.VITEST_SHARD
        ? undefined
        : { lines: 60, statements: 60, functions: 60, branches: 60 },
    },
  },
})
