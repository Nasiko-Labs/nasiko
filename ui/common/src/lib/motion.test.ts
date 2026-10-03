// @vitest-environment node
import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'
import { ROW, LABEL } from '@/app/shell/rowStyles'
import { durations, ease, enter, transitions } from './motion'

const read = (p: string) => readFileSync(new URL(`../${p}`, import.meta.url), 'utf8')

describe('motion presets (plans/feat-app-shell.md §5.4)', () => {
  it('has the §5.4 durations', () => {
    expect(durations).toEqual({
      fast: 150,
      base: 180,
      standard: 200,
      panelIn: 220,
      panelOut: 160,
      wordHold: 2800,
    })
    expect(transitions.collapse).toEqual({ duration: 0.18, ease: 'easeOut' })
    expect(transitions.panelIn.duration).toBe(0.22)
    expect(transitions.panelOut.duration).toBe(0.16)
  })

  it("pins today's inline values, so moving them onto presets later changes nothing (eng D5)", () => {
    // tokenops Disclosure / harnesses panels: { duration: 0.2, ease: 'easeOut' }; rows rise 6, live rows drop 8, 0.03 s stagger.
    expect(transitions.disclosure).toEqual({ duration: 0.2, ease: 'easeOut' })
    expect(ease).toBe('easeOut')
    expect(enter).toMatchObject({ rise: 6, drop: -8, stagger: 0.03 })
    expect(transitions.morph).toEqual({})
    // The shared Disclosure animates in CSS: tw-animate-css's collapsible keyframes default to 0.2 s ease-out.
    expect(read('components/shared/disclosure.tsx')).toMatch(
      /animate-collapsible-down.*motion-reduce:animate-none/,
    )
    expect(read('features/harnesses/components/HarnessPanels.tsx')).toMatch(/y: 6/)
    expect(read('features/sessions/SessionRow.tsx')).toMatch(/y: -8/)
  })

  it('times the sheets from panelIn / panelOut, with a fade-only reduced-motion variant on every side', () => {
    const sheet = read('components/ui/sheet.tsx')
    expect(sheet).toContain(`data-[state=open]:duration-[${durations.panelIn}ms]`)
    expect(sheet).toContain(`data-[state=closed]:duration-[${durations.panelOut}ms]`)
    for (const v of [
      'enter-translate-x',
      'enter-translate-y',
      'exit-translate-x',
      'exit-translate-y',
    ]) {
      expect(sheet).toContain(`motion-reduce:[--tw-${v}:0]!`)
    }
    expect(sheet).toContain('motion-reduce:data-[state=open]:fade-in-0')
    expect(sheet).not.toMatch(/duration-(300|500)\b/)
  })

  it('times the sidebar collapse from `base`, off under reduced motion (eng D11)', () => {
    const sidebar = read('components/ui/sidebar.tsx')
    const widthRules = sidebar.match(/transition-\[(?:width|left,right,width)\][^"]*/g) ?? []
    expect(widthRules).toHaveLength(2)
    for (const r of widthRules) {
      expect(r).toContain(`duration-[${durations.base}ms] ease-out motion-reduce:transition-none`)
    }
    expect(sidebar).not.toMatch(/duration-200 ease-linear/)
  })

  it('fades the labels in step with the width, and grows rail rows on touch', () => {
    expect(LABEL).toContain(`duration-[${durations.base}ms] ease-out motion-reduce:transition-none`)
    // The primitive's `group-data-[collapsible=icon]:size-8!` would otherwise keep touch rows at 32 px.
    expect(ROW).toContain('pointer-coarse:group-data-[collapsible=icon]:size-11!')
  })

  it('keeps menus, popovers and tooltips fade-only under reduced motion', () => {
    const css = read('index.css')
    // Substring selectors, because the primitives carry `data-[state=open]:animate-in`, never `.animate-in`.
    const rule =
      /@media \(prefers-reduced-motion: reduce\) \{\s*\[class\*="animate-in"\],\s*\[class\*="animate-out"\] \{([^}]*)\}/.exec(
        css,
      )
    expect(rule, 'global reduced-motion rule').not.toBeNull()
    for (const v of [
      '--tw-enter-translate-x: 0',
      '--tw-enter-translate-y: 0',
      '--tw-exit-translate-x: 0',
      '--tw-exit-translate-y: 0',
      '--tw-enter-scale: 1',
      '--tw-exit-scale: 1',
    ]) {
      expect(rule![1], v).toContain(`${v} !important`)
    }
    // Every primitive that animates carries a class the selector matches (plain or variant form).
    for (const f of ['dropdown-menu', 'popover', 'tooltip', 'dialog'])
      expect(read(`components/ui/${f}.tsx`), f).toMatch(
        /(?:^|[\s"])(?:data-\[state=open\]:)?animate-in\b/,
      )
  })
})
