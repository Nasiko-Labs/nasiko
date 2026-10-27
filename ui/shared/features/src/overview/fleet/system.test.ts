/**
 * The Map's System card over the OV-1 mock: every part of the workspace once, by section and kind; the filters split it
 * into needs attention and healthy without losing a part; a search keeps only what it names; a group's dot is its
 * worst part.
 */
import { describe, expect, it } from 'vitest'
import { estate, viewOf } from '@nasiko/testing/estateView'
import { setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { buildBoard } from './model'
import { filterSystem, needsAttention, systemCounts, systemSections, worst } from './system'

setupPinnedSeed()

async function setup() {
  const e = await estate()
  const v = viewOf(e)
  return { e, v, sections: systemSections(v, buildBoard(v)) }
}

describe('the System card', () => {
  it("lists every part once, in the board's sections, Org and Fleet Runtime", async () => {
    const { e, sections } = await setup()
    expect(sections.map((s) => s.key)).toEqual(['org', 'fleet'])
    const rows = (key: string) =>
      sections.flatMap((s) => s.groups).find((g) => g.key === key)?.rows ?? []
    expect(rows('agents')).toHaveLength(e.agents.length)
    expect(rows('workflows')).toHaveLength(e.workflows.length)
    // Every tool, the ones turned off long ago (off the board) too.
    expect(rows('tools')).toHaveLength(e.tools.length)
    expect(rows('routers')).toHaveLength(e.routers.filter((r) => !r.direct).length)
    expect(rows('providers')).toHaveLength(e.providers.length)
    expect(rows('models')).toHaveLength(e.models.length)
    expect(rows('harnesses')).toHaveLength(new Set(e.harnesses.map((h) => h.harness)).size)
    expect(rows('teams').length).toBeGreaterThanOrEqual(e.groups.length)
    // The biggest spender first, and a row opens its node.
    const agents = rows('agents')
    const top = [...e.agents].sort((a, b) => b.spend_usd - a.spend_usd)[0]
    expect(agents[0]?.sel).toBe(`node:${top?.id}`)
  })

  it('splits into needs attention and healthy, and a search keeps only what it names', async () => {
    const { sections } = await setup()
    const c = systemCounts(sections)
    expect(c.attention).toBeGreaterThan(0)
    expect(c.attention + c.healthy).toBe(c.all)
    const attention = filterSystem(sections, '', 'attention')
    expect(systemCounts(attention).all).toBe(c.attention)
    for (const r of attention.flatMap((s) => s.groups.flatMap((g) => g.rows)))
      expect(needsAttention(r.tone)).toBe(true)
    const found = filterSystem(sections, 'JIRA', 'all').flatMap((s) =>
      s.groups.flatMap((g) => g.rows),
    )
    expect(found.map((r) => r.name)).toEqual(['jira-cloud'])
    expect(filterSystem(sections, 'no such part', 'all')).toEqual([])
  })

  it("dots a group with its worst part's tone", () => {
    const row = (tone: 'ok' | 'warn' | 'bad' | 'idle') => ({ sel: 'x', name: 'x', value: '', tone })
    expect(worst([row('ok'), row('warn'), row('idle')])).toBe('warn')
    expect(worst([row('ok'), row('bad'), row('warn')])).toBe('bad')
    expect(worst([])).toBe('none')
  })
})
