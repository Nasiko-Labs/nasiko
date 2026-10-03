// @vitest-environment node
/** CSS rules the app shell depends on (plans/feat-app-shell.md §5.3, eng D5). */
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { sharedSearchSchema } from './context'
import { tokenopsSearchSchema } from '@/features/tokenops/search'
import { MOCK_VARIANTS } from '@/mocks/handlers'
import { ACCENTS } from './theme'

const SRC = new URL('../../', import.meta.url).pathname
const css = readFileSync(join(SRC, 'index.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '')

/** `selector { body }` blocks at the top level of index.css. */
function blocks(): { selector: string; body: string }[] {
  return [...css.matchAll(/(^|\n)([^{}\n][^{}]*?)\s*\{([^{}]*)\}/g)].map((m) => ({
    selector: m[2]!.trim().split('\n').pop()!.trim(),
    body: m[3]!,
  }))
}

describe('index.css tokens', () => {
  it('defines --primary-text once, as the primary itself', () => {
    const root = blocks().find((b) => b.selector === ':root')!
    expect(root.body).toMatch(/--primary-text:\s*var\(--primary\);/)
    expect(
      blocks()
        .filter((b) => /--primary-text:/.test(b.body))
        .map((b) => b.selector),
    ).toEqual([':root'])
  })

  it('maps every sidebar token onto an existing token', () => {
    const root = blocks().find((b) => b.selector === ':root')!
    // The sidebar's "accent" is its hover fill, so it maps to --muted (the active row uses --accent).
    const map: Record<string, string> = {
      'sidebar-foreground': 'foreground',
      'sidebar-primary': 'primary',
      'sidebar-primary-foreground': 'primary-foreground',
      'sidebar-accent': 'muted',
      'sidebar-accent-foreground': 'foreground',
      'sidebar-border': 'border',
      'sidebar-ring': 'ring',
    }
    for (const [t, to] of Object.entries(map)) {
      expect(root.body, t).toMatch(new RegExp(`--${t}:\\s*var\\(--${to}\\);`))
    }
    expect(css).not.toMatch(/--sidebar[a-z-]*:\s*hsl/)
  })

  it('has no retired accent presets, and status colours stay per mode, outside the theme blocks', () => {
    expect(css).not.toMatch(/data-accent/)
    for (const b of blocks().filter((x) => x.selector.includes('data-theme'))) {
      expect(b.body, b.selector).not.toMatch(/--(success|warning|destructive|info|logo):/)
    }
  })
})

describe('accent text class (design D3)', () => {
  // TypeScript is covered by ESLint (eslint.config.js `banned`: any string or template literal with bare
  // text-primary); this checks the stylesheets, which ESLint doesn't read.
  it('colours accent text with text-primary-text, never text-primary, in any stylesheet', () => {
    const offenders: string[] = []
    const walk = (dir: string) => {
      for (const f of readdirSync(dir)) {
        const p = join(dir, f)
        if (statSync(p).isDirectory()) walk(p)
        else if (f.endsWith('.css')) {
          readFileSync(p, 'utf8')
            .split('\n')
            .forEach((line, i) => {
              if (/text-primary(?![-\w])/.test(line))
                offenders.push(`${p.slice(SRC.length)}:${i + 1}`)
            })
        }
      }
    }
    walk(SRC)
    expect(offenders).toEqual([])
    expect(readFileSync(join(SRC, 'components/ui/button.tsx'), 'utf8')).toMatch(
      /link: "text-primary-text/,
    )
    expect(readFileSync(join(SRC, 'components/ui/badge.tsx'), 'utf8')).toMatch(
      /link: "text-primary-text/,
    )
  })
})

describe('theme swatches', () => {
  it("show each theme's light --primary exactly", () => {
    const light = (sel: string) =>
      blocks()
        .find((b) => b.selector === sel)!
        .body.match(/--primary:\s*([^;]+);/)![1]!
        .trim()
    for (const a of ACCENTS) {
      const expected = light(`:root[data-theme="${a.id}"]`)
      expect(a.swatch.toLowerCase(), a.id).toBe(expected.toLowerCase())
    }
  })
})

describe('mock variants carried between pages', () => {
  it('the shared context accepts every observability and shell variant the mocks know', () => {
    const expected = [
      'tempo-down',
      'empty',
      'trace-503',
      'trace-500',
      'scan-fail',
      'server-down',
      'logout-unavailable',
    ]
    for (const v of expected) {
      expect(MOCK_VARIANTS as readonly string[]).toContain(v)
      expect(sharedSearchSchema.parse({ mock: v }).mock, v).toBe(v)
    }
    // TokenOps extends the shared schema, so it accepts the same variants.
    for (const v of expected) expect(tokenopsSearchSchema.parse({ mock: v }).mock, v).toBe(v)
  })
})

describe('TokenOps month calendar', () => {
  it('never uses the primary fill as a ring; the selected day uses the ring token', () => {
    const src = readFileSync(join(SRC, 'features/tokenops/components/MonthHero.tsx'), 'utf8')
    expect(src).not.toMatch(/\bring-primary\b(?!-)/)
    expect(src).toMatch(/\bring-ring\b/)
  })
})
