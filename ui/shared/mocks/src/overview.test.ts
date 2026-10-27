/**
 * The Overview mocks (OV-1..OV-3) through MSW: the estate parses as the UI's wire type, every problem, chain step and
 * opportunity names a node on the map, OV-2 writes persist until the mocks reset, pins round-trip, and the variants
 * answer as a server without the routes (`overview-absent`) or a healthy estate (`overview-quiet`).
 */
import { afterEach, describe, expect, it } from 'vitest'
import { estateSchema, pinsSchema } from '@nasiko/features/overview/estate/types'
import { indexEstate } from '@nasiko/features/overview/estate/model'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { configureMocks, resetAgentsMock } from './handlers'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null }))

const json = async (path: string, init?: RequestInit) => {
  const r = await fetch(new URL(path, globalThis.location.origin), init)
  return { status: r.status, body: r.status === 200 ? ((await r.json()) as unknown) : null }
}
const estate = async () =>
  estateSchema.parse(
    ((await json('/api/overview/estate?range=30d')).body as { data: unknown }).data,
  )

describe('OV-1', () => {
  it('answers the wire type, every reference a node on the map', async () => {
    const e = await estate()
    const nodes = indexEstate(e)
    expect(e.problems.length).toBeGreaterThan(5)
    expect(e.days).toHaveLength(14)
    for (const p of e.problems)
      for (const id of p.entities) expect(nodes.has(id), `${p.id} → ${id}`).toBe(true)
    for (const c of e.chains)
      for (const s of c.steps) expect(nodes.has(s.entity), `${c.id} → ${s.entity}`).toBe(true)
    for (const o of e.opportunities) for (const id of o.entities) expect(nodes.has(id)).toBe(true)
    for (const p of e.problems)
      if (p.chain_id) expect(e.chains.some((c) => c.id === p.chain_id)).toBe(true)
    expect(e.mine).toContain(e.me)
  })

  it('names agents as the catalog does and tools as the MCP page does', async () => {
    const e = await estate()
    expect(e.agents.map((a) => a.name)).toContain('Code Reviewer')
    expect(e.tools.map((t) => t.name)).toEqual(expect.arrayContaining(['jira-cloud', 'github']))
  })

  it('a quiet estate has nothing falling behind', async () => {
    configureMocks({ variant: 'overview-quiet' })
    const e = await estate()
    expect(e.problems).toEqual([])
    expect(e.chains).toEqual([])
  })

  it('a server without the routes answers bare 404s', async () => {
    configureMocks({ variant: 'overview-absent' })
    for (const path of ['/api/overview/estate', '/api/me/overview-pins'])
      expect((await json(path)).status).toBe(404)
  })
})

describe('OV-2', () => {
  it('keeps an owner, a snooze and handled until the mocks reset', async () => {
    const patch = (body: unknown) =>
      json('/api/overview/problems/ov-p2', {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      })
    expect((await patch({ owner_id: 'someone' })).status).toBe(200)
    expect((await patch({ snoozed_until: '2099-01-01T00:00:00Z' })).status).toBe(200)
    let p = (await estate()).problems.find((x) => x.id === 'ov-p2')!
    expect(p).toMatchObject({
      owner_id: 'someone',
      snoozed_until: '2099-01-01T00:00:00Z',
      handled_at: null,
    })
    await patch({ handled: true })
    p = (await estate()).problems.find((x) => x.id === 'ov-p2')!
    expect(p.handled_at).not.toBeNull()
    resetAgentsMock()
    p = (await estate()).problems.find((x) => x.id === 'ov-p2')!
    expect(p).toMatchObject({ owner_id: null, snoozed_until: null, handled_at: null })
    expect(
      (await json('/api/overview/problems/nope', { method: 'PATCH', body: '{}' })).status,
    ).toBe(404)
  })
})

describe('OV-3', () => {
  it('starts unset, then keeps the order it was given', async () => {
    expect(pinsSchema.parse((await json('/api/me/overview-pins')).body).pins).toBeNull()
    const put = await json('/api/me/overview-pins', {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ pins: ['spend', 'agent-status'] }),
    })
    expect(put.status).toBe(200)
    expect(pinsSchema.parse((await json('/api/me/overview-pins')).body).pins).toEqual([
      'spend',
      'agent-status',
    ])
    const bad = await json('/api/me/overview-pins', {
      method: 'PUT',
      body: JSON.stringify({ pins: 'spend' }),
    })
    expect(bad.status).toBe(422)
    resetAgentsMock()
  })
})
