// @vitest-environment node
/**
 * N18: a live build must not carry the harness seed in any eagerly loaded chunk: it may exist only as a lazy chunk
 * reached through mock-mode dynamic imports. And an OSS build carries no EE layer code at all, mocks and personas
 * included (docs/lab-vs-react-migration-review.md §10.5; the EE build's own check is the layer's test).
 */
import { mkdtempSync, readdirSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'

const outDir = mkdtempSync(join(tmpdir(), 'harness-bundle-'))
afterAll(() => rmSync(outDir, { recursive: true, force: true }))

// One production build, shared by the checks below.
beforeAll(async () => {
  const { build } = await import('vite')
  // An empty envDir: a developer's .env.local (e.g. VITE_NASIKO_ALLOW_MOCK_BUILD) can't turn this into a mock build.
  const envDir = mkdtempSync(join(tmpdir(), 'harness-env-'))
  try {
    await build({
      // The OSS edition's build (docs/lab-vs-react-migration-review.md §10.6), as `npm run build` makes it.
      configFile: fileURLToPath(new URL('../../../oss/vite.config.ts', import.meta.url)),
      logLevel: 'silent',
      mode: 'production',
      envDir,
      build: { outDir, emptyOutDir: true },
    })
  } finally {
    rmSync(envDir, { recursive: true, force: true })
  }
}, 60_000)

const assets = () => join(outDir, 'assets')
const js = () => readdirSync(assets()).filter((f) => f.endsWith('.js'))
const carriers = (re: RegExp) =>
  js().filter((f) => re.test(readFileSync(join(assets(), f), 'utf8')))

describe('live bundle boundary', () => {
  it('keeps seed code out of every chunk but its own lazy one', () => {
    // The seed id prefix exists only in seed-harness.ts, which only the mock bootstrap (browser.ts) reaches.
    const seedChunks = carriers(/5eed0002/)
    expect(seedChunks).toHaveLength(1)
    expect(seedChunks[0]).toMatch(/^(seed-harness|browser)-/)
    const entry = /src="\/assets\/([^"]+\.js)"/.exec(
      readFileSync(join(outDir, 'index.html'), 'utf8'),
    )![1]!
    // The entry names the chunk only in Vite's preload map for a dynamic import, never as a static import.
    expect(readFileSync(join(assets(), entry), 'utf8')).not.toMatch(
      /from\s*["']\.\/(seed-harness|browser)-/,
    )
  })

  it('carries no EE layer code: no personas, org-level copy, EE grant or crash-guardian mocks', () => {
    for (const marker of [
      /Manager · nested units/, // the mock personas
      /View as \(mock persona\)/, // the persona switcher
      /Org and team views/, // the org page's status line
      /Shared with org units|Not shared with any org unit/, // the Access tab's unit grantees
      /OOMKilled/, // the crash guardian's recorded reason
      /Enterprise edition/, // the retired "needs Enterprise" lines
    ])
      expect(carriers(marker), String(marker)).toEqual([])
  })
})

/**
 * App shell eng D8 (R7): the shadcn animation classes do something only if tw-animate-css is in
 * the emitted CSS. A class-string test alone would pass on inert classes.
 */
describe('animation CSS', () => {
  it('defines the sheet and menu animation utilities and their reduced-motion overrides', () => {
    const css = readdirSync(assets())
      .filter((f) => f.endsWith('.css'))
      .map((f) => readFileSync(join(assets(), f), 'utf8'))
      .join('\n')
    expect(css).toMatch(/@keyframes enter/)
    expect(css).toMatch(/@keyframes exit/)
    for (const cls of [
      'animate-in',
      'animate-out',
      'slide-in-from-right',
      'slide-in-from-left',
      'fade-in-0',
    ]) {
      expect(css, cls).toMatch(new RegExp(`\\.[^{]*${cls}`))
    }
    // The sheet's reduced-motion rule zeroes the slide inside a prefers-reduced-motion query.
    expect(css).toMatch(
      /@media\s*\(prefers-reduced-motion:\s*reduce\)\s*\{[^@]*--tw-enter-translate-x:\s*0\s*!important/,
    )
    // Sheet timings reach the animation: tw-animate-css reads --tw-duration.
    expect(css).toMatch(/--tw-duration:\s*\.22s/)
    expect(css).toMatch(/var\(--tw-animation-duration,\s*var\(--tw-duration/)
  })
})
