// @vitest-environment node
/**
 * WCAG AA for every colour theme × mode (docs/lab-vs-react-migration-review.md §6.4), computed from
 * the tokens in src/index.css. A small OKLCH → sRGB helper; no dependency.
 */
import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'
import { ACCENTS } from './theme'

const css = readFileSync(new URL('../../index.css', import.meta.url), 'utf8').replace(
  /\/\*[\s\S]*?\*\//g,
  '',
)

function block(selector: string): Record<string, string> {
  const out: Record<string, string> = {}
  for (const m of css.matchAll(/(^|\n)([^{}]*?)\{([^{}]*)\}/g)) {
    if (m[2]!.trim().split('\n').pop()!.trim() !== selector) continue
    for (const d of m[3]!.matchAll(/(--[a-z0-9-]+):\s*([^;]+);/g)) out[d[1]!] = d[2]!.trim()
  }
  return out
}

/** The cascade for one theme × mode: shared :root, shared .dark, then the theme's own block. */
function tokens(theme: string, dark: boolean): Record<string, string> {
  return {
    ...block(':root'),
    ...(dark ? block('.dark') : {}),
    ...(dark ? block(`.dark[data-theme="${theme}"]`) : block(`:root[data-theme="${theme}"]`)),
  }
}

/** Linear-light sRGB channels, clamped to the gamut. */
function toLinearRgb(value: string, t: Record<string, string>): [number, number, number] {
  const v = /^var\((--[a-z0-9-]+)\)$/.exec(value)
  if (v) return toLinearRgb(t[v[1]!]!, t)
  const hex = /^#([0-9a-f]{6})$/i.exec(value)
  if (hex) {
    const n = Number.parseInt(hex[1]!, 16)
    const lin = (c: number) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4)
    return [lin(((n >> 16) & 255) / 255), lin(((n >> 8) & 255) / 255), lin((n & 255) / 255)]
  }
  const ok = /^oklch\(([\d.]+)\s+([\d.]+)\s+([\d.]+)\)$/.exec(value)
  if (!ok) throw new Error(`unsupported colour: ${value}`)
  const [L, C, H] = [Number(ok[1]), Number(ok[2]), (Number(ok[3]) * Math.PI) / 180]
  const a = C * Math.cos(H)
  const b = C * Math.sin(H)
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3
  const clamp = (x: number) => Math.min(1, Math.max(0, x))
  return [
    clamp(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s),
    clamp(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s),
    clamp(-0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s),
  ]
}

const luminance = ([r, g, b]: [number, number, number]) => 0.2126 * r + 0.7152 * g + 0.0722 * b

function contrast(fg: string, bg: string, t: Record<string, string>): number {
  const [a, b] = [luminance(toLinearRgb(t[fg]!, t)), luminance(toLinearRgb(t[bg]!, t))]
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05)
}

const THEMED = [
  '--background',
  '--sidebar',
  '--card',
  '--popover',
  '--muted',
  '--border',
  '--input',
  '--foreground',
  '--muted-foreground',
  '--primary',
  '--primary-hover',
  '--primary-foreground',
  '--accent',
  '--accent-foreground',
  '--ring',
  ...[1, 2, 3, 4, 5].flatMap((n) => [`--chart-${n}`, `--chart-${n}-edge`]),
]

