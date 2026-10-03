// @vitest-environment node
/** DESIGN.md documents what exists (design review 8A): every token it names must exist in index.css. */
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { durations } from '@/lib/motion'
import { ACCENTS, ACCENT_KEY, THEME_KEY } from './theme'

const doc = readFileSync(join(process.cwd(), 'DESIGN.md'), 'utf8')
const css = readFileSync(join(process.cwd(), 'common/src/index.css'), 'utf8')

describe('DESIGN.md', () => {
  it('names only tokens that index.css defines', () => {
    const named = new Set(
      [...doc.matchAll(/`(--[a-z0-9-]+)`/g)].map((m) => m[1]!).filter((t) => !t.endsWith('-')),
    )
    expect(named.size).toBeGreaterThan(10)
    for (const t of named) expect(css, t).toMatch(new RegExp(`${t}:`))
  })

  it('matches the storage keys, presets and motion values in code', () => {
    expect(doc).toContain(THEME_KEY)
    expect(doc).toContain(ACCENT_KEY)
    for (const a of ACCENTS) expect(doc).toContain(a.label.replace(' (Nasiko)', ''))
    // Each preset appears by name with its value in ms on the same line.
    for (const [name, ms] of Object.entries(durations)) {
      expect(doc, name).toMatch(
        new RegExp(
          `\`${name}\`[^\n]*\\b${ms}\\b[^\n]*ms|\`${name}\`[^\n]*${ms}\\s?/\\s?\\d+\\s?ms`,
        ),
      )
    }
  })
})
