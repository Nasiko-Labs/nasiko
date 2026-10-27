/**
 * The usage endpoint as the OSS server answers it (docs/designs/openruntime-harness-endpoint-requirements.md §3):
 * only `user_id = self`, and the landing is self, for every login. The EE layer tests its org rules on its own.
 */
import { describe, expect, it } from 'vitest'
import { usage, UsageHttpError } from './harnessUsage'
import { codingAgentName, generateHarnessSeed } from './seed-harness'

const NOW = Date.parse('2026-03-20T15:00:00Z')
const seed = generateHarnessSeed({ anchor: new Date(NOW) })
const user = (name: string) => seed.users.find((u) => u.username === name)!
const call = (who: string, p: Record<string, string> = {}) => usage(seed, p, user(who), NOW)
const err = (fn: () => unknown) => {
  try {
    fn()
  } catch (e) {
    return e as UsageHttpError
  }
  throw new Error('expected an error')
}

describe('harness seed', () => {
  it('names agents like nasiko-server coding_agent_name', () => {
    expect(codingAgentName('Priya.Nair', 'claude')).toBe('priya-nair-claude-code')
    expect(codingAgentName("o'neil__x", 'cursor')).toBe('o-neil-x-cursor')
  })

  it('is deterministic', () => {
    const again = generateHarnessSeed({ anchor: new Date(NOW) })
    expect(again.sessions.length).toBe(seed.sessions.length)
    expect(again.sessions[0]).toEqual(seed.sessions[0])
  })
})

describe('landing (no params)', () => {
  it.each(['root', 'admin', 'maya', 'sam'])('%s lands on their own Individual level', (who) => {
    const res = call(who)
    expect(res.scope).toMatchObject({ kind: 'user', user_id: user(who).id, visibility: 'named' })
    // OSS has no org tree: no unit placement.
    expect(res.scope).not.toHaveProperty('units')
    expect(res.rows).toEqual([])
    expect(res.recent_sessions).toBeDefined()
  })

  it('asking for yourself is the landing', () => {
    expect(call('sam', { user_id: user('sam').id })).toEqual(call('sam'))
  })
})

describe('scope rule and errors', () => {
  it('every scope is a coded 404, superusers included (no org view on OSS)', () => {
    for (const scope of ['org', 'mine', 'unassigned']) {
      const e = err(() => call('root', { scope }))
      expect([e.status, e.code]).toEqual([404, 'unit_not_visible'])
    }
    const e = err(() => call('admin', { scope: 'unit', unit_id: seed.units[0]!.id }))
    expect([e.status, e.code]).toEqual([404, 'unit_not_visible'])
  })

  it('another developer is a coded 404, for an admin too', () => {
    expect(err(() => call('admin', { user_id: user('sam').id })).code).toBe('user_not_visible')
  })

  it('an unknown scope is a coded 400', () => {
    expect(err(() => call('admin', { scope: 'everything' })).code).toBe('invalid_scope')
  })

  it('range replaces start_time and anchors to end_time (or now), as finops does', () => {
    const res = call('admin', {
      range: '7d',
      start_time: '2020-01-01T00:00:00Z',
      end_time: '2026-03-10T00:00:00Z',
    })
    expect(res.window.range).toBe('7d')
    expect(res.window.end_time).toBe('2026-03-10T00:00:00.000Z')
    expect(res.window.start_time).toBe('2026-03-03T00:00:00.000Z')
    expect(Date.parse(call('admin', { range: '7d' }).window.end_time)).toBe(NOW)
  })

  it('each bad window has its own code', () => {
    expect(err(() => call('admin', { range: '9d' })).code).toBe('invalid_range')
    expect(err(() => call('admin', { start_time: 'x', end_time: 'y' })).code).toBe('invalid_window')
  })

  it('the Individual level still carries the paging fields (API_CONVENTIONS §1)', () => {
    expect(call('admin')).toMatchObject({ has_more: false, next_cursor: null, prev_cursor: null })
  })
})

describe('rollups', () => {
  it('a reinstall is one registration; all-deleted rows are no registration but their activity counts', () => {
    const claude = call('sam').by_harness.find((h) => h.harness === 'claude')!
    expect(claude.registered_devs).toBe(1)
    const lapsed = seed.agents.find(
      (a) =>
        a.deleted &&
        a.harness === 'codex' &&
        !seed.agents.some((b) => b.owner_id === a.owner_id && b.harness === 'codex' && !b.deleted),
    )!
    const owner = seed.users.find((u) => u.id === lapsed.owner_id)!
    const codex = call(owner.username).by_harness.find((h) => h.harness === 'codex')!
    expect(codex.registered_devs).toBe(0)
    expect(codex.turns).toBeGreaterThan(0)
  })

  it('idle = registered with no activity, per harness', () => {
    const all = seed.users.filter((x) => x.is_active).map((u) => call(u.username))
    for (const res of all)
      for (const h of res.by_harness) expect(h.idle_seats).toBeLessThanOrEqual(h.registered_devs)
    expect(all.some((res) => res.totals.idle_seats > 0)).toBe(true)
  })

  it('keeps unknown harness ids', () => {
    const owner = seed.sessions.find((s) => s.harness === 'windsurf' && s.ts >= NOW - 30 * 864e5)
    expect(owner).toBeDefined()
    const who = seed.users.find((u) => u.id === owner!.user_id)!
    expect(call(who.username).by_harness.map((h) => h.harness)).toContain('windsurf')
  })
})