describe('theme contrast (WCAG AA)', () => {
  it('knows white on the brand yellow fails, so the helper is calibrated', () => {
    const t = { '--a': '#ffffff', '--b': '#bb8f06' }
    expect(contrast('--a', '--b', t)).toBeCloseTo(2.98, 1)
  })

  for (const theme of ACCENTS.map((a) => a.id)) {
    for (const dark of [false, true]) {
      it(`${theme} ${dark ? 'dark' : 'light'}`, () => {
        const t = tokens(theme, dark)
        // Every themed token is set by this theme × mode's own block, so none leaks from another.
        const own = dark
          ? block(`.dark[data-theme="${theme}"]`)
          : block(`:root[data-theme="${theme}"]`)
        for (const k of THEMED) expect(own[k], k).toBeDefined()
        expect(contrast('--foreground', '--card', t), 'text on cards').toBeGreaterThanOrEqual(7)
        expect(
          contrast('--muted-foreground', '--card', t),
          'secondary text on cards',
        ).toBeGreaterThanOrEqual(4.5)
        expect(
          contrast('--muted-foreground', '--sidebar', t),
          'secondary text on the sidebar',
        ).toBeGreaterThanOrEqual(4.5)
        expect(
          contrast('--primary-foreground', '--primary', t),
          'button text on the fill',
        ).toBeGreaterThanOrEqual(4.5)
        expect(
          contrast('--primary-foreground', '--primary-hover', t),
          'button text on the hover fill',
        ).toBeGreaterThanOrEqual(4.5)
        expect(
          contrast('--primary-text', '--background', t),
          'accent text on the page',
        ).toBeGreaterThanOrEqual(4.5)
        expect(
          contrast('--primary-text', '--card', t),
          'accent text on cards',
        ).toBeGreaterThanOrEqual(4.5)
        expect(
          contrast('--accent-foreground', '--accent', t),
          'active nav / badge text on its tint',
        ).toBeGreaterThanOrEqual(4.5)
        // A hovered chat rail row (--muted) keeps its secondary line (agent · time) readable; the open row's line takes
        // --accent-foreground, checked above.
        expect(
          contrast('--muted-foreground', '--muted', t),
          'secondary text on the hover fill',
        ).toBeGreaterThanOrEqual(4.5)
        expect(contrast('--ring', '--background', t), 'focus ring').toBeGreaterThanOrEqual(3)
        // Logotypes are exempt from WCAG contrast; light mode's yellow-600 mark is ~2.9:1 by design (§6.4).
        if (dark)
          expect(
            contrast('--logo', '--sidebar', t),
            'logo on the dark sidebar',
          ).toBeGreaterThanOrEqual(7)
        for (const s of ['--success', '--warning', '--destructive', '--info']) {
          expect(contrast(s, '--card', t), `${s} on cards`).toBeGreaterThanOrEqual(4.5)
        }
        // Light mode draws each series' edge; dark mode draws the fill alone (edge = fill).
        for (const n of [1, 2, 3, 4, 5]) {
          expect(contrast(`--chart-${n}-edge`, '--card', t), `chart ${n}`).toBeGreaterThanOrEqual(3)
          if (dark) expect(t[`--chart-${n}-edge`], `chart ${n} edge off`).toBe(t[`--chart-${n}`])
        }
      })
    }
  }

  it('keeps the login headline card readable over every hue of the glow, in both modes', () => {
    // The card's glass over whichever glow hue (or the page) is behind it: 58% white in light mode, 62% black in dark.
    const blend = (hex: string, toward: number, amount: number) =>
      '#' +
      [1, 3, 5]
        .map((i) =>
          Math.round(toward * amount + Number.parseInt(hex.slice(i, i + 2), 16) * (1 - amount))
            .toString(16)
            .padStart(2, '0'),
        )
        .join('')
    const modes = [
      { name: 'light', t: block(':root'), toward: 255, amount: 0.58 },
      { name: 'dark', t: { ...block(':root'), ...block('.dark') }, toward: 0, amount: 0.62 },
    ]
    for (const { name, t, toward, amount } of modes) {
      for (const stop of ['#a49b8d', '#e3b386', '#a8808a', '#7fb08c', '#8c9796', '#faf8f4']) {
        const over = { ...t, '--card-over-glow': blend(stop, toward, amount) }
        expect(
          contrast('--showcase-card-foreground', '--card-over-glow', over),
          `${name}: text over ${stop}`,
        ).toBeGreaterThanOrEqual(4.5)
        // The line under it: an opaque colour in light mode (dark mode's is translucent white, above the text's 4.5).
        if (name === 'light')
          expect(
            contrast('--showcase-card-muted', '--card-over-glow', over),
            `${name}: line over ${stop}`,
          ).toBeGreaterThanOrEqual(4.5)
        // The word is heading-size (30-42 px semibold): WCAG's large-text minimum.
        expect(
          contrast('--showcase-accent', '--card-over-glow', over),
          `${name}: gold over ${stop}`,
        ).toBeGreaterThanOrEqual(3)
      }
    }
  })

  it("never leads a chart with the theme's own hue (Teal and Indigo avoid cornflower first, Plum avoids orchid)", () => {
    expect(tokens('teal', false)['--chart-1']).toBe('#4777d2')
    expect(tokens('indigo', false)['--chart-1']).toBe('#ffb98c')
    expect(tokens('plum', false)['--chart-1']).toBe('#8fedd0')
    for (const theme of ACCENTS.map((a) => a.id)) {
      const light = tokens(theme, false)
      const dark = tokens(theme, true)
      for (const n of [1, 2, 3, 4, 5])
        expect(dark[`--chart-${n}`], `${theme} chart ${n} same fill in both modes`).toBe(
          light[`--chart-${n}`],
        )
    }
  })
})
